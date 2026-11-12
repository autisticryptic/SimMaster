//! 原 libmtc 的离线积压协议；不是授信、SMS 分类或新的网络通道。
//! 5B898C/1AFA10: [version 0], UID, selector, signed position64, limit32=100.
//! 5B57F8/E67B04: bool, count32 * (position64, raw blob), position64, footer.
//! 58EB58/4A6384: 每个 blob 是独立 type-0 的 kind/map/body，不重复方法版本。
//! type-2 构造器虽解析 JSON，但顺序 string/blob 槽 E77EBC/E77E1C 均抛
//! not-support；本子集保留未知原包，不猜 JSON 字段或推进其消费游标。
use super::{arc_protocol::DirectReplyTicket, batch_scheduler::ServerRequestTicket,
    rpc_pending::RpcConnectionScope, rpc_stream::{read_footer, CodecError, Reader, RpcBytes, RpcFooter, RpcProperty, Writer},
    session::AttemptOwner};
use ring::digest::{Context, SHA256};
use std::fmt;
use zeroize::Zeroizing;

pub const MORE_OFFLINE_METHOD: &str = "moreOfflineMessages.MessageReceiver.Client";
pub const GET_OFFLINE_METHOD: &str = "getOfflineMessages.AccountEntry.Account";
pub const OFFLINE_LIMIT: usize = 100;

pub fn encode_get_offline(user: &[u8], selector: &[u8], position: i64) -> Result<RpcBytes, CodecError> {
    if user.is_empty() || selector.is_empty() || position < -1 || position == i64::MAX { return Err(CodecError::InvalidField); }
    let mut writer = Writer::versioned()?;
    writer.bytes(user)?; writer.bytes(selector)?; writer.i64(position)?; writer.u32(OFFLINE_LIMIT as u32)?;
    Ok(writer.finish())
}

pub fn decode_more_offline(arguments: &[u8]) -> Result<i64, CodecError> {
    let mut reader = Reader::versioned(arguments)?;
    let position = reader.i64()?; reader.finish()?;
    if position < -1 || position == i64::MAX { return Err(CodecError::InvalidField); }
    Ok(position)
}

pub struct OfflineRecord<'a> { pub position: i64, pub envelope: &'a [u8] }
impl fmt::Debug for OfflineRecord<'_> { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OfflineRecord(<redacted>)") } }
pub struct OfflineResult<'a> { pub records: Vec<OfflineRecord<'a>>, pub server_position: i64, pub footer: RpcFooter<'a> }
impl fmt::Debug for OfflineResult<'_> { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OfflineResult(<redacted>)") } }

pub fn decode_get_offline(code: u32, bytes: &[u8]) -> Result<OfflineResult<'_>, CodecError> {
    if code != 0 { return Err(CodecError::RemoteFailure); }
    let mut reader = Reader::new(bytes)?;
    let accepted = reader.boolean()?;
    let count = reader.count(12)?;
    if count > OFFLINE_LIMIT { return Err(CodecError::TooLarge); }
    let mut records: Vec<OfflineRecord<'_>> = Vec::with_capacity(count);
    for _ in 0..count {
        let position = reader.i64()?;
        if position < 0 || position == i64::MAX { return Err(CodecError::InvalidField); }
        let envelope = reader.bytes()?;
        if envelope.is_empty() { return Err(CodecError::InvalidField); }
        if records.iter().any(|record| record.position == position) { return Err(CodecError::DuplicateKey); }
        records.push(OfflineRecord { position, envelope });
    }
    let server_position = reader.i64()?;
    if server_position < -1 || server_position == i64::MAX || records.iter().any(|record| record.position > server_position) {
        return Err(CodecError::InvalidField);
    }
    let footer = read_footer(&mut reader)?; reader.finish()?;
    if !accepted || !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
    records.sort_by_key(|record| record.position); // 对应原有序 position→blob 集合。
    Ok(OfflineResult { records, server_position, footer })
}

pub struct OfflineMessage<'a> { pub receiver_kind: &'a [u8], pub properties: Vec<RpcProperty<'a>>, pub body: &'a [u8] }
impl fmt::Debug for OfflineMessage<'_> { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OfflineMessage(<redacted>)") } }
pub fn decode_offline_message(bytes: &[u8]) -> Result<OfflineMessage<'_>, CodecError> {
    let mut reader = Reader::new(bytes)?;
    let receiver_kind = reader.bytes()?;
    if receiver_kind.is_empty() { return Err(CodecError::InvalidField); }
    let properties = reader.properties()?; let body = reader.bytes()?; reader.finish()?;
    Ok(OfflineMessage { receiver_kind, properties, body })
}

