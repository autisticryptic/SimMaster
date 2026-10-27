//! Same-run batch replay tombstones, based on original result-bearing ACKs.
//! No payload eviction, invented remote ID, queued-result or DIRECT shortcut.
use super::{account_runs, account_store, batch_scheduler::BatchReplayRetirement,
    rpc_pending::RpcConnectionScope, session::AttemptOwner};
use crate::platform::db::Database;
use rusqlite::{params, Connection, Transaction, TransactionBehavior};

const STORAGE: &str = "app_account_batch_retention_storage_failed";
const SCOPE: &str = "app_account_batch_retention_scope_invalid";
const MAX_SPARSE_PER_ACCOUNT: i64 = 2048;
const MAX_SPARSE_TOTAL: i64 = 8192;

pub(super) fn initialize_schema(conn: &Connection) -> Result<(), &'static str> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_batch_replay_fences(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL,run_nonce TEXT NOT NULL,
        peer_epoch INTEGER NOT NULL CHECK(peer_epoch>0 AND peer_epoch<=4294967295),
        through_sequence INTEGER NOT NULL CHECK(through_sequence>=0 AND through_sequence<=2147483647),
        PRIMARY KEY(line_id,revision,run_nonce,peer_epoch));
        CREATE TABLE IF NOT EXISTS app_voice_batch_replay_ids(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL,run_nonce TEXT NOT NULL,
        peer_epoch INTEGER NOT NULL CHECK(peer_epoch>0 AND peer_epoch<=4294967295),
        sequence INTEGER NOT NULL CHECK(sequence>0 AND sequence<=2147483647),
        PRIMARY KEY(line_id,revision,run_nonce,peer_epoch,sequence));")
        .map_err(|_| STORAGE)
}

pub(super) fn retired(conn: &Connection, line: &str, revision: u64, nonce: &str,
    epoch: u32, sequence: i32) -> Result<bool, &'static str> {
    conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_batch_replay_fences
        WHERE line_id=?1 AND revision=?2 AND run_nonce=?3 AND peer_epoch=?4 AND through_sequence>=?5)
        OR EXISTS(SELECT 1 FROM app_voice_batch_replay_ids
        WHERE line_id=?1 AND revision=?2 AND run_nonce=?3 AND peer_epoch=?4 AND sequence=?5)",
        params![line,revision as i64,nonce,i64::from(epoch),sequence], |row| row.get(0)).map_err(|_| STORAGE)
}

