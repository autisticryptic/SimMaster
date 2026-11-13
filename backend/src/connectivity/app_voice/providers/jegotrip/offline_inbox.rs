//! 原生离线页的事务消费：原包/SMS/事件/游标一起提交，才允许下一次取件。
//! 游标按同一账号修订、匹配 native UID/selector 与原 receiver 上下文隔离；
//! run nonce/请求 ID 不是远端消息身份。新连接先从 -1 取得真实身份，再复用
//! 相同上下文的持久游标。不以数据库缓存创建授信、SDK 就绪或厂商唤醒。
use super::{account_inbox::{self, InboxScope, STORAGE, SCOPE, FULL, INVALID}, account_runs, account_sms,
    native_events::IncomingMessage, rpc_offline::{decode_offline_message, OwnedOfflinePage},
    rpc_pending::RpcConnectionScope, session::AttemptOwner, smgw_text};
use crate::platform::db::{insert_app_event_for_conn, Database};
use ring::digest::{Context, SHA256};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use std::fmt;

const COLUMNS: &str = "id,line_id,revision,run_nonce,source_kind,peer_epoch,sequence,fingerprint,body,sender,opaque_time,receiver_object,receiver_domain,receiver_kind,reply_state,created_at,offline_identity,offline_envelope";

/// 只迁移本项目旧 CHECK 约束；事务中的临时表不是设备回滚备份。
/// 原行 ID、SQLite 自增水位、所有字段保留；未知附加字段/触发器不猜、不删。
pub(super) fn initialize_schema(conn: &Connection) -> Result<(), &'static str> {
    let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|_| STORAGE)?;
    let columns = {
        let mut query = tx.prepare("PRAGMA table_info(app_voice_inbox)").map_err(|_| STORAGE)?;
        let rows = query.query_map([], |row| row.get::<_, String>(1)).map_err(|_| STORAGE)?
            .collect::<Result<Vec<_>, _>>().map_err(|_| STORAGE)?; rows
    };
    for (name, sql) in [("offline_identity", "ALTER TABLE app_voice_inbox ADD COLUMN offline_identity TEXT"),
        ("offline_envelope", "ALTER TABLE app_voice_inbox ADD COLUMN offline_envelope BLOB")] {
        if !columns.iter().any(|column| column == name) { tx.execute(sql, []).map_err(|_| STORAGE)?; }
    }
    let original: String = tx.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='app_voice_inbox'", [], |row| row.get(0)).map_err(|_| STORAGE)?;
    let normalized: String = original.chars().filter(|character| !character.is_ascii_whitespace()).flat_map(char::to_lowercase).collect();
    if normalized.contains("check(source_kindin('direct','batch'))") || normalized.contains("check(reply_statein('pending','direct_submitted','batch_queued'))") {
        if columns.iter().any(|column| !COLUMNS.split(',').any(|known| known == column)) { return Err(STORAGE); }
        let custom: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE tbl_name='app_voice_inbox' AND sql IS NOT NULL
            AND (type='trigger' OR type='index' AND name!='idx_app_voice_inbox_line'))", [], |row| row.get(0)).map_err(|_| STORAGE)?;
        if custom { return Err(STORAGE); }
        let frontier: i64 = tx.query_row("SELECT seq FROM sqlite_sequence WHERE name='app_voice_inbox'", [], |row| row.get(0)).optional().map_err(|_| STORAGE)?.unwrap_or(0);
        tx.execute_batch("CREATE TEMP TABLE jegotrip_inbox_migration AS SELECT * FROM app_voice_inbox; DROP TABLE app_voice_inbox;")
            .map_err(|_| STORAGE)?;
        tx.execute_batch(account_inbox::CREATE_INBOX_SQL).map_err(|_| STORAGE)?;
        tx.execute_batch(&format!("INSERT INTO app_voice_inbox({COLUMNS}) SELECT {COLUMNS} FROM jegotrip_inbox_migration;
            DROP TABLE jegotrip_inbox_migration; CREATE INDEX idx_app_voice_inbox_line ON app_voice_inbox(line_id,id);"))
            .map_err(|_| STORAGE)?;
        tx.execute("INSERT INTO sqlite_sequence(name,seq) SELECT 'app_voice_inbox',?1 WHERE NOT EXISTS
            (SELECT 1 FROM sqlite_sequence WHERE name='app_voice_inbox')", [frontier]).map_err(|_| STORAGE)?;
        tx.execute("UPDATE sqlite_sequence SET seq=MAX(seq,?1) WHERE name='app_voice_inbox'", [frontier]).map_err(|_| STORAGE)?;
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_offline_cursors(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL,identity_key TEXT NOT NULL,next_position INTEGER NOT NULL CHECK(next_position>=0),
        PRIMARY KEY(line_id,revision,identity_key));
        CREATE TABLE IF NOT EXISTS app_voice_offline_quarantine(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL,identity_key TEXT NOT NULL,
        fingerprint TEXT NOT NULL,reason TEXT NOT NULL,envelope BLOB NOT NULL,created_at TEXT NOT NULL,
        UNIQUE(line_id,revision,identity_key,fingerprint));
        CREATE UNIQUE INDEX IF NOT EXISTS idx_app_voice_inbox_original_sequence ON app_voice_inbox(line_id,revision,run_nonce,source_kind,peer_epoch,sequence)
            WHERE source_kind IN ('direct','batch');
        CREATE UNIQUE INDEX IF NOT EXISTS idx_app_voice_offline_position ON app_voice_inbox(line_id,revision,offline_identity,sequence)
            WHERE source_kind='offline';").map_err(|_| STORAGE)?;
    tx.commit().map_err(|_| STORAGE)
}

pub(super) struct CommittedOfflineCursor { owner: AttemptOwner, scope: RpcConnectionScope, next_position: i64, server_position: i64 }
impl fmt::Debug for CommittedOfflineCursor { fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result { formatter.write_str("CommittedOfflineCursor(<redacted>)") } }
impl CommittedOfflineCursor {
    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && scope.is_open()
    }
    pub(super) fn next_position(&self) -> i64 { self.next_position }
    pub(super) fn more(&self) -> bool { self.next_position <= self.server_position }
}

fn context_key(context: &InboxScope, page: &OwnedOfflinePage) -> Result<String, &'static str> {
    if !page.matches(&context.owner, &context.scope) { return Err(SCOPE); }
    let receiver = context.receiver.as_ref().ok_or(SCOPE)?;
    let mut digest = Context::new(&SHA256);
    if receiver.domain.is_none() { digest.update(b"native-client-adapter-v1\0"); }
    for value in [page.identity_key().as_bytes(), receiver.object.as_slice(), receiver.domain.as_ref().map_or(&[][..], |domain| domain.as_slice()), receiver.kind.as_slice()] {
        digest.update(&(value.len() as u64).to_be_bytes()); digest.update(value);
    }
    Ok(digest.finish().as_ref().iter().map(|byte| format!("{byte:02x}")).collect())
}
fn quarantine(context: &InboxScope, database: &Database, page: &OwnedOfflinePage, key: &str, reason: &'static str) -> Result<(), &'static str> {
    let mut digest = Context::new(&SHA256); digest.update(page.raw());
    let fingerprint: String = digest.finish().as_ref().iter().map(|byte| format!("{byte:02x}")).collect();
    database.with_conn(|conn| {
        let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|_| STORAGE)?;
        account_inbox::require_account(&tx, &context.line_id, context.revision)?;
        account_runs::require_live(&tx, &context.line_id, context.revision, &context.nonce)?;
        let exists: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_offline_quarantine WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND fingerprint=?4)",
            params![context.line_id, context.revision as i64, key, fingerprint], |row| row.get(0)).map_err(|_| STORAGE)?;
        if !exists {
            let (local, total, bytes): (i64, i64, i64) = tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*),COALESCE(SUM(length(envelope)),0) FROM app_voice_offline_quarantine", [&context.line_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(|_| STORAGE)?;
            let inbox_bytes: i64 = tx.query_row("SELECT COALESCE(SUM(length(body)+COALESCE(length(sender),0)+COALESCE(length(opaque_time),0)
                +COALESCE(length(receiver_object),0)+COALESCE(length(receiver_domain),0)+COALESCE(length(receiver_kind),0)+COALESCE(length(offline_envelope),0)),0) FROM app_voice_inbox", [], |row| row.get(0)).map_err(|_| STORAGE)?;
            if local >= 8 || total >= 64 || bytes.checked_add(page.raw().len() as i64).is_none_or(|bytes| bytes > 8 * 1024 * 1024)
                || bytes.checked_add(inbox_bytes).and_then(|bytes| bytes.checked_add(page.raw().len() as i64)).is_none_or(|bytes| bytes > account_inbox::MAX_TOTAL_BYTES) { return Err(FULL); }
            tx.execute("INSERT INTO app_voice_offline_quarantine(line_id,revision,identity_key,fingerprint,reason,envelope,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                params![context.line_id, context.revision as i64, key, fingerprint, reason, page.raw(), chrono::Utc::now().to_rfc3339()]).map_err(|_| STORAGE)?;
        }
        // 这里不建/推进 cursor，不生成 consumer ACK 或 SMS。
        tx.commit().map_err(|_| STORAGE)
    })
}

