//! Private durable native PDU inbox. Commit precedes transport ACK/deletion.
//! Raw bytes and stable SIM scopes never enter public events. Promotion of a
//! complete message, scoped dedup and its application event is one transaction.
use super::{
    insert_app_event_for_conn, required_line_id, sms_timestamp_for_storage, Database, SmsMessage,
};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension, Result};

const MAX_PENDING_PER_LINE: i64 = 256;
const MAX_PENDING_TOTAL: i64 = 4096;
const RETENTION_SECONDS: i64 = 7 * 86400;

pub(super) fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS native_sms_inbox (
        id INTEGER PRIMARY KEY AUTOINCREMENT, line_id TEXT NOT NULL, sim_key TEXT NOT NULL,
        kind TEXT NOT NULL CHECK(kind IN ('deliver','report')), fingerprint TEXT NOT NULL,
        pdu TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', ack TEXT NOT NULL DEFAULT 'pending',
        summary TEXT, created_at INTEGER NOT NULL,
        UNIQUE(line_id,sim_key,kind,fingerprint));
        CREATE INDEX IF NOT EXISTS idx_native_inbox_scope ON native_sms_inbox(line_id,sim_key,state);
        CREATE TABLE IF NOT EXISTS native_sms_received (
        line_id TEXT NOT NULL, sim_key TEXT NOT NULL, dedup_key TEXT NOT NULL, sms_id INTEGER NOT NULL,
        PRIMARY KEY(line_id,sim_key,dedup_key));
        CREATE TABLE IF NOT EXISTS native_sms_sends (
        sms_id INTEGER PRIMARY KEY, line_id TEXT NOT NULL, sim_key TEXT NOT NULL,
        recipient_key TEXT NOT NULL, part_count INTEGER NOT NULL, started_at INTEGER NOT NULL,
        state TEXT NOT NULL DEFAULT 'sending');
        CREATE TABLE IF NOT EXISTS native_sms_submissions (
        sms_id INTEGER NOT NULL, part INTEGER NOT NULL, reference INTEGER NOT NULL,
        started_at INTEGER NOT NULL, finished_at INTEGER NOT NULL,
        report_status INTEGER, report_scts TEXT, PRIMARY KEY(sms_id,part));")
}

