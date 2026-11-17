//! Independently consented original Group READ observation of a recorded user
//! decision. A reported position is not its matched WRITE result or no-IO proof.
//! No receipt/cursor/message/event is mutated and no write permit is minted.
use super::*;
use super::super::{im_data::ImBox, im_status::StatusLabel, rpc_im::{ImQuery, ImReplyData, OwnedImReply}};
const MISSING: &str = "app_account_im_reconciliation_not_found";
const CONFLICT: &str = "app_account_im_reconciliation_context_conflict";

#[derive(Serialize)]
pub struct ImReadObservation {
    pub receipt_id: i64,
    pub message_id: i64,
    pub target_index: String,
    pub local_state: &'static str,
    pub report: &'static str,
    pub reported_last_read: Option<String>,
    pub reported_last_received: Option<String>,
    pub reported_latest: Option<String>,
    pub server_update_time: Option<String>,
    pub observed_at: String,
}
impl fmt::Debug for ImReadObservation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ImReadObservation(<redacted>)") }
}
fn state_name(state: i64) -> Result<&'static str, &'static str> {
    match state { 0 => Ok("uncertain"), 1 => Ok("confirmed"), 2 => Ok("rejected"), 3 => Ok("not_sent"), _ => Err(STORAGE) }
}

pub(in super::super) struct ImReconciliationAction {
    database: Arc<Database>,
    run: Arc<RunToken>,
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    parent_gate: Arc<ImReceiptGate>,
    gate: Arc<ImReceiptGate>,
    receipt_id: i64,
    deadline: Instant,
}
impl fmt::Debug for ImReconciliationAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ImReconciliationAction(<redacted>)") }
}
impl ImReconciliationAction {
    pub(in super::super) fn original(database: Arc<Database>, run: Arc<RunToken>, owner: AttemptOwner,
        scope: RpcConnectionScope, parent_gate: Arc<ImReceiptGate>, receipt_id: i64, deadline: Instant)
        -> Result<Arc<Self>, &'static str> {
        if receipt_id <= 0 || owner.line_id() != run.line_id() || !scope.is_open() || scope.session_revoked()
            || !parent_gate.live() || Instant::now() >= deadline { return Err(SCOPE); }
        Ok(Arc::new(Self { database, run, owner, scope, parent_gate, gate: ImReceiptGate::fresh(), receipt_id, deadline }))
    }
    pub(in super::super) fn owner(&self) -> &AttemptOwner { &self.owner }
    pub(in super::super) fn scope(&self) -> RpcConnectionScope { self.scope.clone() }
    pub(in super::super) fn gate(&self) -> Arc<ImReceiptGate> { self.gate.clone() }
    pub(in super::super) fn parent_gate(&self) -> Arc<ImReceiptGate> { self.parent_gate.clone() }
    pub(in super::super) fn deadline(&self) -> Instant { self.deadline }
    pub(in super::super) fn live(&self) -> bool {
        self.parent_gate.live() && self.gate.live() && self.scope.is_open() && !self.scope.session_revoked() && Instant::now() < self.deadline
    }
    pub(in super::super) fn retire(&self) { self.gate.retire(); }
    pub(in super::super) fn bind_current(self: &Arc<Self>, binding: Arc<ImBinding>) -> Result<ImReconciliationContext, &'static str> {
        if !self.live() || !binding.matches(&self.owner, &self.scope) { return Err(SCOPE); }
        let context = ImInboxScope::original(self.run.clone(), binding, self.parent_gate.clone())?;
        let record = self.database.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred).map_err(|_| STORAGE)?;
            context.validate(&tx)?;
            let record = record(&tx, self, &context)?;
            if !self.live() { return Err(SCOPE); }
            tx.commit().map_err(|_| STORAGE)?;
            Ok(record)
        })?;
        let label = StatusLabel::parse(&record.box_id[4..]).map_err(|_| CONFLICT)?;
        let query = Arc::new(ImQuery::refresh(&[label], -1).map_err(|_| CONFLICT)?);
        Ok(ImReconciliationContext { action: self.clone(), context, record, query })
    }
}
struct Record { message_id: i64, index: i64, box_id: Zeroizing<Vec<u8>>, fingerprint: [u8; 32], state: i64 }
fn record(conn: &Connection, action: &ImReconciliationAction, context: &ImInboxScope) -> Result<Record, &'static str> {
    let row: Option<(i64, i64, Vec<u8>, Vec<u8>, i64)> = conn.query_row(
        "SELECT message_id,target_index,box_id,fingerprint,state FROM app_voice_im_user_reads WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4",
        params![action.receipt_id, action.run.line_id(), action.run.revision() as i64, context.binding.identity().as_slice()],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional().map_err(|_| STORAGE)?;
    let (message_id, index, box_id, fingerprint, state) = row.ok_or(MISSING)?;
    if message_id <= 0 || index < 0 || ImBox::parse(&box_id).is_err() { return Err(CONFLICT); }
    state_name(state)?;
    let fingerprint: [u8; 32] = fingerprint.try_into().map_err(|_| STORAGE)?;
    // A normal deleted tombstone retains the same body fingerprint; never
    // restore text or use its existence to confirm the unknown native WRITE.
    let present: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_voice_im_messages WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4 AND box_id=?5 AND message_index=?6 AND fingerprint=?7)",
        params![message_id, action.run.line_id(), action.run.revision() as i64, context.binding.identity().as_slice(), box_id.as_slice(), index, fingerprint.as_slice()],
        |r| r.get(0)).map_err(|_| STORAGE)?;
    if !present { return Err(CONFLICT); }
    Ok(Record { message_id, index, box_id: Zeroizing::new(box_id), fingerprint, state })
}
pub(in super::super) struct ImReconciliationContext {
    action: Arc<ImReconciliationAction>, context: ImInboxScope, record: Record, query: Arc<ImQuery>,
}
impl fmt::Debug for ImReconciliationContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("ImReconciliationContext(<redacted>)") }
}
impl ImReconciliationContext {
    pub(in super::super) fn query(&self) -> Arc<ImQuery> { self.query.clone() }
    pub(in super::super) fn observe(&self, reply: &OwnedImReply) -> Result<ImReadObservation, &'static str> {
        if !self.action.live() || !reply.matches(self.action.owner(), &self.action.scope())
            || !Arc::ptr_eq(reply.request().binding(), &self.context.binding)
            || !Arc::ptr_eq(reply.request().query_arc(), &self.query)
            || !Arc::ptr_eq(&reply.request().gate(), &self.action.gate()) { return Err(SCOPE); }
        let ImReplyData::Refresh(status) = reply.result().map_err(|_| "app_account_im_reconciliation_reply_invalid")? else { return Err(SCOPE); };
        let entry = status.accepted.then(|| status.entries.iter().find(|e| e.label.bytes() == &self.record.box_id[4..])).flatten();
        let current_state = self.action.database.with_conn(|conn| {
            let tx = Transaction::new_unchecked(conn, TransactionBehavior::Deferred).map_err(|_| STORAGE)?;
            self.context.validate(&tx)?;
            let current = record(&tx, &self.action, &self.context)?;
            if current.message_id != self.record.message_id || current.index != self.record.index || current.box_id.as_slice() != self.record.box_id.as_slice()
                || current.fingerprint != self.record.fingerprint { return Err(CONFLICT); }
            if !self.action.live() { return Err(SCOPE); }
            tx.commit().map_err(|_| STORAGE)?;
            state_name(current.state)
        })?;
        if !self.action.live() { return Err(SCOPE); }
        Ok(ImReadObservation {
            receipt_id: self.action.receipt_id, message_id: self.record.message_id, target_index: self.record.index.to_string(), local_state: current_state,
            report: if !status.accepted { "rejected" } else if entry.is_some() { "reported" } else { "unreported" },
            reported_last_read: entry.and_then(|e| e.last_read).map(|n| n.to_string()),
            reported_last_received: entry.and_then(|e| e.last_received).map(|n| n.to_string()),
            reported_latest: entry.and_then(|e| e.latest).map(|n| n.to_string()),
            server_update_time: entry.map(|_| status.update_time.to_string()), observed_at: chrono::Utc::now().to_rfc3339(),
        })
    }
}
pub(in super::super) struct ImReconciliationWait { action: Arc<ImReconciliationAction> }
impl ImReconciliationWait { pub(in super::super) fn original(action: Arc<ImReconciliationAction>) -> Self { Self { action } } }
impl Drop for ImReconciliationWait { fn drop(&mut self) { self.action.retire(); } }
