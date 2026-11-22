//! Original matched-bootstrap target DATA for the current SDK's endpoint-less
//! Client factory. This snapshot is not authentication or a transport grant.
//!
//! Supplied read-only native evidence: 59169C/E854B8 parses the full reference;
//! with endpoint-vector count zero, 591560 retains the exact decoded factory
//! STRING at +112. 59994C forces `id` to that whole STRING after merging Agent
//! and per-call metadata, not to a parsed identity or canonical re-encoding.
//! The endpoint-bearing Common factory (176/DC8230) remains a separate path.
//!
//! Retaining Adapter/locator or grouped identity syntax here does NOT implement
//! locator wire/routing. No AuxBook, SDK login, spawn, pending id, Query, WRITE,
//! retry or UID/body metadata can be obtained from this DATA alone. A consumer
//! must independently check current sealed SDK auth and account admission, and
//! charge all retained preparation bytes to the existing 128-KiB total budget.
use super::{
    arc_session::OwnedBootstrapReply,
    legacy_config::ConfigDecodeKey,
    object_reference::{ObjectReference, ReferenceTarget},
    router_connector::{RouterPolicy, RouterPolicyIdentity},
    rpc_pending::RpcConnectionScope,
    rpc_stream::{CodecError, MAX_FIELD_BYTES},
    session::AttemptOwner,
    user_entry_reference::UserEntryReference,
};
use std::fmt;
use zeroize::Zeroizing;

pub(super) struct NewClientTarget {
    owner: AttemptOwner,
    parent: RpcConnectionScope,
    router: RouterPolicyIdentity,
    bootstrap_id: i32,
    reference: ObjectReference,
    realm: Zeroizing<Vec<u8>>,
    cc: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for NewClientTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("NewClientTarget(<redacted>)")
    }
}

impl NewClientTarget {
    pub(super) fn from_bootstrap(
        reply: &OwnedBootstrapReply,
        owner: &AttemptOwner,
        source: &RpcConnectionScope,
        parent: &RpcConnectionScope,
        router: &RouterPolicy,
        reference_key: Option<&ConfigDecodeKey>,
        realm: &[u8],
        cc: &[u8],
    ) -> Result<Option<Box<Self>>, CodecError> {
        // Check the original source before decrypting/parsing. Matching the
        // bootstrap is performed by UserEntryReference, not inferred from data.
        if source.matches(parent) || !source.is_open() || source.session_revoked()
            || !parent.is_open() || parent.session_revoked() || realm.is_empty()
        {
            return Err(CodecError::InvalidField);
        }
        if realm.len() > MAX_FIELD_BYTES || cc.len() > MAX_FIELD_BYTES {
            return Err(CodecError::TooLarge);
        }
        let Some(original) = UserEntryReference::from_bootstrap(reply, owner, source, reference_key)? else {
            return Ok(None);
        };
        let reference = original.reference_for(owner, source).ok_or(CodecError::InvalidField)?;
        match reference.target() {
            ReferenceTarget::IdentityOnly | ReferenceTarget::Adapter { .. } => {},
            // The current parser rejects empty endpoint syntax. If an original
            // parsed Direct(empty) becomes representable, it still has no
            // explicit endpoint vector; never invent a default destination.
            ReferenceTarget::Direct(endpoints) if endpoints.is_empty() => {},
            ReferenceTarget::Direct(_) => return Err(CodecError::InvalidField),
        }
        // Preserve the existing qualified-target factory-input/realm/cc bound;
        // parsed storage is counted separately below, never canonical-encoded.
        let bytes = reference.original_factory_input().len()
            .checked_add(realm.len()).and_then(|n| n.checked_add(cc.len()))
            .ok_or(CodecError::TooLarge)?;
        if bytes > MAX_FIELD_BYTES {
            return Err(CodecError::TooLarge);
        }
        let bootstrap_id = original.original_call_id();
        // Move this very parsed/raw object while its original source grant is
        // valid. There is no public constructor accepting substitute raw data.
        let reference = original.into_reference(owner, source)?;
        if !parent.is_open() || parent.session_revoked() {
            return Err(CodecError::InvalidField);
        }
        Ok(Some(Box::new(Self {
            owner: owner.clone(),
            parent: parent.clone(),
            router: router.original_identity(),
            bootstrap_id,
            reference,
            realm: Zeroizing::new(realm.to_vec()),
            cc: Zeroizing::new(cc.to_vec()),
        })))
    }

    /// DATA access under the original parent and Router policy identity. Source
    /// closure after capture neither erases this DATA nor revives source auth.
    /// Like other borrowed snapshots, a returned slice is not a future IO grant.
    pub(super) fn matches(&self, owner: &AttemptOwner, parent: &RpcConnectionScope, router: &RouterPolicy) -> bool {
        &self.owner == owner && self.parent.matches(parent) && parent.is_open()
            && !parent.session_revoked() && self.router.matches(router)
    }

    pub(super) fn factory_input_for(&self, owner: &AttemptOwner, parent: &RpcConnectionScope, router: &RouterPolicy) -> Option<&[u8]> {
        self.matches(owner, parent, router).then(|| self.reference.original_factory_input())
    }

    /// Original bounded metadata DATA, not an authority, host or trust anchor.
    pub(super) fn realm_for(&self, owner: &AttemptOwner, parent: &RpcConnectionScope, router: &RouterPolicy) -> Option<&[u8]> {
        self.matches(owner, parent, router).then_some(self.realm.as_slice())
    }

    pub(super) fn cc_for(&self, owner: &AttemptOwner, parent: &RpcConnectionScope, router: &RouterPolicy) -> Option<&[u8]> {
        self.matches(owner, parent, router).then_some(self.cc.as_slice())
    }

    /// Owned payload bytes (exact original + parsed fields + realm + cc), using
    /// ObjectReference's existing accounting. No encoding or allocation occurs.
    pub(super) fn retained_bytes(&self) -> usize {
        self.reference.retained_bytes() + self.realm.len() + self.cc.len()
    }

    pub(super) fn original_bootstrap_id(&self) -> i32 { self.bootstrap_id }
}

#[cfg(test)]
#[path = "user_entry_client_target_tests.rs"]
mod tests;