/// 只有先前已提交的游标能压缩先前已持久化的已分类原包；本页不提前删除。
/// 未识别原包保留，SMS/删除墓碑独立；相同远端位置的重放不会重建已删除短信。
fn compact_prior(tx: &Transaction<'_>, line: &str, revision: u64, key: &str, through: i64) -> Result<(), &'static str> {
    tx.execute("UPDATE app_voice_sms_projection SET envelope_retained=0 WHERE line_id=?1 AND revision=?2 AND inbox_id IN
        (SELECT id FROM app_voice_inbox WHERE line_id=?1 AND revision=?2 AND source_kind='offline' AND offline_identity=?3 AND sequence<?4)",
        params![line, revision as i64, key, through]).map_err(|_| STORAGE)?;
    tx.execute("DELETE FROM app_voice_inbox WHERE line_id=?1 AND revision=?2 AND source_kind='offline' AND offline_identity=?3 AND sequence<?4
        AND EXISTS(SELECT 1 FROM app_voice_sms_projection p WHERE p.inbox_id=app_voice_inbox.id AND p.line_id=app_voice_inbox.line_id AND p.revision=app_voice_inbox.revision AND p.envelope_retained=0)",
        params![line, revision as i64, key, through]).map_err(|_| STORAGE)?;
    tx.execute("DELETE FROM app_voice_sms_projection WHERE line_id=?1 AND revision=?2 AND envelope_retained=0 AND deleted=1", params![line, revision as i64]).map_err(|_| STORAGE)?;
    Ok(())
}

impl InboxScope {
    pub(super) fn commit_offline(&self, database: &Database, page: &OwnedOfflinePage) -> Result<CommittedOfflineCursor, &'static str> {
        let key = context_key(self, page)?;
        let result = page.result().map_err(|_| INVALID)?;
        let mut decoded = Vec::with_capacity(result.records.len());
        for record in &result.records {
            match decode_offline_message(record.envelope) {
                Ok(message) => decoded.push(message),
                Err(error) => { quarantine(self, database, page, &key, error.code())?; return Err("app_account_offline_stream_unsupported"); }
            }
        }
        let receiver = self.receiver.as_ref().ok_or(SCOPE)?;
        let (next_position, events) = database.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Immediate).map_err(|_| STORAGE)?;
            account_inbox::require_account(&tx, &self.line_id, self.revision)?;
            account_runs::require_live(&tx, &self.line_id, self.revision, &self.nonce)?;
            let saved: Option<i64> = tx.query_row("SELECT next_position FROM app_voice_offline_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3",
                params![self.line_id, self.revision as i64, key], |row| row.get(0)).optional().map_err(|_| STORAGE)?;
            let current = saved.unwrap_or(-1);
            if page.requested_position() > current || result.server_position + 1 < current { return Err("app_account_offline_cursor_conflict"); }
            if current >= 0 { compact_prior(&tx, &self.line_id, self.revision, &key, current)?; }
            let mut events = Vec::new();
            for (record, decoded) in result.records.iter().zip(&decoded) {
                if record.position < current { continue; } // 已提交的原位置 fence，绝不再投影。
                let find = |name: &[u8]| decoded.properties.iter().find(|property| property.key == name).map(|property| property.value.to_vec());
                let message = IncomingMessage { body: decoded.body.to_vec(), from: find(b"Sip.Message.From"), time: find(b"Sip.Message.Time") };
                let size = account_inbox::message_size(&message)? + receiver.object.len() as i64 + receiver.domain.as_ref().map_or(0, |domain| domain.len()) as i64
                    + decoded.receiver_kind.len() as i64 + record.envelope.len() as i64;
                let (local, completed, total, stored): (i64, i64, i64, i64) = tx.query_row("SELECT
                    COALESCE(SUM(line_id=?1 AND (reply_state='pending' OR NOT EXISTS(SELECT 1 FROM app_voice_sms_projection p WHERE p.inbox_id=i.id AND p.line_id=i.line_id AND p.revision=i.revision))),0),
                    COALESCE(SUM(line_id=?1 AND reply_state!='pending' AND EXISTS(SELECT 1 FROM app_voice_sms_projection p WHERE p.inbox_id=i.id AND p.line_id=i.line_id AND p.revision=i.revision)),0),COUNT(*),
                    COALESCE(SUM(length(body)+COALESCE(length(sender),0)+COALESCE(length(opaque_time),0)+COALESCE(length(receiver_object),0)+COALESCE(length(receiver_domain),0)+COALESCE(length(receiver_kind),0)+COALESCE(length(offline_envelope),0)),0)+(SELECT COALESCE(SUM(length(envelope)),0) FROM app_voice_offline_quarantine) FROM app_voice_inbox i", [&self.line_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))).map_err(|_| STORAGE)?;
                if local >= account_inbox::MAX_PER_ACCOUNT || local + completed >= account_inbox::MAX_IDENTITIES_PER_ACCOUNT || total >= account_inbox::MAX_TOTAL
                    || stored.checked_add(size).is_none_or(|bytes| bytes > account_inbox::MAX_TOTAL_BYTES) { return Err(FULL); }
                let stamp = chrono::Utc::now().to_rfc3339();
                tx.execute("INSERT INTO app_voice_inbox(line_id,revision,run_nonce,source_kind,peer_epoch,sequence,fingerprint,body,sender,opaque_time,
                    receiver_object,receiver_domain,receiver_kind,reply_state,created_at,offline_identity,offline_envelope)
                    VALUES(?1,?2,?3,'offline',0,?4,?5,?6,?7,?8,?9,?10,?11,'offline_committed',?12,?13,?14)",
                    params![self.line_id, self.revision as i64, self.nonce, record.position, account_inbox::fingerprint(&message), message.body.as_slice(), message.from.as_deref(), message.time.as_deref(),
                        receiver.object.as_slice(), receiver.domain.as_ref().map(|domain| domain.as_slice()), decoded.receiver_kind, stamp, key, record.envelope]).map_err(|_| STORAGE)?;
                let id = tx.last_insert_rowid();
                let classified = (decoded.receiver_kind == receiver.kind.as_slice()).then(|| smgw_text::classify(decoded.receiver_kind, &message)).flatten();
                let classification = if classified.is_some() { "smgw_text" } else { "unclassified" };
                let payload = serde_json::json!({"inbox_id":id,"classification":classification,"source":"offline"}).to_string();
                events.push(insert_app_event_for_conn(&tx, "app_voice.inbox_stored", Some(&self.line_id), Some(super::PROVIDER_ID), &payload, &stamp).map_err(|_| STORAGE)?);
                if let Some(text) = classified {
                    let notices = self.notifications.as_ref().map_err(|code| *code)?;
                    events.push(account_sms::promote_new(&tx, id, &self.line_id, self.revision, &text, &stamp, notices.as_deref())?);
                }
            }
            // 原非空集合按最大实际位置+1，空页才使用服务器位置+1；不因 hint 跳过消息。
            let candidate = result.records.last().map_or(result.server_position + 1, |record| record.position + 1);
            let next = candidate.max(current);
            if saved.is_none() {
                let count: i64 = tx.query_row("SELECT COUNT(*) FROM app_voice_offline_cursors", [], |row| row.get(0)).map_err(|_| STORAGE)?;
                if count >= 4096 { return Err(FULL); }
            }
            tx.execute("INSERT INTO app_voice_offline_cursors(line_id,revision,identity_key,next_position) VALUES(?1,?2,?3,?4)
                ON CONFLICT(line_id,revision,identity_key) DO UPDATE SET next_position=MAX(next_position,excluded.next_position)",
                params![self.line_id, self.revision as i64, key, next]).map_err(|_| STORAGE)?;
            tx.commit().map_err(|_| STORAGE)?;
            Ok((next, events))
        })?;
        for event in events { database.publish_committed_app_event(event); }
        Ok(CommittedOfflineCursor { owner: self.owner.clone(), scope: self.scope.clone(), next_position, server_position: result.server_position })
    }
}

pub(super) fn delete_account(conn: &Connection, line: &str) -> Result<(), &'static str> {
    let exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_offline_cursors')", [], |row| row.get(0)).map_err(|_| STORAGE)?;
    if exists {
        conn.execute("DELETE FROM app_voice_offline_cursors WHERE line_id=?1", [line]).map_err(|_| STORAGE)?;
        conn.execute("DELETE FROM app_voice_offline_quarantine WHERE line_id=?1", [line]).map_err(|_| STORAGE)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "offline_inbox_tests.rs"]
mod tests;
