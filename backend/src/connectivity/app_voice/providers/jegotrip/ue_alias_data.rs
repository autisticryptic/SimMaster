//! Private DATA returned by getRelations.User.User. Neither a roster nor a WRITE grant.
use super::{native_rpc_budget::NativeRpcReservation, rpc_pending::RpcConnectionScope,
    rpc_stream::{compare_keys, read_footer, CodecError, Reader, RpcProperty, MAX_MAP_ENTRIES}};
use std::{fmt, mem::size_of, sync::Arc};
use zeroize::Zeroizing;

struct Row { kind: Zeroizing<Vec<u8>>, value: Zeroizing<Vec<u8>>, preemptible: bool }
pub(super) struct AliasRows { rows: Vec<Row>, _charge: Arc<NativeRpcReservation> }
impl fmt::Debug for AliasRows { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("AliasRows(<redacted>)") } }
impl AliasRows {
    // Reserve before the model, string or footer allocations, including both
    // old and new snapshots while a replacement is staged. No retained footer.
    pub(super) fn decode(scope: &RpcConnectionScope, code: u32, bytes: &[u8]) -> Result<Self, CodecError> {
        if code != 0 { return Err(CodecError::RemoteFailure); }
        let mut r = Reader::new(bytes)?;
        let accepted = r.boolean()?;
        let count = r.count(9)?;
        let peak = bytes.len().checked_add(count * size_of::<Row>())
            .and_then(|n| n.checked_add((bytes.len() / 8).min(MAX_MAP_ENTRIES) * size_of::<RpcProperty<'_>>() + size_of::<Self>() + 64))
            .ok_or(CodecError::TooLarge)?;
        let charge = scope.reserve_native_rpc(peak)?;
        let mut rows = Vec::with_capacity(count);
        for _ in 0..count {
            let kind = r.bytes()?; let value = r.bytes()?; let preemptible = r.boolean()?;
            rows.push(Row { kind: Zeroizing::new(kind.to_vec()), value: Zeroizing::new(value.to_vec()), preemptible });
        }
        let footer = read_footer(&mut r)?;
        r.finish()?;
        if !accepted || !footer.reason.is_empty() { return Err(CodecError::RemoteFailure); }
        let compare = |a: &Row, b: &Row| compare_keys(&a.kind, &b.kind)
            .then_with(|| compare_keys(&a.value, &b.value)).then(a.preemptible.cmp(&b.preemptible));
        rows.sort_unstable_by(compare);
        if rows.windows(2).any(|ab| compare(&ab[0], &ab[1]).is_eq()) { return Err(CodecError::DuplicateKey); }
        Ok(Self { rows, _charge: charge })
    }
    pub(super) fn contains(&self, kind: &[u8], value: &[u8]) -> bool {
        self.rows.iter().any(|row| row.kind.as_slice() == kind && row.value.as_slice() == value)
    }
    // The original ordered triple set visits false before true for an equal
    // pair. Never erase all matching rows and never interpret the bit as a role.
    pub(super) fn preempt(&mut self, kind: &[u8], value: &[u8]) {
        if let Some(i) = self.rows.iter().position(|r| r.kind.as_slice() == kind && r.value.as_slice() == value) {
            if self.rows[i].preemptible { self.rows.remove(i); }
        }
    }
}