#[derive(Clone)]
pub struct InboxPdu {
    pub id: i64,
    pub kind: String,
    pub hex: String,
}
pub struct InboxMessage<'a> {
    pub ids: &'a [i64],
    pub line_id: &'a str,
    pub sim_key: &'a str,
    pub number: &'a str,
    pub text: &'a str,
    pub timestamp: &'a str,
    pub marker: &'a str,
    pub fingerprint: Option<&'a str>,
}
pub struct NativeSend<'a> {
    pub line_id: &'a str,
    pub sim_key: &'a str,
    pub recipient_key: &'a str,
    pub number: &'a str,
    pub text: &'a str,
    pub part_count: usize,
    pub started_at: i64,
}
pub struct NativeReport<'a> {
    pub id: i64,
    pub line_id: &'a str,
    pub sim_key: &'a str,
    pub recipient_key: &'a str,
    pub reference: u8,
    pub scts: &'a str,
    pub status: u8,
}
fn invalid(reason: &str) -> rusqlite::Error {
    rusqlite::Error::InvalidParameterName(reason.into())
}
fn hash_valid(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn scope_valid(line: &str, sim: &str) -> Result<()> {
    required_line_id(line)?;
    if !hash_valid(sim) {
        return Err(invalid("native_sim_scope_invalid"));
    }
    Ok(())
}

impl Database {
    pub fn stage_native_pdu(
        &self,
        line: &str,
        sim: &str,
        kind: &str,
        fingerprint: &str,
        pdu: &str,
    ) -> Result<i64> {
        scope_valid(line, sim)?;
        if !hash_valid(fingerprint)
            || !matches!(kind, "deliver" | "report")
            || pdu.is_empty()
            || pdu.len() > 1024
            || pdu.len() % 2 != 0
            || !pdu.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(invalid("native_pdu_invalid"));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        if let Some(id)=tx.query_row("SELECT id FROM native_sms_inbox WHERE line_id=?1 AND sim_key=?2 AND kind=?3 AND fingerprint=?4",params![line,sim,kind,fingerprint],|r|r.get(0)).optional()? {
            return Ok(id);
        }
        // Quarantined/incomplete messages also count. Never evict unpromoted
        // acknowledged bytes to make room for new traffic.
        let (local, total): (i64, i64) = tx.query_row(
            "SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM native_sms_inbox WHERE state!='done'",
            [line],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if local >= MAX_PENDING_PER_LINE || total >= MAX_PENDING_TOTAL {
            return Err(invalid("native_pdu_inbox_full"));
        }
        tx.execute("INSERT INTO native_sms_inbox(line_id,sim_key,kind,fingerprint,pdu,created_at)VALUES(?1,?2,?3,?4,?5,?6)",params![line,sim,kind,fingerprint,pdu.to_ascii_uppercase(),Utc::now().timestamp()])?;
        let id = tx.last_insert_rowid();
        tx.execute("DELETE FROM native_sms_inbox WHERE line_id=?1 AND state='done' AND id NOT IN (SELECT id FROM native_sms_inbox WHERE line_id=?1 AND state='done' ORDER BY id DESC LIMIT 2048)",[line])?;
        tx.commit()?;
        Ok(id)
    }

    pub fn mark_native_pdu_ack(
        &self,
        line: &str,
        sim: &str,
        fingerprint: &str,
        ack: &str,
    ) -> Result<()> {
        scope_valid(line, sim)?;
        if !matches!(
            ack,
            "confirmed"
                | "not_required"
                | "unconfirmed"
                | "ambiguous"
                | "service_unconfirmed"
                | "stored"
        ) {
            return Err(invalid("native_ack_invalid"));
        }
        self.conn.lock().unwrap().execute(
            "UPDATE native_sms_inbox SET ack=?4 WHERE line_id=?1 AND sim_key=?2 AND fingerprint=?3",
            params![line, sim, fingerprint, ack],
        )?;
        Ok(())
    }

    pub fn pending_native_pdus(&self, line: &str, sim: &str) -> Result<Vec<InboxPdu>> {
        scope_valid(line, sim)?;
        let conn = self.conn.lock().unwrap();
        let mut q=conn.prepare("SELECT id,kind,pdu FROM native_sms_inbox WHERE line_id=?1 AND sim_key=?2 AND state='pending' ORDER BY id LIMIT 256")?;
        let rows = q
            .query_map(params![line, sim], |r| {
                Ok(InboxPdu {
                    id: r.get(0)?,
                    kind: r.get(1)?,
                    hex: r.get(2)?,
                })
            })?
            .collect();
        rows
    }

    pub fn quarantine_native_pdu(&self, id: i64, line: &str, sim: &str) -> Result<()> {
        scope_valid(line, sim)?;
        self.conn.lock().unwrap().execute("UPDATE native_sms_inbox SET state='quarantined',summary='unsupported_or_invalid_pdu' WHERE id=?1 AND line_id=?2 AND sim_key=?3 AND state='pending'",params![id,line,sim])?;
        Ok(())
    }

    pub fn promote_native_sms(&self, message: InboxMessage<'_>) -> Result<Option<SmsMessage>> {
        let InboxMessage {
            ids,
            line_id,
            sim_key,
            number,
            text,
            timestamp,
            marker,
            fingerprint,
        } = message;
        scope_valid(line_id, sim_key)?;
        if ids.is_empty()
            || ids.len() > 255
            || ids
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != ids.len()
        {
            return Err(invalid("native_message_parts_invalid"));
        }
        // Reject invalid timestamps instead of generating a different identity
        // on every retry. Decoder admission already verifies SCTS.
        if super::normalize_sms_timestamp_for_display(timestamp).is_none() {
            return Err(invalid("native_message_time_invalid"));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for id in ids {
            let valid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_sms_inbox WHERE id=?1 AND line_id=?2 AND sim_key=?3 AND kind='deliver' AND state='pending')",params![id,line_id,sim_key],|r|r.get(0))?;
            if !valid {
                return Err(invalid("native_message_scope_or_state_changed"));
            }
        }
        // Do not treat an unscoped legacy claim (or another SIM's identical
        // text/SCTS) as proof of ingestion. This table is committed WITH SMS.
        let dedup_key = fingerprint.unwrap_or(marker);
        let existing:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_sms_received WHERE line_id=?1 AND sim_key=?2 AND dedup_key=?3)",params![line_id,sim_key,dedup_key],|r|r.get(0))?;
        let stamp = sms_timestamp_for_storage(timestamp);
        let mut emitted = None;
        let result = if !existing {
            tx.execute("INSERT INTO sms_messages(direction,phone_number,content,timestamp,status,pdu,transport,line_id)VALUES('incoming',?1,?2,?3,'received',?4,'modem',?5)",params![number,text,stamp,marker,line_id])?;
            let id = tx.last_insert_rowid();
            tx.execute("INSERT INTO native_sms_received(line_id,sim_key,dedup_key,sms_id)VALUES(?1,?2,?3,?4)",params![line_id,sim_key,dedup_key,id])?;
            let payload =
                serde_json::json!({"sms_id":id,"direction":"incoming","status":"received"})
                    .to_string();
            emitted = Some(insert_app_event_for_conn(
                &tx,
                "sms.received",
                Some(line_id),
                Some("modem"),
                &payload,
                &Utc::now().to_rfc3339(),
            )?);
            Some(SmsMessage {
                id,
                direction: "incoming".into(),
                phone_number: number.into(),
                content: text.into(),
                timestamp: stamp,
                status: "received".into(),
                pdu: Some(marker.into()),
                transport: "modem".into(),
                line_id: Some(line_id.into()),
            })
        } else {
            None
        };
        for id in ids {
            tx.execute(
                "UPDATE native_sms_inbox SET state='done',pdu='' WHERE id=?1",
                [id],
            )?;
        }
        tx.execute("DELETE FROM native_sms_received WHERE line_id=?1 AND sms_id NOT IN (SELECT sms_id FROM native_sms_received WHERE line_id=?1 ORDER BY sms_id DESC LIMIT 2048)",[line_id])?;
        tx.commit()?;
        if let Some(event) = emitted {
            let _ = self.app_event_tx.send(event);
        }
        Ok(result)
    }

    /// Persist the expected total BEFORE sending the first part. Cancellation
    /// or a partial send must not turn its recorded prefix into 'delivered'.
    pub fn begin_native_sms_send(&self, send: NativeSend<'_>) -> Result<i64> {
        let NativeSend {
            line_id,
            sim_key,
            recipient_key,
            number,
            text,
            part_count,
            started_at,
        } = send;
        scope_valid(line_id, sim_key)?;
        if !hash_valid(recipient_key) || !(1..=255).contains(&part_count) {
            return Err(invalid("native_submission_invalid"));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let count:i64=tx.query_row("SELECT COUNT(*) FROM native_sms_sends WHERE line_id=?1 AND state IN ('sending','unconfirmed')",[line_id],|r|r.get(0))?;
        if count >= 256 {
            return Err(invalid("native_unconfirmed_submissions_full"));
        }
        tx.execute("INSERT INTO sms_messages(direction,phone_number,content,timestamp,status,transport,line_id)VALUES('outgoing',?1,?2,?3,'pending','modem',?4)",params![number,text,Utc::now().to_rfc3339(),line_id])?;
        let id = tx.last_insert_rowid();
        tx.execute("INSERT INTO native_sms_sends(sms_id,line_id,sim_key,recipient_key,part_count,started_at)VALUES(?1,?2,?3,?4,?5,?6)",params![id,line_id,sim_key,recipient_key,part_count as i64,started_at])?;
        let payload = serde_json::json!({"sms_id":id,"status":"pending"}).to_string();
        let event = insert_app_event_for_conn(
            &tx,
            "sms.queued",
            Some(line_id),
            Some("modem"),
            &payload,
            &Utc::now().to_rfc3339(),
        )?;
        tx.commit()?;
        let _ = self.app_event_tx.send(event);
        Ok(id)
    }

    pub fn remember_native_sms_part(
        &self,
        sms_id: i64,
        part: usize,
        reference: u8,
        started: i64,
        finished: i64,
    ) -> Result<()> {
        if finished < started || finished - started > 120 {
            return Err(invalid("native_submission_interval_invalid"));
        }
        let conn = self.conn.lock().unwrap();
        let valid:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM native_sms_sends WHERE sms_id=?1 AND state='sending' AND ?2<part_count)",params![sms_id,part as i64],|r|r.get(0))?;
        if !valid {
            return Err(invalid("native_submission_state_invalid"));
        }
        conn.execute("INSERT INTO native_sms_submissions(sms_id,part,reference,started_at,finished_at)VALUES(?1,?2,?3,?4,?5)",params![sms_id,part as i64,reference,started,finished])?;
        Ok(())
    }

    pub fn finish_native_sms_send(&self, sms_id: i64, confirmed: bool) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let (line, total, state): (String, i64, String) = tx.query_row(
            "SELECT line_id,part_count,state FROM native_sms_sends WHERE sms_id=?1",
            [sms_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if state != "sending" {
            return Err(invalid("native_submission_already_finished"));
        }
        let parts: i64 = tx.query_row(
            "SELECT COUNT(*) FROM native_sms_submissions WHERE sms_id=?1",
            [sms_id],
            |r| r.get(0),
        )?;
        if confirmed && parts != total {
            return Err(invalid("native_submission_parts_incomplete"));
        }
        let state = if confirmed { "sent" } else { "unconfirmed" };
        let status = if confirmed { "sent" } else { "pending" };
        tx.execute(
            "UPDATE native_sms_sends SET state=?2 WHERE sms_id=?1",
            params![sms_id, state],
        )?;
        tx.execute(
            "UPDATE sms_messages SET status=?2 WHERE id=?1",
            params![sms_id, status],
        )?;
        let payload=serde_json::json!({"sms_id":sms_id,"status":status,"submission_state":state,"submitted_parts":parts,"part_count":total}).to_string();
        let event = insert_app_event_for_conn(
            &tx,
            if confirmed {
                "sms.sent"
            } else {
                "sms.native_submission_unconfirmed"
            },
            Some(&line),
            Some("modem"),
            &payload,
            &Utc::now().to_rfc3339(),
        )?;
        // Only retire old confirmed history; unknown submissions remain visible
        // and bounded. No automatic resend is authorized by this pruning.
        tx.execute("DELETE FROM native_sms_submissions WHERE sms_id IN (SELECT sms_id FROM native_sms_sends WHERE state IN ('sent','delivered') AND started_at<?1)",[Utc::now().timestamp()-RETENTION_SECONDS])?;
        tx.execute(
            "DELETE FROM native_sms_sends WHERE state IN ('sent','delivered') AND started_at<?1",
            [Utc::now().timestamp() - RETENTION_SECONDS],
        )?;
        tx.commit()?;
        let _ = self.app_event_tx.send(event);
        Ok(())
    }

    pub fn consume_native_report(&self, report: NativeReport<'_>) -> Result<Option<i64>> {
        let NativeReport {
            id,
            line_id: line,
            sim_key: sim,
            recipient_key: recipient,
            reference,
            scts,
            status,
        } = report;
        scope_valid(line, sim)?;
        if !hash_valid(recipient) {
            return Err(invalid("native_report_recipient_invalid"));
        }
        let report_time = chrono::DateTime::parse_from_rfc3339(scts)
            .map_err(|_| invalid("native_report_time_invalid"))?
            .timestamp();
        let now = Utc::now().timestamp();
        if report_time > now + 60 || report_time < now - RETENTION_SECONDS {
            self.quarantine_native_pdu(id, line, sim)?;
            return Ok(None);
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let (created,previous):(i64,Option<String>)=tx.query_row("SELECT created_at,summary FROM native_sms_inbox WHERE id=?1 AND line_id=?2 AND sim_key=?3 AND kind='report' AND state='pending'",params![id,line,sim],|r|Ok((r.get(0)?,r.get(1)?)))?;
        // TP-SCTS is SC acceptance time, NOT discharge time. Its narrow window
        // must overlap this actual submission. Never match 'any MR in a day'.
        let candidates = {
            let mut q=tx.prepare("SELECT p.sms_id,p.part,s.state,s.part_count FROM native_sms_submissions p JOIN native_sms_sends s ON s.sms_id=p.sms_id WHERE s.line_id=?1 AND s.sim_key=?2 AND s.recipient_key=?3 AND p.reference=?4 AND ?5 BETWEEN p.started_at-2 AND p.finished_at+120")?;
            let rows = q
                .query_map(params![line, sim, recipient, reference, report_time], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>>>()?;
            rows
        };
        let uncertain:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM native_sms_sends WHERE line_id=?1 AND sim_key=?2 AND recipient_key=?3 AND state IN ('sending','unconfirmed') AND ?4 BETWEEN started_at-2 AND started_at+300)",params![line,sim,recipient,report_time],|r|r.get(0))?;
        let mut delivered = None;
        let mut matched = false;
        if !uncertain {
            if let [(sms_id, part, state, total)] = candidates.as_slice() {
                if matches!(state.as_str(), "sent" | "delivered") {
                    matched = true;
                    tx.execute("UPDATE native_sms_submissions SET report_status=?3,report_scts=?4 WHERE sms_id=?1 AND part=?2 AND (report_status IS NULL OR report_status!=0)",params![sms_id,part,status,scts])?;
                    let successful:i64=tx.query_row("SELECT COUNT(*) FROM native_sms_submissions WHERE sms_id=?1 AND report_status=0",[sms_id],|r|r.get(0))?;
                    if successful == *total && status == 0 {
                        let changed=tx.execute("UPDATE sms_messages SET status='delivered' WHERE id=?1 AND line_id=?2 AND direction='outgoing' AND transport='modem' AND status='sent'",params![sms_id,line])?;
                        tx.execute(
                            "UPDATE native_sms_sends SET state='delivered' WHERE sms_id=?1",
                            [sms_id],
                        )?;
                        if changed > 0 {
                            delivered = Some(*sms_id);
                        }
                    }
                }
            }
        }
        let summary=serde_json::json!({"reference":reference,"status":status,"matches":candidates.len(),"correlated":matched,"uncertain_submission":uncertain,"delivered_sms_id":delivered}).to_string();
        // A report can precede its sending task's DB commit. Unmatched reports
        // keep their bytes and are reconsidered; ambiguity never means success.
        if matched {
            tx.execute(
                "UPDATE native_sms_inbox SET state='done',pdu='',summary=?2 WHERE id=?1",
                params![id, summary],
            )?;
        } else {
            tx.execute(
                "UPDATE native_sms_inbox SET state=?2,summary=?3 WHERE id=?1",
                params![
                    id,
                    if now - created > RETENTION_SECONDS {
                        "quarantined"
                    } else {
                        "pending"
                    },
                    summary
                ],
            )?;
        }
        let event = if previous.as_deref() != Some(summary.as_str()) {
            Some(insert_app_event_for_conn(
                &tx,
                "sms.native_status_report",
                Some(line),
                Some("modem"),
                &summary,
                &Utc::now().to_rfc3339(),
            )?)
        } else {
            None
        };
        tx.commit()?;
        if let Some(event) = event {
            let _ = self.app_event_tx.send(event);
        }
        Ok(delivered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn db() -> Database {
        Database::new(":memory:".into()).unwrap()
    }
    fn scope() -> String {
        "a".repeat(64)
    }
    fn hash(n: usize) -> String {
        format!("{n:064x}")
    }
    fn stage(db: &Database, n: usize, sim: &str, kind: &str) -> i64 {
        db.stage_native_pdu("line", sim, kind, &hash(n), "0000112233")
            .unwrap()
    }
    fn promote(db: &Database, id: i64, sim: &str) -> Result<Option<SmsMessage>> {
        db.promote_native_sms(InboxMessage {
            ids: &[id],
            line_id: "line",
            sim_key: sim,
            number: "1234",
            text: "fixture",
            timestamp: "2026-01-01T00:00:00Z",
            marker: "fixture-marker",
            fingerprint: Some("same-content-fingerprint"),
        })
    }
    fn start(db: &Database, total: usize, now: i64) -> i64 {
        db.begin_native_sms_send(NativeSend {
            line_id: "line",
            sim_key: &scope(),
            recipient_key: &hash(9),
            number: "1234",
            text: "fixture",
            part_count: total,
            started_at: now,
        })
        .unwrap()
    }
    fn report(db: &Database, id: i64, mr: u8, now: i64, status: u8) -> Option<i64> {
        db.consume_native_report(NativeReport {
            id,
            line_id: "line",
            sim_key: &scope(),
            recipient_key: &hash(9),
            reference: mr,
            scts: &chrono::DateTime::from_timestamp(now, 0)
                .unwrap()
                .to_rfc3339(),
            status,
        })
        .unwrap()
    }
    #[test]
    fn durable_inbox_and_final_dedup_are_sim_scoped() {
        let db = db();
        let sim = scope();
        let other = "b".repeat(64);
        let id = stage(&db, 1, &sim, "deliver");
        assert_eq!(id, stage(&db, 1, &sim, "deliver"));
        assert!(promote(&db, id, &other).is_err());
        assert!(promote(&db, id, &sim).unwrap().is_some());
        let repeated = stage(&db, 2, &sim, "deliver");
        assert!(promote(&db, repeated, &sim).unwrap().is_none());
        let another = stage(&db, 1, &other, "deliver");
        assert!(promote(&db, another, &other).unwrap().is_some());
        assert!(db.pending_native_pdus("line", &sim).unwrap().is_empty());
    }
    #[test]
    fn event_failure_rolls_back_message_dedup_and_consumption_together() {
        let db = db();
        let id = stage(&db, 1, &scope(), "deliver");
        db.conn.lock().unwrap().execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON app_events BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(promote(&db, id, &scope()).is_err());
        assert_eq!(db.pending_native_pdus("line", &scope()).unwrap().len(), 1);
        db.conn
            .lock()
            .unwrap()
            .execute_batch("DROP TRIGGER fail_event;")
            .unwrap();
        assert!(promote(&db, id, &scope()).unwrap().is_some());
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sms_messages", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let payload: String = conn
            .query_row("SELECT payload_json FROM app_events", [], |r| r.get(0))
            .unwrap();
        assert!(
            !payload.contains("1234")
                && !payload.contains("fixture")
                && !payload.contains(&scope())
        );
    }
    #[test]
    fn quarantine_and_old_sim_rows_count_toward_the_bound() {
        let db = db();
        for n in 0..256 {
            let id = stage(&db, n, &scope(), "deliver");
            db.quarantine_native_pdu(id, "line", &scope()).unwrap();
        }
        assert!(db
            .stage_native_pdu("line", &"b".repeat(64), "deliver", &hash(300), "00")
            .is_err());
        assert_eq!(stage(&db, 1, &scope(), "deliver"), 2);
        assert!(db
            .stage_native_pdu("line", &"x".repeat(64), "deliver", &hash(301), "00")
            .is_err());
    }
    #[test]
    fn early_report_replays_after_submission_commits() {
        let db = db();
        let now = Utc::now().timestamp();
        let id = stage(&db, 1, &scope(), "report");
        assert_eq!(report(&db, id, 7, now, 0), None);
        assert_eq!(db.pending_native_pdus("line", &scope()).unwrap().len(), 1);
        let sms = start(&db, 1, now);
        db.remember_native_sms_part(sms, 0, 7, now, now).unwrap();
        db.finish_native_sms_send(sms, true).unwrap();
        assert_eq!(report(&db, id, 7, now, 0), Some(sms));
        assert!(db.pending_native_pdus("line", &scope()).unwrap().is_empty());
    }
    #[test]
    fn every_part_requires_affirmative_delivery_without_status_regression() {
        let db = db();
        let now = Utc::now().timestamp();
        let sms = start(&db, 2, now);
        db.remember_native_sms_part(sms, 0, 7, now, now).unwrap();
        assert!(db.finish_native_sms_send(sms, true).is_err());
        db.remember_native_sms_part(sms, 1, 8, now, now).unwrap();
        db.finish_native_sms_send(sms, true).unwrap();
        assert_eq!(
            report(&db, stage(&db, 1, &scope(), "report"), 7, now, 0),
            None
        );
        assert_eq!(
            report(&db, stage(&db, 2, &scope(), "report"), 8, now, 1),
            None
        );
        assert_eq!(
            report(&db, stage(&db, 3, &scope(), "report"), 7, now, 64),
            None
        );
        assert_eq!(
            report(&db, stage(&db, 4, &scope(), "report"), 8, now, 0),
            Some(sms)
        );
    }
    #[test]
    fn mr_reuse_old_scts_and_partial_sends_never_guess_delivery() {
        let db = db();
        let now = Utc::now().timestamp();
        let first = start(&db, 1, now);
        db.remember_native_sms_part(first, 0, 7, now, now).unwrap();
        db.finish_native_sms_send(first, true).unwrap();
        assert_eq!(
            report(&db, stage(&db, 1, &scope(), "report"), 7, now - 3600, 0),
            None
        );
        let second = start(&db, 2, now);
        db.remember_native_sms_part(second, 0, 7, now, now).unwrap();
        db.finish_native_sms_send(second, false).unwrap();
        assert_eq!(
            report(&db, stage(&db, 2, &scope(), "report"), 7, now, 0),
            None
        );
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sms_messages WHERE status='delivered'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn staging_survives_database_reopen_before_ack_or_promotion() {
        let path = std::env::temp_dir().join(format!(
            "native-inbox-{}-{}.db",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap()
        ));
        let id = {
            let db = Database::new(path.clone()).unwrap();
            stage(&db, 1, &scope(), "deliver")
        };
        let db = Database::new(path.clone()).unwrap();
        assert_eq!(db.pending_native_pdus("line", &scope()).unwrap()[0].id, id);
        assert!(promote(&db, id, &scope()).unwrap().is_some());
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
}