pub(super) fn apply(database: &Database, line: &str, revision: u64, nonce: &str,
    owner: &AttemptOwner, scope: &RpcConnectionScope, proof: &BatchReplayRetirement) -> Result<(), &'static str> {
    if owner.line_id()!=line || !proof.matches(owner,scope) { return Err(SCOPE); }
    database.with_conn(|conn| {
        let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_| STORAGE)?;
        let account=account_store::metadata(&tx,line)?.ok_or(SCOPE)?;
        if account.revision!=revision { return Err(SCOPE); }
        account_runs::require_live(&tx,line,revision,nonce)?;
        let held:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_runtime_leases
            WHERE line_id=?1 AND revision=?2 AND run_nonce=?3)",params![line,revision as i64,nonce],|row|row.get(0)).map_err(|_| STORAGE)?;
        if !held { return Err(SCOPE); }
        let epoch=i64::from(proof.peer_incarnation());
        tx.execute("INSERT INTO app_voice_batch_replay_fences(line_id,revision,run_nonce,peer_epoch,through_sequence)
            VALUES(?1,?2,?3,?4,?5) ON CONFLICT(line_id,revision,run_nonce,peer_epoch) DO UPDATE
            SET through_sequence=MAX(through_sequence,excluded.through_sequence)",
            params![line,revision as i64,nonce,epoch,proof.through()]).map_err(|_| STORAGE)?;
        let through:i32=tx.query_row("SELECT through_sequence FROM app_voice_batch_replay_fences
            WHERE line_id=?1 AND revision=?2 AND run_nonce=?3 AND peer_epoch=?4",params![line,revision as i64,nonce,epoch],|row|row.get(0)).map_err(|_| STORAGE)?;
        // Only prefix-proven tombstones can be removed. A blocked low request
        // may leave sparse ACKed IDs below the moving native ring; preserve them.
        tx.execute("DELETE FROM app_voice_batch_replay_ids WHERE line_id=?1 AND revision=?2 AND run_nonce=?3
            AND peer_epoch=?4 AND sequence<=?5",params![line,revision as i64,nonce,epoch,through]).map_err(|_| STORAGE)?;
        for &sequence in proof.acknowledged() {
            if sequence<=through { continue; }
            // ACKs for calls/rejected/unknown payloads do not classify or erase
            // them. A classified durable consumer result must already exist.
            tx.execute("INSERT OR IGNORE INTO app_voice_batch_replay_ids(line_id,revision,run_nonce,peer_epoch,sequence)
                SELECT i.line_id,i.revision,i.run_nonce,i.peer_epoch,i.sequence FROM app_voice_inbox i
                WHERE i.line_id=?1 AND i.revision=?2 AND i.run_nonce=?3 AND i.peer_epoch=?4 AND i.sequence=?5
                    AND i.source_kind='batch' AND i.reply_state='batch_queued'
                    AND EXISTS(SELECT 1 FROM app_voice_sms_projection p
                        WHERE p.inbox_id=i.id AND p.line_id=i.line_id AND p.revision=i.revision)",
                params![line,revision as i64,nonce,epoch,sequence]).map_err(|_| STORAGE)?;
        }
        let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_batch_replay_ids",
            [line],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_| STORAGE)?;
        if local>MAX_SPARSE_PER_ACCOUNT || total>MAX_SPARSE_TOTAL { return Err("app_account_batch_retention_full"); }
        tx.execute("UPDATE app_voice_sms_projection SET envelope_retained=0 WHERE line_id=?1 AND revision=?2 AND inbox_id IN
            (SELECT i.id FROM app_voice_inbox i WHERE i.line_id=?1 AND i.revision=?2 AND i.run_nonce=?3 AND i.peer_epoch=?4
                AND i.source_kind='batch' AND i.reply_state='batch_queued'
                AND (i.sequence<=?5 OR EXISTS(SELECT 1 FROM app_voice_batch_replay_ids r
                    WHERE r.line_id=i.line_id AND r.revision=i.revision AND r.run_nonce=i.run_nonce AND r.peer_epoch=i.peer_epoch AND r.sequence=i.sequence))
                AND EXISTS(SELECT 1 FROM app_voice_sms_projection p WHERE p.inbox_id=i.id AND p.line_id=i.line_id AND p.revision=i.revision)
                ORDER BY i.id DESC LIMIT -1 OFFSET 128)",params![line,revision as i64,nonce,epoch,through]).map_err(|_| STORAGE)?;
        tx.execute("DELETE FROM app_voice_inbox WHERE line_id=?1 AND revision=?2 AND run_nonce=?3 AND peer_epoch=?4
            AND source_kind='batch' AND reply_state='batch_queued'
            AND (sequence<=?5 OR EXISTS(SELECT 1 FROM app_voice_batch_replay_ids r
                WHERE r.line_id=app_voice_inbox.line_id AND r.revision=app_voice_inbox.revision AND r.run_nonce=app_voice_inbox.run_nonce
                    AND r.peer_epoch=app_voice_inbox.peer_epoch AND r.sequence=app_voice_inbox.sequence))
            AND EXISTS(SELECT 1 FROM app_voice_sms_projection p WHERE p.inbox_id=app_voice_inbox.id
                AND p.line_id=app_voice_inbox.line_id AND p.revision=app_voice_inbox.revision AND p.envelope_retained=0)",
            params![line,revision as i64,nonce,epoch,through]).map_err(|_| STORAGE)?;
        tx.execute("DELETE FROM app_voice_sms_projection WHERE line_id=?1 AND revision=?2 AND envelope_retained=0 AND deleted=1",
            params![line,revision as i64]).map_err(|_| STORAGE)?;
        tx.commit().map_err(|_| STORAGE)
    })
}

pub(super) fn delete_run(conn: &Connection, line: &str, revision: u64, nonce: &str) -> Result<(), &'static str> {
    for table in ["app_voice_batch_replay_ids","app_voice_batch_replay_fences"] {
        conn.execute(&format!("DELETE FROM {table} WHERE line_id=?1 AND revision=?2 AND run_nonce=?3"),
            params![line,revision as i64,nonce]).map_err(|_| STORAGE)?;
    }
    Ok(())
}
pub(super) fn delete_account(conn: &Connection, line: &str) -> Result<(), &'static str> {
    for table in ["app_voice_batch_replay_ids","app_voice_batch_replay_fences"] {
        conn.execute(&format!("DELETE FROM {table} WHERE line_id=?1"),[line]).map_err(|_| STORAGE)?;
    }
    Ok(())
}

#[cfg(test)]
#[path="batch_retention_tests.rs"]
mod tests;
