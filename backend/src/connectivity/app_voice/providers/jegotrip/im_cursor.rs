//! Durable original native IM receive positions. A server baseline is not a
//! local message receipt; only atomic records/explicit missing-range outcomes
//! advance consumed. No cached row is a current IO/write/notification grant.
use super::*;
use super::super::{im_data::{ImBox,ImMessage,GAP_BATCH_COUNT},im_receive_state::Gap,
    rpc_im::{ImQuery,ImReplyData,OwnedImReply}};
use ring::digest::{Context,SHA256};
use zeroize::Zeroize;
const MAX_BOXES:i64=128;
const MAX_CURSOR_TOTAL:i64=4096;
const CURSOR:&str="app_account_im_cursor_invalid";
const UNSUPPORTED:&str="app_account_im_type_unsupported";
fn hash(parts:&[&[u8]])->[u8;32]{let mut d=Context::new(&SHA256);for p in parts{d.update(&(p.len()as u64).to_be_bytes());d.update(p);}let mut out=[0;32];out.copy_from_slice(d.finish().as_ref());out}
pub(in super::super) fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_im_receive_cursors(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        box_id BLOB NOT NULL CHECK(typeof(box_id)='blob' AND length(box_id) BETWEEN 1 AND 512),
        baseline INTEGER NOT NULL CHECK(baseline>=-1),consumed INTEGER NOT NULL CHECK(consumed>=baseline),
        target INTEGER NOT NULL CHECK(target>=consumed),status_time INTEGER NOT NULL CHECK(status_time>=-1),
        observed_run TEXT NOT NULL,observed_epoch TEXT NOT NULL,
        last_span_start INTEGER,last_span_count INTEGER CHECK(last_span_count BETWEEN 1 AND 10),last_span_end INTEGER,
        last_span_hash BLOB CHECK(last_span_hash IS NULL OR(typeof(last_span_hash)='blob' AND length(last_span_hash)=32)),
        PRIMARY KEY(line_id,revision,identity_key,box_id));") .map_err(|_|STORAGE)
}
pub(in super::super) fn delete_account(conn:&Connection,line:&str)->Result<(),&'static str>{
    if account_store::metadata(conn,line)?.is_some(){return Err(SCOPE);}
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_im_receive_cursors')",[],|row|row.get(0)).map_err(|_|STORAGE)?;
    if exists{conn.execute("DELETE FROM app_voice_im_receive_cursors WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}Ok(())
}
struct Record{key:Arc<ImKey>,raw:Zeroizing<Vec<u8>>,text:Zeroizing<String>,sender:Zeroizing<Vec<u8>>,fingerprint:[u8;32],time:i64,attachments:i64}
impl Record{
    fn capture(binding:&ImBinding,box_id:ImBox<'_>,message:&ImMessage<'_>)->Result<Self,&'static str>{
        let text=ImText::decode(message).map_err(|_|INVALID)?.ok_or(UNSUPPORTED)?;
        let raw=message.encode().map_err(|_|INVALID)?;let raw=Zeroizing::new(raw.expose_to_transport().to_vec());
        if raw.len()>65536||text.text().len()>65536||message.sender_uid.len()>65536{return Err(INVALID);}
        let key=ImKey::capture(binding,box_id,message.index);let fingerprint=key.message_fingerprint(&raw);
        Ok(Self{key,raw,text:Zeroizing::new(text.text().to_owned()),sender:Zeroizing::new(message.sender_uid.to_vec()),fingerprint,time:message.time,attachments:message.attachments.len()as i64})
    }
    fn bytes(&self)->usize{self.raw.len()+self.text.len()+self.sender.len()}
}
/// One current-original login view, not a serialized/restartable capability.
pub(in super::super) struct CursorSession{context:ImInboxScope,since:i64,initial:bool}
impl fmt::Debug for CursorSession{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImCursorSession(<redacted>)")}}
pub(in super::super) struct GapPlan{binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>,run:Arc<RunToken>,query:Arc<ImQuery>,baseline:i64,consumed:i64}
impl fmt::Debug for GapPlan{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalImGapPlan(<redacted>)")}}
impl GapPlan{
    pub(in super::super) fn query(&self)->Arc<ImQuery>{self.query.clone()}
    pub(in super::super) fn box_bytes(&self)->&[u8]{match self.query.as_ref(){ImQuery::Receive{box_id,..}=>box_id,_=>unreachable!("private original gap")}}
}
pub(in super::super) struct GapCommitted{binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>,run:Arc<RunToken>,box_id:Zeroizing<Vec<u8>>,start:i64,end:i64,span_hash:[u8;32],proofs:Vec<CommittedIm>}
impl fmt::Debug for GapCommitted{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommittedImGap(<redacted>)")}}
impl GapCommitted{
    pub(in super::super) fn proofs(&self)->&[CommittedIm]{&self.proofs}
    pub(in super::super) fn end(&self)->i64{self.end}
    pub(in super::super) fn box_bytes(&self)->&[u8]{&self.box_id}
    pub(in super::super) fn same_captured_span(&self,other:&Self)->bool{Arc::ptr_eq(&self.binding,&other.binding)&&Arc::ptr_eq(&self.gate,&other.gate)&&Arc::ptr_eq(&self.run,&other.run)&&self.box_id.as_slice()==other.box_id.as_slice()&&self.start==other.start&&self.end==other.end&&self.span_hash==other.span_hash}
    pub(in super::super) fn captured_copy(&self)->Self{Self{binding:self.binding.clone(),gate:self.gate.clone(),run:self.run.clone(),box_id:Zeroizing::new(self.box_id.to_vec()),start:self.start,end:self.end,span_hash:self.span_hash,proofs:self.proofs.iter().map(CommittedIm::captured_copy).collect()}}
    pub(in super::super) fn validate_current(&self,conn:&Connection)->Result<(),&'static str>{
        let context=self.receipt_context();context.validate(conn)?;
        let current:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_receive_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND last_span_start=?5 AND last_span_end=?6 AND last_span_hash=?7 AND observed_run=?8 AND observed_epoch=?9)",params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),self.box_id.as_slice(),self.start,self.end,self.span_hash.as_slice(),self.run.nonce(),self.binding.receipt_epoch()],|r|r.get(0)).map_err(|_|STORAGE)?;
        if !current||!self.gate.live()||!self.binding.matches(self.binding.owner(),&self.binding.scope()){return Err(SCOPE);}Ok(())
    }
    pub(in super::super) fn receipt_context(&self)->ImInboxScope{ImInboxScope{run:self.run.clone(),binding:self.binding.clone(),gate:self.gate.clone()}}
    pub(in super::super) fn receipt_span(&self)->(&[u8],i64,[u8;32]){(&self.box_id,self.end,self.span_hash)}
    pub(in super::super) fn retained_bytes(&self)->usize{self.box_id.len()+256+self.proofs.iter().map(|p|p.key.bytes()+160).sum::<usize>()}
    /// Current scoped evidence only. This is not yet an installed SetReceived
    /// IO capability; a later write journal must recheck these durable facts.
    pub(in super::super) fn is_current(&self,db:&Database)->bool{
        if !self.gate.live()||!self.binding.matches(self.binding.owner(),&self.binding.scope()){return false;}
        let context=ImInboxScope{run:self.run.clone(),binding:self.binding.clone(),gate:self.gate.clone()};
        db.with_conn(|conn|{context.validate(conn)?;let saved:Option<(i64,i64,Vec<u8>,String,String)>=conn.query_row(
            "SELECT last_span_start,last_span_end,last_span_hash,observed_run,observed_epoch FROM app_voice_im_receive_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4",
            params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),self.box_id.as_slice()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?))).optional().map_err(|_|STORAGE)?;
            Ok(saved.is_some_and(|(start,end,h,run,epoch)|start==self.start&&end==self.end&&h.as_slice()==self.span_hash.as_slice()&&run==self.run.nonce()&&epoch==self.binding.receipt_epoch()))}).unwrap_or(false)
    }
}
#[derive(Clone,Copy)]
pub(in super::super) enum OnlinePosition{Pending,Replay,Next,MissingOlder}
impl CursorSession{
    pub(in super::super) fn from_refresh(run:Arc<RunToken>,gate:Arc<ImReceiptGate>,reply:&OwnedImReply)->Result<Self,&'static str>{
        if !matches!(reply.request().query(),ImQuery::Refresh{..})||!Arc::ptr_eq(&gate,&reply.request().gate()){return Err(SCOPE);}
        let context=ImInboxScope::original(run,reply.request().binding().clone(),gate)?;
        let s=Self{context,since:-1,initial:false};s.reply_current(reply)?;Ok(s)
    }
    fn live(&self)->bool{self.context.gate.live()&&self.context.binding.matches(self.context.binding.owner(),&self.context.binding.scope())}
    fn reply_current(&self,reply:&OwnedImReply)->Result<(),&'static str>{
        if !self.live()||!reply.matches(self.context.binding.owner(),&self.context.binding.scope())
            ||!Arc::ptr_eq(reply.request().binding(),&self.context.binding)||!Arc::ptr_eq(&reply.request().gate(),&self.context.gate){return Err(SCOPE);}Ok(())
    }
    pub(in super::super) fn since(&self)->i64{self.since}
    pub(in super::super) fn initial(&self)->bool{self.initial}
    fn limit_new(&self,tx:&Connection)->Result<(),&'static str>{
        let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_im_receive_cursors",[self.context.run.line_id()],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|STORAGE)?;
        if local>=MAX_BOXES||total>=MAX_CURSOR_TOTAL{return Err(FULL);}Ok(())
    }
    pub(in super::super) fn observe_refresh(&mut self,db:&Database,reply:&OwnedImReply)->Result<bool,&'static str>{
        self.reply_current(reply)?;let ImQuery::Refresh{since,..}=reply.request().query()else{return Err(CURSOR);};
        if *since!=self.since{return Err(CURSOR);}let ImReplyData::Refresh(result)=reply.result().map_err(|_|INVALID)?else{return Err(CURSOR);};
        if !result.accepted{return Err("app_account_im_refresh_rejected");}
        if result.update_time<self.since||self.initial&&result.update_time==self.since{return Ok(false);}
        let complete=result.entries.iter().all(|entry|entry.reported_range().is_some());
        db.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.context.validate(&tx)?;self.reply_current(reply)?;
            for entry in &result.entries{let Some((baseline,target))=entry.reported_range()else{continue;};let box_id=entry.label.box_bytes();
                let exists:Option<(i64,i64)>=tx.query_row("SELECT consumed,target FROM app_voice_im_receive_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4",
                    params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id.as_slice()],|row|Ok((row.get(0)?,row.get(1)?))).optional().map_err(|_|STORAGE)?;
                if let Some((consumed,old_target))=exists{
                    if target<consumed{return Err("app_account_im_cursor_rewind");}
                    // A newer remote peer-read is never local consumption.
                    tx.execute("UPDATE app_voice_im_receive_cursors SET target=?5,status_time=?6,observed_run=?7,observed_epoch=?8 WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4",
                        params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id.as_slice(),target.max(old_target),result.update_time,self.context.run.nonce(),self.context.binding.receipt_epoch()]).map_err(|_|STORAGE)?;
                }else{self.limit_new(&tx)?;tx.execute("INSERT INTO app_voice_im_receive_cursors(line_id,revision,identity_key,box_id,baseline,consumed,target,status_time,observed_run,observed_epoch) VALUES(?1,?2,?3,?4,?5,?5,?6,?7,?8,?9)",
                    params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id.as_slice(),baseline,target,result.update_time,self.context.run.nonce(),self.context.binding.receipt_epoch()]).map_err(|_|STORAGE)?;}
            }
            self.reply_current(reply)?;tx.commit().map_err(|_|STORAGE)
        })?;
        self.reply_current(reply)?;self.since=result.update_time;self.initial|=complete;Ok(true)
    }
    pub(in super::super) fn next_gap(&self,db:&Database)->Result<Option<GapPlan>,&'static str>{
        if !self.initial{return Ok(None);}if !self.live(){return Err(SCOPE);}
        let row:Option<(Vec<u8>,i64,i64,i64)>=db.with_conn(|conn|{self.context.validate(conn)?;conn.query_row(
            "SELECT box_id,baseline,consumed,target FROM app_voice_im_receive_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND observed_run=?4 AND observed_epoch=?5 AND consumed<target ORDER BY length(box_id),box_id LIMIT 1",
            params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),self.context.run.nonce(),self.context.binding.receipt_epoch()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_|STORAGE)})?;
        row.map(|(mut box_id,baseline,consumed,target)|{let start=consumed.checked_add(1).ok_or(CURSOR)?;let count=(i128::from(target)-i128::from(start)+1).min(i128::from(GAP_BATCH_COUNT))as u32;
            let query=ImQuery::receive(ImBox::parse(&box_id).map_err(|_|CURSOR)?,start,count).map_err(|_|CURSOR)?;box_id.zeroize();
            Ok(GapPlan{binding:self.context.binding.clone(),gate:self.context.gate.clone(),run:self.context.run.clone(),query:Arc::new(query),baseline,consumed})}).transpose()
    }
    fn insert_record(&self,tx:&Connection,record:&Record)->Result<(i64,Option<crate::platform::db::AppEventEntry>),&'static str>{
        let key=&record.key;let existing:Option<(i64,Vec<u8>)>=tx.query_row("SELECT id,fingerprint FROM app_voice_im_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND message_index=?5",
            params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),key.box_id().bytes(),key.index()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|STORAGE)?;
        if let Some((id,fingerprint))=existing{if fingerprint.as_slice()!=record.fingerprint.as_slice(){return Err("app_account_im_message_conflict");}return Ok((id,None));}
        let(local,total,bytes):(i64,i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*),COALESCE(SUM(length(body)+length(CAST(text AS BLOB))+length(sender)),0) FROM app_voice_im_messages",[self.context.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|STORAGE)?;
        if local>=MAX_PER_ACCOUNT||total>=MAX_TOTAL||bytes.checked_add(record.bytes()as i64).is_none_or(|n|n>MAX_TOTAL_BYTES){return Err(FULL);}
        let kind=match key.box_id().kind(){super::super::im_data::BoxKind::P2P=>"p2p",super::super::im_data::BoxKind::Org=>"org"};let now=chrono::Utc::now().to_rfc3339();
        tx.execute("INSERT INTO app_voice_im_messages(line_id,revision,identity_key,box_id,box_kind,message_index,fingerprint,original_time,body,text,sender,attachment_count,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),key.box_id().bytes(),kind,key.index(),record.fingerprint.as_slice(),record.time,record.raw.as_slice(),record.text.as_str(),record.sender.as_slice(),record.attachments,now]).map_err(|_|STORAGE)?;
        let id=tx.last_insert_rowid();let payload=serde_json::json!({"im_id":id,"kind":"text","transport":"app_voice_im"}).to_string();let event=insert_app_event_for_conn(tx,"im.received",Some(self.context.run.line_id()),Some("app_voice_im"),&payload,&now).map_err(|_|STORAGE)?;Ok((id,Some(event)))
    }
    fn advance(&self,tx:&Connection,box_id:&[u8],baseline:i64,consumed:i64,gap:Gap,end:i64,span_hash:&[u8;32])->Result<(),&'static str>{
        if gap.start!=consumed.checked_add(1).ok_or(CURSOR)?||end<gap.start||end>gap.end().map_err(|_|CURSOR)?{return Err(CURSOR);}
        let n=tx.execute("UPDATE app_voice_im_receive_cursors SET consumed=?5,last_span_start=?6,last_span_count=?7,last_span_end=?5,last_span_hash=?8 WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND baseline=?9 AND consumed=?10 AND target>=?5 AND observed_run=?11 AND observed_epoch=?12",
            params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id,end,gap.start,gap.count,span_hash.as_slice(),baseline,consumed,self.context.run.nonce(),self.context.binding.receipt_epoch()]).map_err(|_|STORAGE)?;
        if n!=1{return Err(CURSOR);}Ok(())
    }
    pub(in super::super) fn commit_gap(&self,db:&Database,plan:&GapPlan,reply:&OwnedImReply)->Result<GapCommitted,&'static str>{
        self.reply_current(reply)?;
        if !self.initial||!Arc::ptr_eq(&plan.query,reply.request().query_arc())||!Arc::ptr_eq(&plan.binding,&self.context.binding)||!Arc::ptr_eq(&plan.gate,&self.context.gate)||plan.run.nonce()!=self.context.run.nonce(){return Err(SCOPE);}
        let ImQuery::Receive{box_id,start,count}=plan.query.as_ref()else{return Err(CURSOR);};let gap=Gap{start:*start,count:*count};let requested_end=gap.end().map_err(|_|CURSOR)?;
        let ImReplyData::Receive(result)=reply.result().map_err(|_|INVALID)?else{return Err(CURSOR);};if !result.accepted{return Err("app_account_im_receive_rejected");}
        //5BE534 ->5C8B28 ->5C7C10: positive empty advances the ORIGINAL requested
        // end; a nonempty result advances only to its actual greatest record.
        // Missing slots through that point are an explicit current recv outcome,
        // not made-up messages. Every returned record must be supported/atomic.
        let mut records=result.messages.iter().map(|m|Record::capture(&self.context.binding,ImBox::parse(box_id).map_err(|_|CURSOR)?,m)).collect::<Result<Vec<_>,_>>()?;
        records.sort_by_key(|r|r.key.index());let end=records.last().map_or(requested_end,|r|r.key.index());
        let mut evidence=Zeroizing::new(Vec::new());for r in &records{evidence.extend_from_slice(&r.key.index().to_be_bytes());evidence.extend_from_slice(&r.fingerprint);}
        let span_hash=hash(&[b"original-im-recv-span-v1\0",box_id,&start.to_be_bytes(),&count.to_be_bytes(),&end.to_be_bytes(),&evidence]);
        let(rows,events)=db.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.context.validate(&tx)?;self.reply_current(reply)?;let mut rows=Vec::new();let mut events=Vec::new();
            for r in &records{let(id,event)=self.insert_record(&tx,r)?;rows.push(id);if let Some(event)=event{events.push(event);}}
            self.advance(&tx,box_id,plan.baseline,plan.consumed,gap,end,&span_hash)?;self.reply_current(reply)?;tx.commit().map_err(|_|STORAGE)?;Ok((rows,events))})?;
        for event in events{db.publish_committed_app_event(event);}self.reply_current(reply)?;
        let proofs=records.iter().zip(rows).map(|(r,id)|self.context.permit(&r.key,r.fingerprint,id)).collect();
        Ok(GapCommitted{binding:self.context.binding.clone(),gate:self.context.gate.clone(),run:self.context.run.clone(),box_id:Zeroizing::new(box_id.to_vec()),start:*start,end,span_hash,proofs})
    }
    pub(in super::super) fn online_position(&self,db:&Database,delivery:&IncomingImDelivery)->Result<OnlinePosition,&'static str>{
        if !self.context.matches(delivery)||delivery.notice().notification_id().is_some(){return Err(SCOPE);}if !self.initial{return Ok(OnlinePosition::Pending);}
        let index=delivery.notice().key().index();if index<0{return Err(CURSOR);}let box_id=delivery.notice().key().box_id();
        db.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.context.validate(&tx)?;
            let saved:Option<i64>=tx.query_row("SELECT consumed FROM app_voice_im_receive_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4",
                params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id.bytes()],|r|r.get(0)).optional().map_err(|_|STORAGE)?;
            let consumed=if let Some(consumed)=saved{tx.execute("UPDATE app_voice_im_receive_cursors SET target=MAX(target,?5),observed_run=?6,observed_epoch=?7 WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4",
                params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id.bytes(),index,self.context.run.nonce(),self.context.binding.receipt_epoch()]).map_err(|_|STORAGE)?;consumed}
            else{let base=index.checked_sub(1).ok_or(CURSOR)?;self.limit_new(&tx)?;tx.execute("INSERT INTO app_voice_im_receive_cursors(line_id,revision,identity_key,box_id,baseline,consumed,target,status_time,observed_run,observed_epoch) VALUES(?1,?2,?3,?4,?5,?5,?6,?7,?8,?9)",
                params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id.bytes(),base,index,self.since,self.context.run.nonce(),self.context.binding.receipt_epoch()]).map_err(|_|STORAGE)?;base};
            let position=if index<=consumed{let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND message_index=?5)",params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),box_id.bytes(),index],|r|r.get(0)).map_err(|_|STORAGE)?;if exists{OnlinePosition::Replay}else{OnlinePosition::MissingOlder}}
                else if consumed.checked_add(1)==Some(index){OnlinePosition::Next}else{OnlinePosition::Pending};tx.commit().map_err(|_|STORAGE)?;Ok(position)})
    }
    pub(in super::super) fn received_span_for_online(&self,db:&Database,delivery:&IncomingImDelivery,proof:&CommittedIm)->Result<Option<GapCommitted>,&'static str>{
        if !self.initial||!self.context.matches(delivery)||delivery.notice().notification_id().is_some()||!proof.authorizes(delivery.ticket(),Instant::now()){return Err(SCOPE);}
        let key=delivery.notice().key();if key.index()<0{return Ok(None);}
        let span:Option<(i64,i64,Vec<u8>)>=db.with_conn(|conn|{self.context.validate(conn)?;conn.query_row("SELECT last_span_start,last_span_end,last_span_hash FROM app_voice_im_receive_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND consumed=?5 AND last_span_end=?5 AND observed_run=?6 AND observed_epoch=?7 AND last_span_hash IS NOT NULL",
            params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),key.box_id().bytes(),key.index(),self.context.run.nonce(),self.context.binding.receipt_epoch()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(|_|STORAGE)})?;
        span.map(|(start,end,hash)|{let span_hash=hash.try_into().map_err(|_|STORAGE)?;Ok(GapCommitted{binding:self.context.binding.clone(),gate:self.context.gate.clone(),run:self.context.run.clone(),box_id:Zeroizing::new(key.box_id().bytes().to_vec()),start,end,span_hash,proofs:vec![proof.captured_copy()]})}).transpose()
    }
    pub(in super::super) fn commit_online(&self,db:&Database,delivery:&IncomingImDelivery)->Result<Option<CommittedIm>,&'static str>{
        if !self.context.matches(delivery)||!self.initial||delivery.notice().notification_id().is_some(){return Err(SCOPE);}
        let message=delivery.notice().message().map_err(|_|INVALID)?.ok_or(INVALID)?;let record=Record::capture(&self.context.binding,delivery.notice().key().box_id(),&message)?;
        if delivery.notice().message_fingerprint()!=Some(&record.fingerprint){return Err(INVALID);}
        let(id,event)=db.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.context.validate(&tx)?;
            let(baseline,consumed):(i64,i64)=tx.query_row("SELECT baseline,consumed FROM app_voice_im_receive_cursors WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND observed_run=?5 AND observed_epoch=?6",
                params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),record.key.box_id().bytes(),self.context.run.nonce(),self.context.binding.receipt_epoch()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|CURSOR)?;
            if record.key.index()<=consumed{let existing:Option<(i64,Vec<u8>)>=tx.query_row("SELECT id,fingerprint FROM app_voice_im_messages WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND message_index=?5",params![self.context.run.line_id(),self.context.run.revision()as i64,self.context.binding.identity().as_slice(),record.key.box_id().bytes(),record.key.index()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|STORAGE)?;
                let Some((id,fingerprint))=existing else{return Ok((None,None));};if fingerprint.as_slice()!=record.fingerprint.as_slice(){return Err("app_account_im_message_conflict");}tx.commit().map_err(|_|STORAGE)?;return Ok((Some(id),None));}
            let span_hash=hash(&[b"original-im-online-span-v1\0",record.key.box_id().bytes(),&record.key.index().to_be_bytes(),&record.fingerprint]);let(id,event)=self.insert_record(&tx,&record)?;
            self.advance(&tx,record.key.box_id().bytes(),baseline,consumed,Gap{start:record.key.index(),count:1},record.key.index(),&span_hash)?;
            tx.commit().map_err(|_|STORAGE)?;Ok((Some(id),event))})?;
        if let Some(event)=event{db.publish_committed_app_event(event);}if !self.live(){return Err(SCOPE);}Ok(id.map(|id|self.context.permit(&record.key,record.fingerprint,id)))
    }
}