/// 只有匹配的原生 RPC 回复能创建本页；原包保留到 SQLite 消费结束。
/// 身份摘要不含 phone/ARC token，且不向 HTTP 暴露或授权下一条连接。
pub struct OwnedOfflinePage {
    owner: AttemptOwner, scope: RpcConnectionScope, call_id: i32, requested_position: i64,
    identity_key: String, bytes: Zeroizing<Vec<u8>>,
}
impl fmt::Debug for OwnedOfflinePage { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("OwnedOfflinePage(<redacted>)") } }
impl OwnedOfflinePage {
    pub(super) fn original(owner: AttemptOwner, scope: RpcConnectionScope, call_id: i32, position: i64,
        user: &[u8], selector: &[u8], account_identity: Option<&[u8]>, bytes: &[u8]) -> Result<Self, CodecError> {
        decode_get_offline(0, bytes)?;
        if !scope.is_open() || call_id <= 0 || position < -1 || position == i64::MAX { return Err(CodecError::InvalidField); }
        let mut digest = Context::new(&SHA256);
        for field in [Some(user), Some(selector), account_identity] {
            digest.update(&[u8::from(field.is_some())]); let value = field.unwrap_or_default();
            digest.update(&(value.len() as u64).to_be_bytes()); digest.update(value);
        }
        let identity_key = digest.finish().as_ref().iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self { owner, scope, call_id, requested_position: position, identity_key, bytes: Zeroizing::new(bytes.to_vec()) })
    }
    pub(super) fn owner(&self) -> &AttemptOwner { &self.owner }
    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && scope.is_open()
    }
    pub(super) fn call_id(&self) -> i32 { self.call_id }
    pub(super) fn requested_position(&self) -> i64 { self.requested_position }
    pub(super) fn identity_key(&self) -> &str { &self.identity_key }
    pub(super) fn raw(&self) -> &[u8] { &self.bytes }
    pub(super) fn result(&self) -> Result<OfflineResult<'_>, CodecError> { decode_get_offline(0, &self.bytes) }
}

pub(super) enum OfflineHintTicket { Direct(DirectReplyTicket), Batch(ServerRequestTicket) }
pub struct IncomingOfflineHint { owner: AttemptOwner, scope: RpcConnectionScope, position: i64, ticket: OfflineHintTicket }
impl fmt::Debug for IncomingOfflineHint { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("IncomingOfflineHint(<redacted>)") } }
impl IncomingOfflineHint {
    pub(super) fn direct(owner: AttemptOwner, scope: RpcConnectionScope, position: i64, ticket: DirectReplyTicket) -> Self {
        Self { owner, scope, position, ticket: OfflineHintTicket::Direct(ticket) }
    }
    pub(super) fn batch(owner: AttemptOwner, scope: RpcConnectionScope, position: i64, ticket: ServerRequestTicket) -> Self {
        Self { owner, scope, position, ticket: OfflineHintTicket::Batch(ticket) }
    }
    pub(super) fn owner(&self) -> &AttemptOwner { &self.owner }
    pub(super) fn position(&self) -> i64 { self.position }
    pub(super) fn ticket(&self) -> &OfflineHintTicket { &self.ticket }
    pub(super) fn captured_copy(&self) -> Self {
        let ticket = match &self.ticket {
            OfflineHintTicket::Direct(ticket) => OfflineHintTicket::Direct(ticket.captured_copy()),
            OfflineHintTicket::Batch(ticket) => OfflineHintTicket::Batch(ticket.captured_copy()),
        };
        Self { owner: self.owner.clone(), scope: self.scope.clone(), position: self.position, ticket }
    }
    pub(super) fn matches(&self, owner: &AttemptOwner, scope: &RpcConnectionScope) -> bool {
        &self.owner == owner && self.scope.matches(scope) && scope.is_open() && match &self.ticket {
            OfflineHintTicket::Direct(ticket) => ticket.matches(owner, scope),
            OfflineHintTicket::Batch(ticket) => ticket.matches(owner, scope),
        }
    }
}

#[cfg(test)]
#[path = "rpc_offline_tests.rs"]
mod tests;
