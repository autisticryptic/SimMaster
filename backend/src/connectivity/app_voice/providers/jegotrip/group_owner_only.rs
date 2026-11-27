//! Automatic owner for the original empty input member-list branch only.
//! A retained UserEntry reference is required even though this UID-local path
//! never dials it. These private strings are DATA, not a WRITE permission.
use super::{group_create::digest, native_rpc_budget::NativeRpcReservation,
    router_connector::RouterPolicy, rpc_pending::RpcConnectionScope, rpc_stream::{CodecError,MAX_FIELD_BYTES},
    session::AttemptOwner, user_entry_data::{valid_uid,RecipientUri}, user_entry_local::UserEntryLocalReference,
    user_entry_ownership::UserEntryBook};
use serde::{Deserialize,Serialize};
use std::{fmt,sync::Arc};
use zeroize::Zeroizing;

#[derive(Clone,Copy,Deserialize,Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupOwnerOnlyConfig { pub from_session: bool }
impl fmt::Debug for GroupOwnerOnlyConfig { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str("GroupOwnerOnlyConfig(<redacted>)") } }

pub(super) struct OwnerOnlySource {
    owner: AttemptOwner,
    scope: RpcConnectionScope,
    book: Arc<UserEntryBook>,
    _reference: Arc<UserEntryLocalReference>,
    uri_digest: [u8;32],
    first: Zeroizing<Vec<u8>>,
    _budget: Arc<NativeRpcReservation>,
}
impl fmt::Debug for OwnerOnlySource { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.write_str("OwnerOnlySource(<redacted>)") } }
impl OwnerOnlySource {
    /// Only the runtime supplies the submitted, derived session URI. The ARC
    /// actor independently matches its own login bytes before allocating an ID.
    pub(super) fn original(owner:&AttemptOwner,scope:&RpcConnectionScope,router:&RouterPolicy,reference:Arc<UserEntryLocalReference>,book:Arc<UserEntryBook>,uri:&[u8])->Result<Arc<Self>,CodecError> {
        if !reference.matches(owner,scope,router)||!book.permits_client_read(owner,scope){return Err(CodecError::InvalidField);}
        let first=RecipientUri::parse(uri)?.value();
        let budget=scope.reserve_native_rpc(reference.retained_bytes()+first.len()+owner.line_id().len()+std::mem::size_of::<Self>()+128)?;
        Ok(Arc::new(Self{owner:owner.clone(),scope:scope.clone(),book,_reference:reference,uri_digest:digest(&[b"original-group-owner-uri-v1",uri]),first:Zeroizing::new(first.to_vec()),_budget:budget}))
    }
    pub(super) fn current(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool {
        &self.owner==owner&&self.scope.matches(scope)&&self._budget.belongs_to(scope)&&self.book.permits_client_read(owner,scope)
    }
    pub(super) fn current_uri(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,uri:Option<&[u8]>)->bool {
        self.current(owner,scope)&&uri.is_some_and(|v|digest(&[b"original-group-owner-uri-v1",v])==self.uri_digest)
    }
    pub(super) fn size(&self,uid:&[u8],properties:&[u8])->Result<usize,CodecError> {
        if !self.current(&self.owner,&self.scope)||!valid_uid(uid)||properties.first()!=Some(&0){return Err(CodecError::InvalidField);}
        // Version[0], one map entry, UID/type/two strings/empty member map,
        // followed by the group property map (without another stream marker).
        let n=28+uid.len()+self.first.len()+properties.len();
        if n>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}Ok(n)
    }
    pub(super) fn arguments(&self,uid:&[u8],properties:&[u8])->Result<Zeroizing<Vec<u8>>,CodecError> {
        let n=self.size(uid,properties)?;let mut out=Zeroizing::new(Vec::with_capacity(n));
        out.extend_from_slice(&[0,0,1,0,0]);out.extend_from_slice(&1u32.to_be_bytes());
        out.extend_from_slice(&(uid.len() as u32).to_be_bytes());out.extend_from_slice(uid);
        out.extend_from_slice(&0u32.to_be_bytes());out.extend_from_slice(&(self.first.len() as u32).to_be_bytes());out.extend_from_slice(&self.first);
        out.extend_from_slice(&[0;8]);out.extend_from_slice(&properties[1..]);debug_assert_eq!(out.len(),n);Ok(out)
    }
}
