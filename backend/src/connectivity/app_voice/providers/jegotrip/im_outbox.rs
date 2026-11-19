//! Original userSend intent/uncertainty journal, independent of received IM,
//! user UDMR, SMS, media and notification delivery. SQL rows are DATA only.
//! A fresh original current SDK identity + explicit business action owns work.
use super::*;
use super::super::{arc_session::ImSendIdentity,im_send_data::{self,OutgoingMessage,SendKind},im_send_policy::NativeImSendConfig,user_entry_action::UserEntryAction,user_entry_rpc::OwnedUserEntryReply};
use ring::digest::{Context,SHA256};
use super::super::rpc_stream::RpcProperty;
use std::sync::{Mutex,Weak,atomic::{AtomicI64,AtomicU8}};
const UNCERTAIN:&str="app_account_im_send_uncertain";
const INVALID_SEND:&str="app_account_im_send_request_invalid";
const SEND_SCOPE:&str="app_account_im_send_scope_invalid";
const QUEUED:u8=0;const WRITING:u8=1;const SUBMITTED:u8=2;const RESOLVED:u8=3;const RETIRED:u8=4;const CLOSED_WRITE:u8=5;
fn lock<T>(m:&Mutex<T>)->std::sync::MutexGuard<'_,T>{m.lock().unwrap_or_else(|e|e.into_inner())}
fn digest(parts:&[&[u8]])->[u8;32]{let mut h=Context::new(&SHA256);for p in parts{h.update(&(p.len()as u64).to_be_bytes());h.update(p);}let mut out=[0;32];out.copy_from_slice(h.finish().as_ref());out}
pub(in super::super) fn initialize_schema(c:&Connection)->Result<(),&'static str>{c.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_im_outbox(id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),user_nonce TEXT NOT NULL CHECK(length(user_nonce)=32),intent_hash BLOB NOT NULL CHECK(typeof(intent_hash)='blob' AND length(intent_hash)=32),box_id BLOB CHECK(box_id IS NULL OR(typeof(box_id)='blob' AND length(box_id) BETWEEN 1 AND 512)),arguments_hash BLOB CHECK(arguments_hash IS NULL OR(typeof(arguments_hash)='blob' AND length(arguments_hash)=32)),message_kind TEXT NOT NULL CHECK(message_kind IN('text','info')),run_nonce TEXT NOT NULL,scope_epoch TEXT NOT NULL,state INTEGER NOT NULL CHECK(state IN(0,1,2,3)),request_id INTEGER CHECK(request_id>0),result_code INTEGER,message_index INTEGER,original_time INTEGER,updated_at TEXT NOT NULL,UNIQUE(line_id,user_nonce));").map_err(|_|STORAGE)}
pub(in super::super) fn delete_account(c:&Connection,line:&str)->Result<(),&'static str>{if account_store::metadata(c,line)?.is_some(){return Err(SEND_SCOPE);}let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_im_outbox')",[],|r|r.get(0)).map_err(|_|STORAGE)?;if exists{c.execute("DELETE FROM app_voice_im_outbox WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}Ok(())}
struct OwnedField{key:Zeroizing<Vec<u8>>,value:Zeroizing<Vec<u8>>}
impl OwnedField{fn capture(p:&RpcProperty<'_>)->Self{Self{key:Zeroizing::new(p.key.to_vec()),value:Zeroizing::new(p.value.to_vec())}}}
fn borrow_fields(fields:&[OwnedField])->Vec<RpcProperty<'_>>{fields.iter().map(|p|RpcProperty{key:&p.key,value:&p.value}).collect()}
pub struct ImSendMessage{kind:Zeroizing<Vec<u8>>,content:Zeroizing<Vec<u8>>,attachments:Vec<OwnedField>,parameters:Vec<OwnedField>,notifications:Option<Vec<OwnedField>>,notification_digest:Option<Zeroizing<Vec<u8>>>}
impl fmt::Debug for ImSendMessage{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSendMessage(<redacted>)")}}
impl ImSendMessage{
    /// Plain user body DATA only; active original SDK/policy/consent still required.
    pub fn text(content:&str)->Result<Self,&'static str>{Self::capture(&OutgoingMessage{kind:SendKind::Text,content:content.as_bytes(),attachments:&[],parameters:&[]},None)}
    pub fn info(subtype:&str,content:&str)->Result<Self,&'static str>{Self::capture(&OutgoingMessage{kind:SendKind::Info(subtype.as_bytes()),content:content.as_bytes(),attachments:&[],parameters:&[]},None)}
    /// Original MtcImDigestKey is notification display DATA, NOT a crypto key
    /// or SDK/WRITE grant. Only Text defaults can use it; an explicit override
    /// is a conflicting source, never silently mixed/rebound into permission.
    pub fn with_notification_digest(mut self,value:&str)->Result<Self,&'static str>{if self.kind.as_slice()!=b"Text"||self.notifications.is_some()||value.len()>65536||value.as_bytes().contains(&0){return Err(INVALID_SEND);}self.notification_digest=Some(Zeroizing::new(value.as_bytes().to_vec()));Ok(self)}
    pub fn components(info_type:Option<&str>,content:&str,attachments:&[(Vec<u8>,Vec<u8>)],parameters:&[(Vec<u8>,Vec<u8>)],notifications:Option<&[(Vec<u8>,Vec<u8>)]>)->Result<Self,&'static str>{if attachments.len()>128||parameters.len()>128||notifications.is_some_and(|v|v.len()>128){return Err(INVALID_SEND);}let a=attachments.iter().map(|(key,value)|RpcProperty{key,value}).collect::<Vec<_>>();let p=parameters.iter().map(|(key,value)|RpcProperty{key,value}).collect::<Vec<_>>();let n=notifications.map(|v|v.iter().map(|(key,value)|RpcProperty{key,value}).collect::<Vec<_>>());let kind=info_type.map_or(SendKind::Text,|value|SendKind::Info(value.as_bytes()));Self::capture(&OutgoingMessage{kind,content:content.as_bytes(),attachments:&a,parameters:&p},n.as_deref())}
    pub(in super::super) fn capture(message:&OutgoingMessage<'_>,notifications:Option<&[RpcProperty<'_>]>)->Result<Self,&'static str>{
        if message.content.is_empty()||message.content.len()>65536{return Err(INVALID_SEND);}
        // Parent-compatible pure input bounds. Actual derived notification
        // duplicates are validated BEFORE a fresh permit is retained/armed;
        // they cannot preempt metadata-only replay of an existing legacy nonce.
        im_send_data::validate_user_send_capture(message,notifications).map_err(|_|INVALID_SEND)?;
        if message.parameters.iter().any(|p|p.key==b"account")||notifications.is_some_and(|v|v.iter().any(|p|p.key.starts_with(b"Statistics."))){return Err(INVALID_SEND);}
        let mut kind=Zeroizing::new(Vec::with_capacity(69));match &message.kind{SendKind::Text=>kind.extend_from_slice(b"Text"),SendKind::Info(s)=>{kind.extend_from_slice(b"Info.");kind.extend_from_slice(s);}}
        Ok(Self{kind,content:Zeroizing::new(message.content.to_vec()),attachments:message.attachments.iter().map(OwnedField::capture).collect(),parameters:message.parameters.iter().map(OwnedField::capture).collect(),notifications:notifications.map(|v|v.iter().map(OwnedField::capture).collect()),notification_digest:None})
    }
    fn kind_name(&self)->&'static str{if self.kind.as_slice()==b"Text"{"text"}else{"info"}}
    fn bytes(&self)->usize{let fields=|v:&Vec<OwnedField>|v.capacity()*std::mem::size_of::<OwnedField>()+v.iter().map(|p|p.key.capacity()+p.value.capacity()).sum::<usize>();std::mem::size_of::<Self>()+self.kind.capacity()+self.content.capacity()+fields(&self.attachments)+fields(&self.parameters)+self.notifications.as_ref().map_or(0,fields)+self.notification_digest.as_ref().map_or(0,|value|value.capacity())}
    /// Accounting DATA only. Reserve from the current original scope before
    /// the controller/auth awaits; a caller cannot turn this into a permit.
    pub(in super::super) fn reserve_input(&self,scope:&RpcConnectionScope,recipient:&[u8],nonce:&str)->Result<ImSendInput,&'static str>{
        if recipient.is_empty()||recipient.len()>512||nonce.len()!=32||!nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)){return Err(INVALID_SEND);}
        let group=Zeroizing::new([b"Org/".as_slice(),recipient].concat());
        if super::super::im_data::ImBox::parse(&group).is_err(){super::super::user_entry_data::RecipientUri::parse(recipient).map_err(|_|INVALID_SEND)?;}
        let bytes=self.bytes().checked_add(recipient.len()*3).and_then(|n|n.checked_add(nonce.len()*2+768)).ok_or("app_account_im_send_capacity")?;
        let budget=scope.reserve_native_rpc(bytes).map_err(|_|"app_account_im_send_capacity")?;
        Ok(ImSendInput{budget,fingerprint:digest(&[b"original-user-send-input-v1\0",&self.fingerprint(),recipient,nonce.as_bytes()])})
    }
    fn fingerprint(&self)->[u8;32]{let mut h=Context::new(&SHA256);h.update(b"original-im-send-body-v1\0");for value in [&*self.kind,&*self.content]{h.update(&(value.len()as u64).to_be_bytes());h.update(value);}for fields in [&self.attachments,&self.parameters]{let mut sorted=fields.iter().collect::<Vec<_>>();sorted.sort_by(|a,b|a.key.len().cmp(&b.key.len()).then(a.key.cmp(&b.key)));h.update(&(sorted.len()as u64).to_be_bytes());for p in sorted{h.update(&(p.key.len()as u64).to_be_bytes());h.update(&p.key);h.update(&(p.value.len()as u64).to_be_bytes());h.update(&p.value);}}h.update(&[u8::from(self.notifications.is_some())]);if let Some(fields)=&self.notifications{let mut sorted=fields.iter().collect::<Vec<_>>();sorted.sort_by(|a,b|a.key.len().cmp(&b.key.len()).then(a.key.cmp(&b.key)));for p in sorted{h.update(&(p.key.len()as u64).to_be_bytes());h.update(&p.key);h.update(&(p.value.len()as u64).to_be_bytes());h.update(&p.value);}}if let Some(value)=&self.notification_digest{h.update(b"original-im-notification-digest-v1\0");h.update(&(value.len()as u64).to_be_bytes());h.update(value);}let mut out=[0;32];out.copy_from_slice(h.finish().as_ref());out}
    fn arguments(&self,box_id:&[u8],identity:&ImSendIdentity,owner:&AttemptOwner,scope:&RpcConnectionScope,policy:&NativeImSendConfig)->Result<Zeroizing<Vec<u8>>,&'static str>{let kind=if self.kind.as_slice()==b"Text"{if !policy.send_text{return Err("app_account_im_send_not_installed");}SendKind::Text}else{if !policy.send_info{return Err("app_account_im_send_not_installed");}SendKind::Info(&self.kind[5..])};let attachments=borrow_fields(&self.attachments);let parameters=borrow_fields(&self.parameters);let message=OutgoingMessage{kind,content:&self.content,attachments:&attachments,parameters:&parameters};let country=policy.auth_country_code.as_deref().unwrap_or("").as_bytes();let statistics=identity.statistics_for(owner,scope,country).ok_or(SEND_SCOPE)?;let mut notifications=if let Some(fields)=&self.notifications{borrow_fields(fields)}else{im_send_data::default_user_send_notifications(&message,self.notification_digest.as_deref().map(|value|value.as_slice())).map_err(|_|INVALID_SEND)?};notifications.extend_from_slice(&statistics);let uri=identity.uri_for(owner,scope).ok_or(SEND_SCOPE)?;let bytes=im_send_data::encode_user_send(super::super::im_data::ImBox::parse(box_id).map_err(|_|INVALID_SEND)?,&message,uri,Some(&notifications)).map_err(|_|INVALID_SEND)?;Ok(Zeroizing::new(bytes.expose_to_transport().to_vec()))}
}
/// Finite stage observation, not a remote WRITE result or authorization.
/// Only the preparing original path can report that no NEW intent was admitted.
pub struct ImSendFailure{code:&'static str,not_admitted:bool}
impl fmt::Debug for ImSendFailure{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSendFailure(<redacted>)")}}
impl ImSendFailure{
    pub fn code(&self)->&'static str{self.code}
    pub fn not_admitted(&self)->bool{self.not_admitted}
    pub(in super::super) fn before(code:&'static str)->Self{Self{code,not_admitted:true}}
    pub(in super::super) fn after(code:&'static str)->Self{Self{code,not_admitted:false}}
}
pub(in super::super) struct ImSendInput{budget:Arc<super::super::native_rpc_budget::NativeRpcReservation>,fingerprint:[u8;32]}
impl ImSendInput{fn matches(&self,scope:&RpcConnectionScope,message:&ImSendMessage,recipient:&[u8],nonce:&str)->bool{self.budget.belongs_to(scope)&&self.fingerprint==digest(&[b"original-user-send-input-v1\0",&message.fingerprint(),recipient,nonce.as_bytes()])}}
struct PermitSlot{cancelled:bool,permit:Weak<ImSendPermit>}
pub(in super::super) enum PreparedImSend{Fresh(Arc<ImSendAction>),Replay(ImSendView)}
pub(in super::super) struct ImSendAction{database:Arc<Database>,context:ImInboxScope,identity:Arc<ImSendIdentity>,recipient:Zeroizing<Vec<u8>>,fixed_box:Option<Zeroizing<Vec<u8>>>,lookup:Option<Arc<UserEntryAction>>,lookup_reply:Mutex<Option<Arc<OwnedUserEntryReply>>>,message:ImSendMessage,nonce:Zeroizing<String>,intent_hash:[u8;32],row:AtomicI64,gate:Arc<ImReceiptGate>,deadline:Instant,permit:Mutex<PermitSlot>,_input_budget:ImSendInput}
impl fmt::Debug for ImSendAction{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSendAction(<redacted>)")}}
impl ImSendAction{
    #[cfg(test)]
    pub(in super::super) fn original(database:Arc<Database>,run:Arc<RunToken>,parent:Arc<ImReceiptGate>,binding:Arc<ImBinding>,identity:Arc<ImSendIdentity>,recipient:&[u8],message:ImSendMessage,nonce:&str,deadline:Instant,policy:&NativeImSendConfig)->Result<PreparedImSend,&'static str>{
        let input=message.reserve_input(&binding.scope(),recipient,nonce)?;
        Self::original_with_input(database,run,parent,binding,identity,recipient,message,nonce,deadline,policy,input)
    }
    pub(in super::super) fn original_with_input(database:Arc<Database>,run:Arc<RunToken>,parent:Arc<ImReceiptGate>,binding:Arc<ImBinding>,identity:Arc<ImSendIdentity>,recipient:&[u8],message:ImSendMessage,nonce:&str,deadline:Instant,policy:&NativeImSendConfig,input:ImSendInput)->Result<PreparedImSend,&'static str>{
        policy.validate()?;
        if !input.matches(&binding.scope(),&message,recipient,nonce)||!identity.authenticates(&binding)||!parent.live()||Instant::now()>=deadline{return Err(SEND_SCOPE);}
        if message.kind_name()=="text"&&!policy.send_text||message.kind_name()=="info"&&!policy.send_info{return Err("app_account_im_send_not_installed");}
        let context=ImInboxScope::original(run,binding,parent)?;
        let gate=ImReceiptGate::fresh();
        let candidate=Zeroizing::new([b"Org/".as_slice(),recipient].concat());
        let(fixed_box,lookup)=if super::super::im_data::ImBox::parse(&candidate).is_ok(){(Some(candidate),None)}else{
            let uri=super::super::user_entry_data::RecipientUri::parse(recipient).map_err(|_|INVALID_SEND)?;
            if uri.kind()==super::super::user_entry_data::AidKind::Uid{(Some(Zeroizing::new([b"P2P/".as_slice(),uri.value()].concat())),None)}else{
                (None,Some(UserEntryAction::original(context.binding.owner().clone(),context.binding.scope(),gate.clone(),deadline,recipient).map_err(|_|SEND_SCOPE)?))
            }
        };
        let uri=identity.uri_for(context.binding.owner(),&context.binding.scope()).ok_or(SEND_SCOPE)?;
        let intent_hash=digest(&[b"original-user-send-intent-v1\0",context.binding.identity(),uri,recipient,&message.fingerprint()]);
        let action=Arc::new(Self{database,context,identity,recipient:Zeroizing::new(recipient.to_vec()),fixed_box,lookup,lookup_reply:Mutex::new(None),message,nonce:Zeroizing::new(nonce.to_owned()),intent_hash,row:AtomicI64::new(0),gate,deadline,permit:Mutex::new(PermitSlot{cancelled:false,permit:Weak::new()}),_input_budget:input});
        let admission=action.database.with_conn(|c|{
            let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
            action.context.validate(&tx)?;
            if !action.live(){return Err(SEND_SCOPE);}
            let old:Option<(i64,Vec<u8>)>=tx.query_row("SELECT id,intent_hash FROM app_voice_im_outbox WHERE line_id=?1 AND user_nonce=?2",params![action.context.run.line_id(),action.nonce.as_str()],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|STORAGE)?;
            if let Some((id,hash))=old{
                if hash.as_slice()!=action.intent_hash.as_slice(){return Err("app_account_im_send_nonce_conflict");}
                return Ok((id,true));
            }
            // Fixed original ACK ownership keeps UID+selector. The unresolved
            // account fence deliberately does NOT use that mutable identity.
            let unknown:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_outbox WHERE line_id=?1 AND state=0)",[action.context.run.line_id()],|r|r.get(0)).map_err(|_|STORAGE)?;
            if unknown{return Err(UNCERTAIN);}
            let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_im_outbox",[action.context.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;
            if local>=128||total>=4096{return Err("app_account_im_send_full");}
            tx.execute("INSERT INTO app_voice_im_outbox(line_id,revision,identity_key,user_nonce,intent_hash,message_kind,run_nonce,scope_epoch,state,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0,?9)",params![action.context.run.line_id(),action.context.run.revision()as i64,action.context.binding.identity().as_slice(),action.nonce.as_str(),action.intent_hash.as_slice(),action.message.kind_name(),action.context.run.nonce(),action.context.binding.receipt_epoch(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;
            let id=tx.last_insert_rowid();
            if !action.live(){return Err(SEND_SCOPE);}
            tx.commit().map_err(|_|STORAGE)?;
            Ok((id,false))
        })?;
        if admission.1{return Ok(PreparedImSend::Replay(action.database.with_conn(|c|view(c,admission.0))?));}
        action.row.store(admission.0,Ordering::SeqCst);
        Ok(PreparedImSend::Fresh(action))
    }
    pub(in super::super) fn live(&self)->bool{self.gate.live()&&self.context.gate.live()&&self.identity.authenticates(&self.context.binding)&&Instant::now()<self.deadline}
    pub(in super::super) fn owner(&self)->&AttemptOwner{self.context.binding.owner()}
    pub(in super::super) fn scope(&self)->RpcConnectionScope{self.context.binding.scope()}
    pub(in super::super) fn deadline(&self)->Instant{self.deadline}
    pub(in super::super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(in super::super) fn parent_gate(&self)->Arc<ImReceiptGate>{self.context.gate.clone()}
    pub(in super::super) fn recipient_action(&self)->Option<Arc<UserEntryAction>>{self.lookup.clone()}
    pub(in super::super) fn confirm_recipient(&self,reply:OwnedUserEntryReply)->Result<(),&'static str>{let lookup=self.lookup.as_ref().ok_or(SEND_SCOPE)?;if !self.live()||reply.resolved_uid_for(lookup).is_none(){return Err("app_account_im_send_recipient_unconfirmed");}let mut old=lock(&self.lookup_reply);if old.is_some(){return Err(SEND_SCOPE);}*old=Some(Arc::new(reply));Ok(())}
    pub(in super::super) fn permit(self:&Arc<Self>,policy:&NativeImSendConfig)->Result<Arc<ImSendPermit>,&'static str>{
        // This private lifecycle lock serializes creation with cancellation.
        // It is not a native IO gate and is never held across await/transport.
        let mut slot=lock(&self.permit);
        if slot.cancelled||slot.permit.upgrade().is_some()||!self.live(){return Err(SEND_SCOPE);}
        let box_id=if let Some(b)=&self.fixed_box{b.clone()}else{
            let lookup=self.lookup.as_ref().ok_or(SEND_SCOPE)?;
            let resolved=lock(&self.lookup_reply);
            let uid=resolved.as_ref().and_then(|r|r.resolved_uid_for(lookup)).ok_or("app_account_im_send_recipient_unconfirmed")?;
            Zeroizing::new([b"P2P/".as_slice(),uid].concat())
        };
        let arguments=self.message.arguments(&box_id,&self.identity,self.owner(),&self.scope(),policy)?;
        let argument_bytes=arguments.len().checked_add(box_id.len()+std::mem::size_of::<ImSendPermit>()+64).ok_or("app_account_im_send_capacity")?;
        let arguments_budget=self.scope().reserve_native_rpc(argument_bytes).map_err(|_|"app_account_im_send_capacity")?;
        let arguments_hash=digest(&[b"original-user-send-arguments-v1\0",&arguments]);
        let row=self.row.load(Ordering::SeqCst);
        self.database.with_conn(|c|{
            let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
            self.context.validate(&tx)?;
            let n=tx.execute("UPDATE app_voice_im_outbox SET box_id=?3,arguments_hash=?4 WHERE id=?1 AND user_nonce=?2 AND intent_hash=?5 AND run_nonce=?6 AND scope_epoch=?7 AND state=0 AND request_id IS NULL AND box_id IS NULL",params![row,self.nonce.as_str(),box_id.as_slice(),arguments_hash.as_slice(),self.intent_hash.as_slice(),self.context.run.nonce(),self.context.binding.receipt_epoch()]).map_err(|_|STORAGE)?;
            if n!=1||!self.live(){return Err(SEND_SCOPE);}
            tx.commit().map_err(|_|STORAGE)
        })?;
        let permit=Arc::new(ImSendPermit{action:self.clone(),box_id,arguments,arguments_hash,phase:AtomicU8::new(QUEUED),request:Mutex::new(None),transition:Mutex::new(()),_arguments_budget:arguments_budget,changed:tokio::sync::Notify::new(),#[cfg(test)]resolve_probe:Mutex::new(None)});
        slot.permit=Arc::downgrade(&permit);
        drop(slot);
        if !self.live(){permit.cancel();return Err(SEND_SCOPE);}
        Ok(permit)
    }
    pub(in super::super) fn cancel(&self){
        let permit={let mut slot=lock(&self.permit);slot.cancelled=true;let permit=slot.permit.upgrade();if permit.is_none(){self.gate.retire();}permit};
        if let Some(p)=permit{p.cancel();self.gate.retire();}else{
            let row=self.row.load(Ordering::SeqCst);
            if row>0{let _=self.database.with_conn(|c|c.execute("UPDATE app_voice_im_outbox SET state=3,updated_at=?3 WHERE id=?1 AND user_nonce=?2 AND intent_hash=?4 AND state=0 AND request_id IS NULL",params![row,self.nonce.as_str(),chrono::Utc::now().to_rfc3339(),self.intent_hash.as_slice()]).map_err(|_|STORAGE));}
        }
    }
}
impl Drop for ImSendAction{fn drop(&mut self){self.cancel();}}
struct SendRequest{id:i32,deadline:Instant}
pub(in super::super) struct ImSendPermit{action:Arc<ImSendAction>,box_id:Zeroizing<Vec<u8>>,arguments:Zeroizing<Vec<u8>>,arguments_hash:[u8;32],phase:AtomicU8,request:Mutex<Option<SendRequest>>,transition:Mutex<()>,_arguments_budget:Arc<super::super::native_rpc_budget::NativeRpcReservation>,changed:tokio::sync::Notify,#[cfg(test)]resolve_probe:Mutex<Option<Arc<dyn Fn()+Send+Sync>>>}
impl fmt::Debug for ImSendPermit{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSendPermit(<redacted>)")}}
impl ImSendPermit{
    pub(in super::super) fn binding(&self)->&Arc<ImBinding>{&self.action.context.binding}
    pub(in super::super) fn arguments(&self)->&[u8]{&self.arguments}
    pub(in super::super) fn retained_bytes(&self)->usize{self.arguments.len()+self.action.message.bytes()+self.action.recipient.len()+self.box_id.len()+256}
    pub(in super::super) fn gate(&self)->Arc<ImReceiptGate>{self.action.gate()}
    pub(in super::super) fn parent_gate(&self)->Arc<ImReceiptGate>{self.action.parent_gate()}
    pub(in super::super) fn deadline(&self)->Instant{self.action.deadline()}
    pub(in super::super) fn matches(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->bool{self.action.live()&&self.binding().matches(owner,scope)&&self.phase.load(Ordering::SeqCst)<RESOLVED}
    pub(in super::super) fn token_for(&self,owner:&AttemptOwner,scope:&RpcConnectionScope)->Option<&[u8]>{self.action.identity.token_for(owner,scope)}
    fn fixed(&self,c:&Connection)->Result<(),&'static str>{let matching:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_outbox WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4 AND user_nonce=?5 AND intent_hash=?6 AND box_id=?7 AND arguments_hash=?8 AND run_nonce=?9 AND scope_epoch=?10)",params![self.action.row.load(Ordering::SeqCst),self.action.context.run.line_id(),self.action.context.run.revision()as i64,self.binding().identity().as_slice(),self.action.nonce.as_str(),self.action.intent_hash.as_slice(),self.box_id.as_slice(),self.arguments_hash.as_slice(),self.action.context.run.nonce(),self.binding().receipt_epoch()],|r|r.get(0)).map_err(|_|STORAGE)?;if !matching{return Err(SEND_SCOPE);}Ok(())}
    pub(in super::super) fn arm(&self,id:i32,deadline:Instant)->Result<(),&'static str>{let _transition=lock(&self.transition);let deadline=deadline.min(self.deadline());if id<=0||!self.action.live()||Instant::now()>=deadline||self.phase.load(Ordering::SeqCst)!=QUEUED{return Err(SEND_SCOPE);}let mut request=lock(&self.request);if request.is_some(){return Err(SEND_SCOPE);}self.action.database.with_conn(|c|{let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.action.context.validate(&tx)?;self.fixed(&tx)?;let n=tx.execute("UPDATE app_voice_im_outbox SET request_id=?3 WHERE id=?1 AND user_nonce=?2 AND state=0 AND request_id IS NULL",params![self.action.row.load(Ordering::SeqCst),self.action.nonce.as_str(),id]).map_err(|_|STORAGE)?;if n!=1||!self.action.live()||Instant::now()>=deadline{return Err(SEND_SCOPE);}tx.commit().map_err(|_|STORAGE)})?;*request=Some(SendRequest{id,deadline});Ok(())}
    pub(in super::super) fn current(&self,id:i32)->bool{self.action.live()&&lock(&self.request).as_ref().is_some_and(|r|r.id==id&&Instant::now()<r.deadline)}
    pub(in super::super) fn begin_write(&self,id:i32)->bool{let _transition=lock(&self.transition);self.current(id)&&self.phase.compare_exchange(QUEUED,WRITING,Ordering::SeqCst,Ordering::SeqCst).is_ok()}
    pub(in super::super) fn submitted(&self)->bool{let _transition=lock(&self.transition);self.phase.compare_exchange(WRITING,SUBMITTED,Ordering::SeqCst,Ordering::SeqCst).is_ok()}
    #[cfg(test)]pub(in super::super) fn has_submitted(&self)->bool{self.phase.load(Ordering::SeqCst)==SUBMITTED}
    pub(in super::super) fn retired(&self)->bool{self.phase.load(Ordering::SeqCst)>=RETIRED}
    pub(in super::super) fn closed_original_io(&self)->bool{self.phase.load(Ordering::SeqCst)==CLOSED_WRITE}
    #[cfg(test)]pub(in super::super) fn set_resolve_probe(&self,probe:Arc<dyn Fn()+Send+Sync>){*lock(&self.resolve_probe)=Some(probe);}
    pub(in super::super) fn resolve(&self,reply:&super::super::rpc_im_send::OwnedImSendReply)->Result<ImSendView,&'static str>{
        // The private transition lock, NOT the original IO gate, is the one
        // resolution-vs-cancellation linearization point. A failed transaction
        // leaves SUBMITTED; cancel then keeps unknown and closes this scope.
        let _transition=lock(&self.transition);
        let id=reply.request().id();
        if !reply.belongs_to(self)||!self.current(id)||self.phase.load(Ordering::SeqCst)!=SUBMITTED{return Err(UNCERTAIN);}
        let row=self.action.row.load(Ordering::SeqCst);
        self.action.database.with_conn(|c|{
            let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
            self.action.context.validate(&tx)?;self.fixed(&tx)?;
            let n=tx.execute("UPDATE app_voice_im_outbox SET state=?4,result_code=?5,message_index=?6,original_time=?7,updated_at=?8 WHERE id=?1 AND user_nonce=?2 AND request_id=?3 AND state=0",params![row,self.action.nonce.as_str(),id,if reply.accepted(){1}else{2},i64::from(reply.code()),reply.message_index(),reply.time(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;
            if n!=1||!self.current(id)||self.phase.load(Ordering::SeqCst)!=SUBMITTED{return Err(UNCERTAIN);}
            #[cfg(test)]if let Some(probe)=lock(&self.resolve_probe).clone(){probe();}
            // Build the DATA projection before committing: bad projection must
            // roll back, never commit a terminal row and return a fake unknown.
            let result=view(&tx,row)?;
            tx.commit().map_err(|_|STORAGE)?;
            self.phase.store(RESOLVED,Ordering::SeqCst);
            Ok(result)
        })
    }
    pub(in super::super) fn cancel(&self){let _transition=lock(&self.transition);let original=loop{let before=self.phase.load(Ordering::SeqCst);if before>=RESOLVED{return;}let target=if before==QUEUED{RETIRED}else{CLOSED_WRITE};if self.phase.compare_exchange(before,target,Ordering::SeqCst,Ordering::SeqCst).is_ok(){break before;}};if original==QUEUED{let _=self.action.database.with_conn(|c|{let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.fixed(&tx)?;tx.execute("UPDATE app_voice_im_outbox SET state=3,updated_at=?3 WHERE id=?1 AND user_nonce=?2 AND state=0",params![self.action.row.load(Ordering::SeqCst),self.action.nonce.as_str(),chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;tx.commit().map_err(|_|STORAGE)});}else{self.action.scope().close();}self.changed.notify_waiters();}
    pub(in super::super) async fn cancelled(&self){loop{let changed=self.changed.notified();tokio::pin!(changed);changed.as_mut().enable();if self.retired(){return;}changed.await;}}
}
impl Drop for ImSendPermit{fn drop(&mut self){self.cancel();}}
#[derive(Serialize)]pub struct ImSendView{pub id:i64,pub nonce_tag:String,pub kind:&'static str,pub state:&'static str,pub message_index:Option<String>,pub original_time:Option<String>,pub updated_at:String}
impl fmt::Debug for ImSendView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImSendView(<redacted>)")}}
fn view(c:&Connection,id:i64)->Result<ImSendView,&'static str>{c.query_row("SELECT id,message_kind,state,message_index,original_time,updated_at,user_nonce FROM app_voice_im_outbox WHERE id=?1",[id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,Option<i64>>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?))).map_err(|_|STORAGE).and_then(|(id,kind,state,index,time,updated_at,mut nonce)|{if updated_at.len()>64{return Err(STORAGE);}let mut hash=Context::new(&SHA256);hash.update(b"original-im-send-ui-nonce-v1\0");hash.update(nonce.as_bytes());let nonce_tag=hash.finish().as_ref().iter().map(|b|format!("{b:02x}")).collect();zeroize::Zeroize::zeroize(&mut nonce);Ok(ImSendView{id,nonce_tag,kind:if kind=="text"{"text"}else if kind=="info"{"info"}else{return Err(STORAGE)},state:match state{0=>"uncertain",1=>"confirmed",2=>"rejected",3=>"not_sent",_=>return Err(STORAGE)},message_index:index.map(|v|v.to_string()),original_time:time.map(|v|v.to_string()),updated_at})})}
pub(in super::super) fn history(c:&Connection,line:&str,limit:usize)->Result<Vec<ImSendView>,&'static str>{if !(1..=128).contains(&limit)||account_store::metadata(c,line)?.is_none(){return Err(INVALID_SEND);}let mut q=c.prepare("SELECT id FROM app_voice_im_outbox WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;let ids=q.query_map(params![line,limit as i64],|r|r.get::<_,i64>(0)).map_err(|_|STORAGE)?.collect::<Result<Vec<_>,_>>().map_err(|_|STORAGE)?;ids.into_iter().map(|id|view(c,id)).collect()}
pub(in super::super) struct ImSendWait{action:Arc<ImSendAction>}
impl ImSendWait{pub(in super::super) fn original(action:Arc<ImSendAction>)->Self{Self{action}}}
impl Drop for ImSendWait{fn drop(&mut self){self.action.cancel();}}
pub(in super::super) struct ImSendPermitWait{permit:Arc<ImSendPermit>}
impl ImSendPermitWait{pub(in super::super) fn original(permit:Arc<ImSendPermit>)->Self{Self{permit}}}
impl Drop for ImSendPermitWait{fn drop(&mut self){self.permit.cancel();}}
