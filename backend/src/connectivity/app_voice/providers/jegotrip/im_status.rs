//! Original Group status refresh DATA, not a cursor/IO/consumption grant.
//! 40BAD0: version[0], current UID, label-set, type-set, update-time64.
//! 405050/597DB8/597BD8: bool, map<label,map<string,string>>, time64, footer.
//! Automatic Message.setRead and user P:P_UDMR-N marking remain distinct.
use super::{im_data::{ImBox,MAX_BOX_BYTES},rpc_stream::{compare_keys,read_footer,CodecError,Reader,RpcBytes,RpcFooter,RpcProperty,Writer,MAX_FIELD_BYTES,MAX_MAP_ENTRIES}};
use std::fmt;
pub(super) const REFRESH_METHOD:&str="refreshStatuses.GroupServer.Group";
pub(super) const STATUS_KIND:&[u8]=b"Status.Update";
pub(super) const LATEST:&[u8]=b"ML";
pub(super) const BRIEF:&[u8]=b"MLB";
pub(super) const PEER_READ:&[u8]=b"P:P_MR";
pub(super) const LAST_RECEIVED:&[u8]=b"P:P_MV-N";
pub(super) const LAST_READ:&[u8]=b"P:P_UDMR-N";
pub(super) const PEER_READ_NOTIFICATION:&[u8]=b"P:P_MPR-N";
const REFRESH_TYPES:[&[u8];5]=[LATEST,BRIEF,PEER_READ,LAST_RECEIVED,LAST_READ];
/// A raw Group status label is not a Box, domain, phone number or URL.
#[derive(Clone,Copy,PartialEq,Eq)]
pub(super) struct StatusLabel<'a>{bytes:&'a[u8],prefix:&'static[u8]}
impl fmt::Debug for StatusLabel<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImStatusLabel(<redacted>)")}}
impl<'a>StatusLabel<'a>{
    pub(super) fn parse(bytes:&'a[u8])->Result<Self,CodecError>{
        if bytes.is_empty()||bytes.len()>MAX_BOX_BYTES-4{return Err(CodecError::InvalidField);}
        // Bounded grammar reuse; these private DATA values never select routes.
        for prefix in [b"P2P/".as_slice(),b"Org/".as_slice()]{
            let mut box_id=zeroize::Zeroizing::new(Vec::with_capacity(prefix.len()+bytes.len()));box_id.extend_from_slice(prefix);box_id.extend_from_slice(bytes);
            if ImBox::parse(&box_id).is_ok(){return Ok(Self{bytes,prefix});}
        }
        Err(CodecError::InvalidField)
    }
    pub(super) fn bytes(self)->&'a[u8]{self.bytes}
    pub(super) fn box_bytes(self)->zeroize::Zeroizing<Vec<u8>>{let mut value=zeroize::Zeroizing::new(Vec::with_capacity(self.prefix.len()+self.bytes.len()));value.extend_from_slice(self.prefix);value.extend_from_slice(self.bytes);value}
}
/// Missing status is kept missing. It is NOT caller-selected index0/-1 or a
/// local receipt, and malformed native fallback strings are rejected.
fn index(bytes:Option<&[u8]>)->Result<Option<i64>,CodecError>{
    let Some(bytes)=bytes else{return Ok(None);};if bytes.is_empty()||bytes.len()>20{return Err(CodecError::InvalidField);}
    let text=std::str::from_utf8(bytes).map_err(|_|CodecError::InvalidField)?;let n=text.parse::<i64>().map_err(|_|CodecError::InvalidField)?;
    if n< -1||n.to_string()!=text{return Err(CodecError::InvalidField);}Ok(Some(n))
}
pub(super) struct StatusEntry<'a>{pub(super) label:StatusLabel<'a>,pub(super) latest:Option<i64>,pub(super) peer_read:Option<i64>,pub(super) last_received:Option<i64>,pub(super) last_read:Option<i64>,pub(super) properties:Vec<RpcProperty<'a>>}
impl fmt::Debug for StatusEntry<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImStatusEntry(<redacted>)")}}
impl<'a>StatusEntry<'a>{
    fn capture(label:StatusLabel<'a>,properties:Vec<RpcProperty<'a>>)->Result<Self,CodecError>{
        let get=|key:&[u8]|properties.iter().find(|p|p.key==key).map(|p|p.value);
        let latest=index(get(LATEST))?;let peer_read=index(get(PEER_READ))?;let last_received=index(get(LAST_RECEIVED))?;let last_read=index(get(LAST_READ))?;
        // The peer-read notification is a separate native status key; validate
        // its signed index without treating it as LAST_READ or local receipt.
        index(get(PEER_READ_NOTIFICATION))?;
        Ok(Self{label,latest,peer_read,last_received,last_read,properties})
    }
    /// Native5C92D0 seeds current from peer-read and target from latest, clipping
    /// current>target. Both fields must actually be present; no inference from
    /// missing properties, message count, notificationID, SMS or public input.
    pub(super) fn reported_range(&self)->Option<(i64,i64)>{let target=self.latest?;let current=self.peer_read?;Some((current.min(target),target))}
}
pub(super) struct RefreshResult<'a>{pub(super) accepted:bool,pub(super) entries:Vec<StatusEntry<'a>>,pub(super) update_time:i64,pub(super) footer:RpcFooter<'a>}
impl fmt::Debug for RefreshResult<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImRefreshResult(<redacted>)")}}
pub(super) fn decode_refresh(result_code:u32,bytes:&[u8])->Result<RefreshResult<'_>,CodecError>{
    if result_code!=0{return Err(CodecError::RemoteFailure);}
    let mut r=Reader::new(bytes)?;let accepted=r.boolean()?;let count=r.count(8)?;let mut entries:Vec<StatusEntry<'_>>=Vec::with_capacity(count);
    for _ in 0..count{let label=StatusLabel::parse(r.bytes()?)?;if entries.iter().any(|entry|entry.label.bytes==label.bytes){return Err(CodecError::DuplicateKey);}let properties=r.properties()?;entries.push(StatusEntry::capture(label,properties)?);}
    let update_time=r.i64()?;if update_time< -1{return Err(CodecError::InvalidField);}let footer=read_footer(&mut r)?;r.finish()?;
    Ok(RefreshResult{accepted:accepted&&footer.reason.is_empty(),entries,update_time,footer})
}
fn string_set(w:&mut Writer,values:&[&[u8]])->Result<(),CodecError>{
    if values.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}
    let mut values=values.to_vec();values.sort_by(|a,b|compare_keys(a,b));if values.windows(2).any(|v|v[0]==v[1]){return Err(CodecError::DuplicateKey);}
    w.u32(values.len()as u32)?;for value in values{w.bytes(value)?;}Ok(())
}
/// UID comes from matched SDK authentication at the eventual call site. The
/// DATA encoder has no default identity, token, endpoint or operation replay.
pub(super) fn encode_refresh(uid:&[u8],labels:&[StatusLabel<'_>],since:i64)->Result<RpcBytes,CodecError>{
    if uid.is_empty()||uid.len()>MAX_FIELD_BYTES||uid.iter().any(u8::is_ascii_control)||since< -1{return Err(CodecError::InvalidField);}
    let mut w=Writer::versioned()?;w.bytes(uid)?;let labels:Vec<_>=labels.iter().map(|label|label.bytes()).collect();string_set(&mut w,&labels)?;string_set(&mut w,&REFRESH_TYPES)?;w.i64(since)?;Ok(w.finish())
}
/// The online payload is NOT the nested refresh map or MoreMessage record.
/// 5BD4CC requires Group.GroupId/Group.UpdateTime and one type0 property map.
pub(super) struct StatusUpdate<'a>{pub(super) entry:StatusEntry<'a>,pub(super) update_time:i64}
impl fmt::Debug for StatusUpdate<'_>{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImStatusUpdate(<redacted>)")}}
pub(super) fn decode_owned_status<'a>(label:&'a[u8],update_time:i64,body:&'a[u8])->Result<StatusUpdate<'a>,CodecError>{
    if update_time< -1{return Err(CodecError::InvalidField);}let label=StatusLabel::parse(label)?;let mut r=Reader::new(body)?;let properties=r.properties()?;r.finish()?;
    Ok(StatusUpdate{entry:StatusEntry::capture(label,properties)?,update_time})
}
pub(super) fn decode_status_update<'a>(kind:&[u8],header:&[RpcProperty<'a>],body:&'a[u8])->Result<Option<StatusUpdate<'a>>,CodecError>{
    if kind!=STATUS_KIND{return Ok(None);}
    if header.len()>MAX_MAP_ENTRIES{return Err(CodecError::TooLarge);}for(i,p)in header.iter().enumerate(){if header[..i].iter().any(|other|other.key==p.key){return Err(CodecError::DuplicateKey);}if p.key.len()>MAX_FIELD_BYTES||p.value.len()>MAX_FIELD_BYTES{return Err(CodecError::TooLarge);}}
    let get=|key:&[u8]|header.iter().find(|p|p.key==key).map(|p|p.value);let label=StatusLabel::parse(get(b"Group.GroupId").ok_or(CodecError::InvalidField)?)?;
    let update_time=index(get(b"Group.UpdateTime"))?.ok_or(CodecError::InvalidField)?;let mut r=Reader::new(body)?;let properties=r.properties()?;r.finish()?;
    Ok(Some(StatusUpdate{entry:StatusEntry::capture(label,properties)?,update_time}))
}
