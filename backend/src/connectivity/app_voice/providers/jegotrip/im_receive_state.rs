//! Original per-box receive ordering model. This DATA model grants neither IO
//! nor read-status writes; only current scoped replies plus durable consumption
//! will drive it. Missing baseline is not initialized from a notification ID.
use super::{im_data::{ImBox,GAP_BATCH_COUNT},im_status::{RefreshResult,StatusEntry},rpc_stream::CodecError};
use std::{collections::BTreeMap,fmt};
use zeroize::{Zeroize,Zeroizing};
const MAX_BOXES:usize=128;
#[derive(Clone,PartialEq,Eq)]
struct BoxKey(Vec<u8>);
impl Drop for BoxKey{fn drop(&mut self){self.0.zeroize();}}
impl Ord for BoxKey{fn cmp(&self,other:&Self)->std::cmp::Ordering{super::rpc_stream::compare_keys(&self.0,&other.0)}}
impl PartialOrd for BoxKey{fn partial_cmp(&self,other:&Self)->Option<std::cmp::Ordering>{Some(self.cmp(other))}}
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct Gap{pub(super) start:i64,pub(super) count:u32}
impl Gap{pub(super) fn end(self)->Result<i64,CodecError>{if self.count==0||self.count>GAP_BATCH_COUNT{return Err(CodecError::InvalidField);}self.start.checked_add(i64::from(self.count)-1).ok_or(CodecError::InvalidField)}}
#[derive(Clone,Copy)]
struct Cursor{baseline:i64,consumed:i64,target:i64}
impl Cursor{
    fn from_status(entry:&StatusEntry<'_>)->Option<Self>{let(base,target)=entry.reported_range()?;Some(Self{baseline:base,consumed:base,target})}
    fn gap(self)->Result<Option<Gap>,CodecError>{
        if self.consumed>=self.target{return Ok(None);}let start=self.consumed.checked_add(1).ok_or(CodecError::InvalidField)?;
        let count=(i128::from(self.target)-i128::from(start)+1).min(i128::from(GAP_BATCH_COUNT))as u32;let gap=Gap{start,count};gap.end()?;Ok(Some(gap))
    }
}
pub(super) struct ImReceiveState{update_time:i64,initial_status:bool,boxes:BTreeMap<BoxKey,Cursor>}
impl fmt::Debug for ImReceiveState{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImReceiveState(<redacted>)")}}
impl ImReceiveState{
    ///5B0084 initializes the refresh TIMESTAMP to-1. Per-box receive cursors
    ///stay absent until an actual complete current status entry is provided.
    pub(super) fn new()->Self{Self{update_time:-1,initial_status:false,boxes:BTreeMap::new()}}
    pub(super) fn since(&self)->i64{self.update_time}
    pub(super) fn has_initial_status(&self)->bool{self.initial_status}
    pub(super) fn observe_status(&mut self,result:&RefreshResult<'_>)->Result<bool,CodecError>{
        if !result.accepted{return Err(CodecError::RemoteFailure);}if result.update_time<=self.update_time{return Ok(false);}
        // Validate the complete proposed update/capacity before changing any
        // live state. Incomplete entries cannot authorize a cursor or a write.
        let mut updates=Vec::new();let mut added=0;
        for entry in &result.entries{let Some(cursor)=Cursor::from_status(entry)else{continue;};let key=BoxKey(entry.label.box_bytes().to_vec());if !self.boxes.contains_key(&key){added+=1;}updates.push((key,cursor));}
        if self.boxes.len().checked_add(added).is_none_or(|n|n>MAX_BOXES){return Err(CodecError::TooLarge);}
        for(key,cursor)in updates{
            if let Some(old)=self.boxes.get_mut(&key){
                // New remote status is observation, not proof that skipped
                // local data was consumed. Never jump the durable cursor here.
                old.target=old.target.max(cursor.target);
            }else{self.boxes.insert(key,cursor);}
        }
        self.update_time=result.update_time;self.initial_status|=result.entries.iter().all(|entry|entry.reported_range().is_some());Ok(true)
    }
    pub(super) fn next_gap(&self)->Result<Option<(Zeroizing<Vec<u8>>,Gap)>,CodecError>{
        if !self.initial_status{return Ok(None);}for(key,cursor)in &self.boxes{if let Some(gap)=cursor.gap()?{return Ok(Some((Zeroizing::new(key.0.clone()),gap)));}}Ok(None)
    }
    /// Call only after matching an original requested gap and atomically
    /// recording every returned record or explicit server-empty gap outcome.
    /// It is NOT called merely because a notification or MoreMessage arrived.
    pub(super) fn committed_gap(&mut self,box_id:ImBox<'_>,original:Gap)->Result<(),CodecError>{
        let key=BoxKey(box_id.bytes().to_vec());let cursor=self.boxes.get_mut(&key).ok_or(CodecError::InvalidField)?;
        let end=original.end()?;if original.start!=cursor.consumed.checked_add(1).ok_or(CodecError::InvalidField)?||end<cursor.consumed||end>cursor.target{return Err(CodecError::InvalidField);}cursor.consumed=end;Ok(())
    }
    pub(super) fn observed_target(&mut self,box_id:ImBox<'_>,index:i64)->Result<(),CodecError>{
        if index<0{return Err(CodecError::InvalidField);}let key=BoxKey(box_id.bytes().to_vec());
        if let Some(cursor)=self.boxes.get_mut(&key){cursor.target=cursor.target.max(index);return Ok(());}
        // Original5C7C10 may seed a new box from a real first message index-1
        // only AFTER initial refresh. A caller must NOT pass Notify.MsgIdx.
        if !self.initial_status{return Err(CodecError::InvalidField);}if self.boxes.len()>=MAX_BOXES{return Err(CodecError::TooLarge);}
        let base=index.checked_sub(1).ok_or(CodecError::InvalidField)?;self.boxes.insert(key,Cursor{baseline:base,consumed:base,target:index});Ok(())
    }
    pub(super) fn consumed(&self,box_id:ImBox<'_>)->Option<(i64,i64)>{self.boxes.get(&BoxKey(box_id.bytes().to_vec())).map(|c|(c.baseline,c.consumed))}
}
