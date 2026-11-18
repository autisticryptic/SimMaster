//! Explicit user read/open decision journal, distinct from received setRead.
//! Caller data contains only original account revision/local message ID/consent
//! nonce. Native binding and fresh target proof are obtained on original IO.
use super::*;
use super::super::{im_data::{ImBox,BoxKind},im_status::StatusLabel,rpc_im::{ImQuery,ImReplyData,OwnedImReply},im_user_read_rpc::OwnedUserReadReply};
use std::sync::{Mutex,Weak,atomic::{AtomicU8,Ordering}};
const QUEUED:u8=0;const WRITING:u8=1;const SUBMITTED:u8=2;const RESOLVED:u8=3;const RETIRED:u8=4;const CLOSED_WRITE:u8=5;
const UNCERTAIN:&str="app_account_im_user_read_uncertain";
const MAX_LOCAL:i64=128;const MAX_GLOBAL:i64=4096;
fn lock<T>(m:&Mutex<T>)->std::sync::MutexGuard<'_,T>{m.lock().unwrap_or_else(|e|e.into_inner())}
pub(in super::super) fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_im_user_reads(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        box_id BLOB NOT NULL CHECK(typeof(box_id)='blob' AND length(box_id) BETWEEN 1 AND 512),
        message_id INTEGER NOT NULL CHECK(message_id>0),target_index INTEGER NOT NULL CHECK(typeof(target_index)='integer' AND target_index>=0),
        fingerprint BLOB NOT NULL CHECK(typeof(fingerprint)='blob' AND length(fingerprint)=32),
        user_nonce TEXT NOT NULL CHECK(length(user_nonce)=32),epoch_ms INTEGER NOT NULL CHECK(epoch_ms>=0),
        run_nonce TEXT NOT NULL,scope_epoch TEXT NOT NULL,state INTEGER NOT NULL CHECK(state IN(0,1,2,3)),
        request_id INTEGER CHECK(request_id>0),result_code INTEGER,updated_at TEXT NOT NULL,
        UNIQUE(line_id,user_nonce));
        CREATE INDEX IF NOT EXISTS idx_im_user_read_context ON app_voice_im_user_reads(line_id,revision,identity_key,box_id,state);") .map_err(|_|STORAGE)?;
    // A nullable private intent hash keeps all historical UID-only rows exact.
    // No URI/token/route is stored or exported, and metadata never clears unknown.
    let mut fields=conn.prepare("PRAGMA table_info(app_voice_im_user_reads)").map_err(|_|STORAGE)?;
    let names=fields.query_map([],|r|r.get::<_,String>(1)).map_err(|_|STORAGE)?.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;drop(fields);
    if !names.iter().any(|name|name=="recipient_hash"){conn.execute("ALTER TABLE app_voice_im_user_reads ADD COLUMN recipient_hash BLOB CHECK(recipient_hash IS NULL OR(typeof(recipient_hash)='blob' AND length(recipient_hash)=32))",[]).map_err(|_|STORAGE)?;}
    Ok(())
}
pub(in super::super) fn delete_account(conn:&Connection,line:&str)->Result<(),&'static str>{
    if account_store::metadata(conn,line)?.is_some(){return Err(SCOPE);}
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_im_user_reads')",[],|r|r.get(0)).map_err(|_|STORAGE)?;
    if exists{conn.execute("DELETE FROM app_voice_im_user_reads WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}Ok(())
}
#[derive(Serialize)]
pub struct UserReadView{pub id:i64,pub message_id:i64,pub target_index:String,pub state:&'static str,pub updated_at:String}
impl fmt::Debug for UserReadView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("UserReadView(<redacted>)")}}
fn state_name(state:i64)->&'static str{match state{0=>"uncertain",1=>"confirmed",2=>"rejected",3=>"not_sent",_=>"unknown"}}
pub(in super::super) fn metadata(conn:&Connection,line:&str,limit:usize)->Result<Vec<UserReadView>,&'static str>{
    if !(1..=128).contains(&limit)||account_store::metadata(conn,line)?.is_none(){return Err(INVALID);}
    let mut q=conn.prepare("SELECT id,message_id,target_index,state,updated_at FROM app_voice_im_user_reads WHERE line_id=?1 AND typeof(updated_at)='text' AND length(CAST(updated_at AS BLOB))<=64 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
    let rows=q.query_map(params![line,limit as i64],|r|Ok(UserReadView{id:r.get(0)?,message_id:r.get(1)?,target_index:r.get::<_,i64>(2)?.to_string(),state:state_name(r.get(3)?),updated_at:r.get(4)?})).map_err(|_|STORAGE)?;
    rows.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)
}
pub(in super::super) struct UserReadAction{
    database:Arc<Database>,run:Arc<RunToken>,parent_gate:Arc<ImReceiptGate>,gate:Arc<ImReceiptGate>,owner:AttemptOwner,scope:RpcConnectionScope,
    message_id:i64,nonce:Zeroizing<String>,deadline:Instant,epoch_ms:i64,permit:Mutex<Weak<UserReadPermit>>,recipient:Option<Zeroizing<Vec<u8>>>,
    #[cfg(test)]
    target_submitted:std::sync::atomic::AtomicBool,
}
impl fmt::Debug for UserReadAction{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalUserReadAction(<redacted>)")}}
impl UserReadAction{
    /// The account manager calls only after explicit consent/current Hosted
    /// resource owner; this snapshot alone grants no remote write.
    pub(in super::super) fn original(database:Arc<Database>,run:Arc<RunToken>,parent_gate:Arc<ImReceiptGate>,owner:AttemptOwner,scope:RpcConnectionScope,message_id:i64,nonce:&str,deadline:Instant)->Result<Arc<Self>,&'static str>{
        if message_id<=0||nonce.len()!=32||!nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||owner.line_id()!=run.line_id()
            ||!scope.is_open()||scope.session_revoked()||!parent_gate.live()||Instant::now()>=deadline{return Err("app_account_im_user_read_request_invalid");}
        let epoch_ms=chrono::Utc::now().timestamp_millis();if epoch_ms<0{return Err(SCOPE);}
        Ok(Arc::new(Self{database,run,parent_gate,gate:ImReceiptGate::fresh(),owner,scope,message_id,nonce:Zeroizing::new(nonce.to_owned()),deadline,epoch_ms,permit:Mutex::new(Weak::new()),recipient:None,
            #[cfg(test)]
            target_submitted:std::sync::atomic::AtomicBool::new(false),
        }))
    }
    pub(in super::super) fn original_with_recipient(database:Arc<Database>,run:Arc<RunToken>,parent_gate:Arc<ImReceiptGate>,owner:AttemptOwner,scope:RpcConnectionScope,message_id:i64,nonce:&str,deadline:Instant,recipient:&[u8])->Result<Arc<Self>,&'static str>{
        let parsed=super::super::user_entry_data::RecipientUri::parse(recipient).map_err(|_|"app_account_im_user_read_request_invalid")?;
        if parsed.kind()==super::super::user_entry_data::AidKind::Uid{return Err("app_account_im_user_read_request_invalid");}
        let mut action=Self::original(database,run,parent_gate,owner,scope,message_id,nonce,deadline)?;
        Arc::get_mut(&mut action).ok_or(SCOPE)?.recipient=Some(Zeroizing::new(recipient.to_vec()));Ok(action)
    }
    pub(in super::super) fn recipient(&self)->Option<&[u8]>{self.recipient.as_deref().map(|s|s.as_slice())}
    fn recipient_hash(&self)->Option<[u8;32]>{self.recipient().map(|uri|{let mut h=ring::digest::Context::new(&ring::digest::SHA256);h.update(b"original-im-user-read-recipient-v1\0");h.update(uri);h.finish().as_ref().try_into().expect("SHA256 width")})}
    #[cfg(test)]
    pub(in super::super) fn note_target_submitted(&self){self.target_submitted.store(true,Ordering::SeqCst);}
    #[cfg(test)]
    pub(in super::super) fn target_was_submitted(&self)->bool{self.target_submitted.load(Ordering::SeqCst)}
    pub(in super::super) fn owner(&self)->&AttemptOwner{&self.owner}
    pub(in super::super) fn scope(&self)->RpcConnectionScope{self.scope.clone()}
    pub(in super::super) fn deadline(&self)->Instant{self.deadline}
    pub(in super::super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(in super::super) fn parent_gate(&self)->Arc<ImReceiptGate>{self.parent_gate.clone()}
    pub(in super::super) fn live(&self)->bool{self.parent_gate.live()&&self.gate.live()&&self.scope.is_open()&&!self.scope.session_revoked()&&Instant::now()<self.deadline}
    pub(in super::super) fn cancel(&self){self.gate.retire();if let Some(p)=lock(&self.permit).upgrade(){p.cancel();}}
    pub(in super::super) fn bind_current(self:&Arc<Self>,binding:Arc<ImBinding>)->Result<BoundUserRead,&'static str>{
        if !self.live()||!binding.matches(&self.owner,&self.scope){return Err(SCOPE);}
        let context=ImInboxScope::original(self.run.clone(),binding.clone(),self.parent_gate.clone())?;
        let(box_id,index,fingerprint,replay)=self.database.with_conn(|conn|{
            let tx=Transaction::new_unchecked(conn,TransactionBehavior::Deferred).map_err(|_|STORAGE)?;context.validate(&tx)?;
            let(box_id,index,fingerprint,deleted):(Vec<u8>,i64,Vec<u8>,i64)=tx.query_row("SELECT box_id,message_index,fingerprint,deleted FROM app_voice_im_messages WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4",
                params![self.message_id,self.run.line_id(),self.run.revision()as i64,binding.identity().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_|STORAGE)?.ok_or("app_account_im_user_read_message_missing")?;
            if deleted!=0||index<0{return Err("app_account_im_user_read_message_missing");}
            let key=ImBox::parse(&box_id).map_err(|_|INVALID)?;if key.kind()!=BoxKind::P2P{return Err("app_account_im_user_read_recipient_unsupported");}
            let fingerprint:[u8;32]=fingerprint.try_into().map_err(|_|STORAGE)?;
            let old:Option<(i64,i64,i64,i64,Vec<u8>,Vec<u8>,String,Vec<u8>)>=tx.query_row("SELECT id,revision,message_id,target_index,identity_key,fingerprint,updated_at,box_id FROM app_voice_im_user_reads WHERE line_id=?1 AND user_nonce=?2",
                params![self.run.line_id(),self.nonce.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional().map_err(|_|STORAGE)?;
            let replay=if let Some((id,revision,message_id,target,identity,saved,updated,saved_box))=old{
                let saved_recipient:Option<Vec<u8>>=tx.query_row("SELECT recipient_hash FROM app_voice_im_user_reads WHERE id=?1",[id],|r|r.get(0)).map_err(|_|STORAGE)?;
                let current_recipient=self.recipient_hash();if saved_recipient.as_deref()!=current_recipient.as_ref().map(|h|h.as_slice()){return Err("app_account_im_user_read_nonce_conflict");}
                if revision!=self.run.revision()as i64||message_id!=self.message_id||target!=index||identity.as_slice()!=binding.identity().as_slice()||saved.as_slice()!=fingerprint.as_slice()||saved_box.as_slice()!=box_id.as_slice(){return Err("app_account_im_user_read_nonce_conflict");}
                let state:i64=tx.query_row("SELECT state FROM app_voice_im_user_reads WHERE id=?1",[id],|r|r.get(0)).map_err(|_|STORAGE)?;Some(UserReadView{id,message_id,target_index:index.to_string(),state:state_name(state),updated_at:updated})
            }else{
                let unknown:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_user_reads WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND state=0)",
                    params![self.run.line_id(),self.run.revision()as i64,binding.identity().as_slice(),box_id.as_slice()],|r|r.get(0)).map_err(|_|STORAGE)?;
                if unknown{return Err(UNCERTAIN);}None
            };
            if !self.live(){return Err(SCOPE);}tx.commit().map_err(|_|STORAGE)?;Ok((Zeroizing::new(box_id),index,fingerprint,replay))
        })?;
        if let Some(view)=replay{return Ok(BoundUserRead::Replay(view));}
        let label=StatusLabel::parse(&box_id[4..]).map_err(|_|INVALID)?;let query=Arc::new(ImQuery::refresh(&[label],-1).map_err(|_|INVALID)?);
        let recipient_action=self.recipient().map(|uri|super::super::user_entry_action::UserEntryAction::original(self.owner.clone(),self.scope.clone(),self.gate.clone(),self.deadline,uri)).transpose()?;
        Ok(BoundUserRead::Context(UserReadContext{action:self.clone(),context,box_id,index,fingerprint,query,recipient_action,recipient_reply:None}))
    }
}
pub(in super::super) enum BoundUserRead{Replay(UserReadView),Context(UserReadContext)}
pub(in super::super) struct UserReadContext{action:Arc<UserReadAction>,context:ImInboxScope,box_id:Zeroizing<Vec<u8>>,index:i64,fingerprint:[u8;32],query:Arc<ImQuery>,
    recipient_action:Option<Arc<super::super::user_entry_action::UserEntryAction>>,recipient_reply:Option<Arc<super::super::user_entry_rpc::OwnedUserEntryReply>>,
}
impl fmt::Debug for UserReadContext{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CheckedUserReadContext(<redacted>)")}}
impl UserReadContext{
    pub(in super::super) fn query(&self)->Arc<ImQuery>{self.query.clone()}
    pub(in super::super) fn recipient_action(&self)->Option<Arc<super::super::user_entry_action::UserEntryAction>>{self.recipient_action.clone()}
    pub(in super::super) fn confirm_recipient(mut self,reply:super::super::user_entry_rpc::OwnedUserEntryReply)->Result<Self,&'static str>{
        let action=self.recipient_action.as_ref().ok_or(SCOPE)?;
        if !self.action.live()||reply.resolved_uid_for(action)!=Some(&self.box_id[4..]){return Err("app_account_im_user_read_recipient_unconfirmed");}
        self.action.database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Deferred).map_err(|_|STORAGE)?;self.verify_message(&tx,true)?;tx.commit().map_err(|_|STORAGE)})?;
        self.recipient_reply=Some(Arc::new(reply));Ok(self)
    }
    fn verify_message(&self,conn:&Connection,visible:bool)->Result<(),&'static str>{
        self.context.validate(conn)?;if !self.action.live(){return Err(SCOPE);}
        let matches:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_messages WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4 AND box_id=?5 AND message_index=?6 AND fingerprint=?7 AND (?8=0 OR deleted=0))",
            params![self.action.message_id,self.action.run.line_id(),self.action.run.revision()as i64,self.context.binding.identity().as_slice(),self.box_id.as_slice(),self.index,self.fingerprint.as_slice(),i64::from(visible)],|r|r.get(0)).map_err(|_|STORAGE)?;
        if !matches{return Err("app_account_im_user_read_message_conflict");}
        let saved:Option<Option<Vec<u8>>>=conn.query_row("SELECT recipient_hash FROM app_voice_im_user_reads WHERE line_id=?1 AND user_nonce=?2",params![self.action.run.line_id(),self.action.nonce.as_str()],|r|r.get(0)).optional().map_err(|_|STORAGE)?;
        if let Some(saved)=saved{let expected=self.action.recipient_hash();if saved.as_deref()!=expected.as_ref().map(|h|h.as_slice()){return Err("app_account_im_user_read_nonce_conflict");}}
        Ok(())
    }
    pub(in super::super) fn admit(self,reply:&OwnedImReply)->Result<Arc<UserReadPermit>,&'static str>{
        if self.action.recipient().is_some()&&!self.recipient_action.as_ref().is_some_and(|action|self.recipient_reply.as_ref().is_some_and(|resolved|resolved.resolved_uid_for(action)==Some(&self.box_id[4..]))){return Err("app_account_im_user_read_recipient_unconfirmed");}
        if !self.action.live()||!reply.matches(self.action.owner(),&self.action.scope())||!Arc::ptr_eq(reply.request().binding(),&self.context.binding)
            ||!Arc::ptr_eq(reply.request().query_arc(),&self.query)||!Arc::ptr_eq(&reply.request().gate(),&self.action.gate()){return Err(SCOPE);}
        let ImReplyData::Refresh(status)=reply.result().map_err(|_|INVALID)?else{return Err(SCOPE);};
        if !status.accepted||!status.entries.iter().any(|e|e.label.bytes()==&self.box_id[4..]&&e.latest.is_some_and(|n|n>=self.index)){return Err("app_account_im_user_read_target_unconfirmed");}
        let row=self.action.database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.verify_message(&tx,true)?;
            let unknown:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_user_reads WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND box_id=?4 AND state=0)",params![self.action.run.line_id(),self.action.run.revision()as i64,self.context.binding.identity().as_slice(),self.box_id.as_slice()],|r|r.get(0)).map_err(|_|STORAGE)?;
            if unknown{return Err(UNCERTAIN);}
            let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_im_user_reads",[self.action.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;if local>=MAX_LOCAL||total>=MAX_GLOBAL{return Err("app_account_im_user_read_full");}
            let recipient_hash=self.action.recipient_hash();
            tx.execute("INSERT INTO app_voice_im_user_reads(line_id,revision,identity_key,box_id,message_id,target_index,fingerprint,user_nonce,epoch_ms,run_nonce,scope_epoch,state,updated_at,recipient_hash) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,?12,?13)",
                params![self.action.run.line_id(),self.action.run.revision()as i64,self.context.binding.identity().as_slice(),self.box_id.as_slice(),self.action.message_id,self.index,self.fingerprint.as_slice(),self.action.nonce.as_str(),self.action.epoch_ms,self.action.run.nonce(),self.context.binding.receipt_epoch(),chrono::Utc::now().to_rfc3339(),recipient_hash.as_ref().map(|h|h.as_slice())]).map_err(|_|STORAGE)?;
            let id=tx.last_insert_rowid();if !self.action.live(){return Err(SCOPE);}tx.commit().map_err(|_|STORAGE)?;Ok(id)
        })?;
        let p=Arc::new(UserReadPermit{context:self,row,phase:AtomicU8::new(QUEUED),request:Mutex::new(None),changed:tokio::sync::Notify::new()});
        *lock(&p.context.action.permit)=Arc::downgrade(&p);if !p.context.action.live(){p.cancel();return Err(SCOPE);}Ok(p)
    }
}
struct UserRequest{id:i32,deadline:Instant}
pub(in super::super) struct UserReadPermit{context:UserReadContext,row:i64,phase:AtomicU8,request:Mutex<Option<UserRequest>>,changed:tokio::sync::Notify}
impl fmt::Debug for UserReadPermit{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("OriginalUserReadPermit(<redacted>)")}}
impl UserReadPermit{
    pub(in super::super) fn binding(&self)->&Arc<ImBinding>{&self.context.context.binding}
    pub(in super::super) fn gate(&self)->Arc<ImReceiptGate>{self.context.action.gate()}
    pub(in super::super) fn parent_gate(&self)->Arc<ImReceiptGate>{self.context.action.parent_gate()}
    pub(in super::super) fn deadline(&self)->Instant{self.context.action.deadline()}
    pub(in super::super) fn label(&self)->&[u8]{&self.context.box_id[4..]}
    pub(in super::super) fn index(&self)->i64{self.context.index}
    pub(in super::super) fn epoch_ms(&self)->i64{self.context.action.epoch_ms}
    pub(in super::super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.context.action.live()&&self.binding().matches(owner,scope)}
    pub(in super::super) fn arm(&self,id:i32,deadline:Instant)->Result<(),&'static str>{
        let deadline=deadline.min(self.deadline());if id<=0||!self.context.action.live()||self.phase.load(Ordering::SeqCst)!=QUEUED{return Err(SCOPE);}let mut request=lock(&self.request);if request.is_some(){return Err(SCOPE);}
        self.context.action.database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.context.verify_message(&tx,true)?;
            let n=tx.execute("UPDATE app_voice_im_user_reads SET request_id=?3 WHERE id=?1 AND user_nonce=?2 AND state=0 AND request_id IS NULL",params![self.row,self.context.action.nonce.as_str(),id]).map_err(|_|STORAGE)?;
            if n!=1||Instant::now()>=deadline||!self.context.action.live(){return Err(SCOPE);}tx.commit().map_err(|_|STORAGE)
        })?;if Instant::now()>=deadline||!self.context.action.live(){return Err(SCOPE);}*request=Some(UserRequest{id,deadline});Ok(())
    }
    pub(in super::super) fn current(&self,id:i32)->bool{self.context.action.live()&&lock(&self.request).as_ref().is_some_and(|r|r.id==id&&Instant::now()<r.deadline)}
    pub(in super::super) fn begin_write(&self,id:i32)->bool{self.current(id)&&self.phase.compare_exchange(QUEUED,WRITING,Ordering::SeqCst,Ordering::SeqCst).is_ok()}
    #[cfg(test)]
    pub(in super::super) fn write_was_submitted(&self)->bool{self.phase.load(Ordering::SeqCst)==SUBMITTED}
    pub(in super::super) fn submitted(&self)->bool{self.phase.compare_exchange(WRITING,SUBMITTED,Ordering::SeqCst,Ordering::SeqCst).is_ok()}
    pub(in super::super) fn retired(&self)->bool{self.phase.load(Ordering::SeqCst)>=RETIRED}
    /// Only an original possibly submitted WRITE cancellation retires IO;
    /// queued no-IO cancellation must not alter a later healthy close reason.
    pub(in super::super) fn closed_original_io(&self)->bool{self.phase.load(Ordering::SeqCst)==CLOSED_WRITE}
    pub(in super::super) fn cancel(&self){
        let before=loop{let p=self.phase.load(Ordering::SeqCst);if p>=RESOLVED{return;}let retired=if p==QUEUED{RETIRED}else{CLOSED_WRITE};if self.phase.compare_exchange(p,retired,Ordering::SeqCst,Ordering::SeqCst).is_ok(){break p;}};
        if before==QUEUED{let _=self.context.action.database.with_conn(|c|c.execute("UPDATE app_voice_im_user_reads SET state=3,updated_at=?3 WHERE id=?1 AND user_nonce=?2 AND state=0",params![self.row,self.context.action.nonce.as_str(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE));}
        else if matches!(before,WRITING|SUBMITTED){self.context.action.scope.close();}self.changed.notify_waiters();
    }
    pub(in super::super) async fn cancelled(&self){loop{let n=self.changed.notified();tokio::pin!(n);n.as_mut().enable();if self.retired(){return;}n.await;}}
    pub(in super::super) fn resolve(&self,reply:&OwnedUserReadReply)->Result<UserReadView,&'static str>{
        let id=reply.request().id();if !reply.belongs_to(self)||!self.current(id)||self.phase.load(Ordering::SeqCst)!=SUBMITTED{return Err(UNCERTAIN);}
        let yes=reply.accepted().map_err(|_|UNCERTAIN)?;let at=chrono::Utc::now().to_rfc3339();
        self.context.action.database.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.context.verify_message(&tx,false)?;
            let n=tx.execute("UPDATE app_voice_im_user_reads SET state=?4,result_code=?5,updated_at=?6 WHERE id=?1 AND user_nonce=?2 AND request_id=?3 AND state=0 AND run_nonce=?7 AND scope_epoch=?8 AND fingerprint=?9",
                params![self.row,self.context.action.nonce.as_str(),id,if yes{1}else{2},i64::from(reply.code()),at,self.context.action.run.nonce(),self.context.context.binding.receipt_epoch(),self.context.fingerprint.as_slice()]).map_err(|_|STORAGE)?;
            if n!=1||!self.current(id)||self.phase.load(Ordering::SeqCst)!=SUBMITTED{return Err(UNCERTAIN);}tx.commit().map_err(|_|STORAGE)
        })?;
        self.phase.compare_exchange(SUBMITTED,RESOLVED,Ordering::SeqCst,Ordering::SeqCst).map_err(|_|UNCERTAIN)?;
        Ok(UserReadView{id:self.row,message_id:self.context.action.message_id,target_index:self.context.index.to_string(),state:if yes{"confirmed"}else{"rejected"},updated_at:at})
    }
}
pub(in super::super) struct UserReadPermitWait{permit:Arc<UserReadPermit>}
impl UserReadPermitWait{pub(in super::super) fn original(permit:Arc<UserReadPermit>)->Self{Self{permit}}}
impl Drop for UserReadPermitWait{fn drop(&mut self){self.permit.cancel();}}
pub(in super::super) struct UserReadWait{action:Arc<UserReadAction>}
impl UserReadWait{pub(in super::super) fn original(action:Arc<UserReadAction>)->Self{Self{action}}}
impl Drop for UserReadWait{fn drop(&mut self){self.action.cancel();}}
