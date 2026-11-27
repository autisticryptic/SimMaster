//! Durable before-await relation preparation exclusion, NOT a native WRITE grant.
//! Preparing rows never stand in for UID/token/ImBinding, an RPC id or a reply.
use super::*;
const PREPARING:u8=0;const BOUND:u8=1;const RETIRED:u8=2;
pub(in super::super) enum IntentAdmission{Fresh(Arc<RelationIntent>),Replay(i64)}
pub(in super::super) struct RelationIntent{
    database:Arc<Database>,run:Arc<RunToken>,owner:AttemptOwner,scope:RpcConnectionScope,
    message:Arc<RelationMessage>,configuration:Arc<RelationConfiguration>,parent:Arc<ImReceiptGate>,gate:Arc<ImReceiptGate>,
    claim:Zeroizing<String>,body_hash:[u8;32],id:i64,deadline:Instant,phase:AtomicU8,transition:Mutex<Weak<RelationAction>>,
}
impl fmt::Debug for RelationIntent{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationIntent(<redacted>)")}}
pub(in super::super) fn initialize_schema(c:&Connection)->Result<(),&'static str>{
    c.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_relation_intents(
        id INTEGER PRIMARY KEY AUTOINCREMENT,line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        user_nonce TEXT NOT NULL CHECK(length(user_nonce)=32),body_hash BLOB NOT NULL CHECK(typeof(body_hash)='blob' AND length(body_hash)=32),
        run_nonce TEXT NOT NULL,claim_nonce TEXT NOT NULL CHECK(length(claim_nonce)=32),configuration_hash BLOB NOT NULL CHECK(length(configuration_hash)=32),
        target_kind TEXT NOT NULL CHECK(target_kind IN('buddy','group')),additions INTEGER NOT NULL CHECK(additions BETWEEN 0 AND 128),
        updates INTEGER NOT NULL CHECK(updates BETWEEN 0 AND 128),removals INTEGER NOT NULL CHECK(removals BETWEEN 0 AND 128),
        state INTEGER NOT NULL CHECK(state IN(0,1,2)),write_id INTEGER CHECK(write_id>0),updated_at TEXT NOT NULL,
        UNIQUE(line_id,user_nonce));CREATE INDEX IF NOT EXISTS idx_relation_intents_line ON app_voice_relation_intents(line_id,id);")
        .map_err(|_|STORAGE)
}
pub(in super::super) fn delete_account(c:&Connection,line:&str)->Result<(),&'static str>{
    if account_store::metadata(c,line)?.is_some(){return Err(SCOPE);}
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_relation_intents')",[],|r|r.get(0)).map_err(|_|STORAGE)?;
    if exists{c.execute("DELETE FROM app_voice_relation_intents WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}Ok(())
}
impl RelationIntent{
    pub(in super::super) fn capture(database:Arc<Database>,run:Arc<RunToken>,owner:AttemptOwner,scope:RpcConnectionScope,message:Arc<RelationMessage>,configuration:Arc<RelationConfiguration>,parent:Arc<ImReceiptGate>,deadline:Instant)->Result<IntentAdmission,&'static str>{
        if owner.line_id()!=run.line_id()||!message.budget.belongs_to(&scope)||!scope.is_open()||scope.session_revoked()||!parent.live()||Instant::now()>=deadline{return Err(SCOPE);}
        let body_hash=message.fingerprint();let claim=Zeroizing::new(super::super::account_runtime::local_nonce().map_err(|_|"app_account_relation_entropy_failed")?);
        let(id,replay)=database.with_conn(|c|{
            let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;
            validate_original(&tx,&run,&scope,&parent,&configuration,deadline)?;management::validate_shared(&tx,run.line_id())?;
            let old:Option<(i64,Vec<u8>,i64,Option<i64>)>=tx.query_row("SELECT id,body_hash,state,write_id FROM app_voice_relation_intents WHERE line_id=?1 AND user_nonce=?2",params![run.line_id(),message.nonce.as_str()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_|STORAGE)?;
            if let Some((old_id,body,state,write))=old{
                if body.as_slice()!=body_hash.as_slice(){return Err("app_account_relation_nonce_conflict");}if let Some(input)=&message.creation{super::super::group_create_store::require_input(&tx,run.line_id(),message.nonce.as_str(),old_id,input)?;}management::require(&tx,run.line_id(),message.nonce.as_str(),message.management,Some(old_id),write)?;
                match(state,write){
                    (0,None)=>return Err("app_account_relation_preparing"),
                    (2,None)=>return Err("app_account_relation_nonce_retired"),
                    (1,Some(write))=>{let matches:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_writes WHERE id=?1 AND line_id=?2 AND user_nonce=?3)",params![write,run.line_id(),message.nonce.as_str()],|r|r.get(0)).map_err(|_|STORAGE)?;if !matches{return Err(STORAGE);}return Ok((write,true));},
                    _=>return Err(STORAGE),
                }
            }
            // A legacy WRITE without a before-await body proof is history-only.
            // Do not guess its UID/body association or silently adopt its nonce.
            let legacy:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_writes WHERE line_id=?1 AND user_nonce=?2)",params![run.line_id(),message.nonce.as_str()],|r|r.get(0)).map_err(|_|STORAGE)?;
            if legacy{return Err("app_account_relation_legacy_intent");}
            let unknown:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_writes WHERE line_id=?1 AND state=0)",[run.line_id()],|r|r.get(0)).map_err(|_|STORAGE)?;
            if unknown{return Err(UNKNOWN);}
            let pending:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_intents e WHERE e.line_id=?1 AND(e.state=0 OR(e.state=1 AND NOT EXISTS(SELECT 1 FROM app_voice_relation_writes w WHERE w.id=e.write_id AND w.line_id=e.line_id AND w.user_nonce=e.user_nonce AND w.state IN(1,2,3)))))",[run.line_id()],|r|r.get(0)).map_err(|_|STORAGE)?;
            if pending{return Err("app_account_relation_preparing");}
            let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_relation_intents",[run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;
            if local>=128||total>=4096{return Err("app_account_relation_full");}
            tx.execute("INSERT INTO app_voice_relation_intents(line_id,revision,user_nonce,body_hash,run_nonce,claim_nonce,configuration_hash,target_kind,additions,updates,removals,state,updated_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,0,?12)",params![run.line_id(),run.revision() as i64,message.nonce.as_str(),body_hash.as_slice(),run.nonce(),claim.as_str(),configuration.0.as_slice(),message.target_kind(),message.counts[0] as i64,message.counts[1] as i64,message.counts[2] as i64,chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;
            let id=tx.last_insert_rowid();if let Some(input)=&message.creation{super::super::group_create_store::capture_input(&tx,run.line_id(),message.nonce.as_str(),input,id)?;}management::capture(&tx,run.line_id(),message.nonce.as_str(),message.management,Some(id),None)?;validate_original(&tx,&run,&scope,&parent,&configuration,deadline)?;tx.commit().map_err(|_|STORAGE)?;Ok((id,false))
        })?;
        if replay{return Ok(IntentAdmission::Replay(id));}
        let intent=Arc::new(Self{database,run,owner,scope,message,configuration,parent,gate:ImReceiptGate::fresh(),claim,body_hash,id,deadline,phase:AtomicU8::new(PREPARING),transition:Mutex::new(Weak::new())});
        if !intent.live(){intent.cancel();return Err(SCOPE);}Ok(IntentAdmission::Fresh(intent))
    }
    fn live(&self)->bool{self.scope.is_open()&&!self.scope.session_revoked()&&self.parent.live()&&self.gate.live()&&Instant::now()<self.deadline}
    pub(super) fn fixed(&self,c:&Connection,state:i64,write:Option<i64>)->Result<(),&'static str>{
        let yes:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_intents WHERE id=?1 AND line_id=?2 AND revision=?3 AND user_nonce=?4 AND body_hash=?5 AND run_nonce=?6 AND claim_nonce=?7 AND configuration_hash=?8 AND state=?9 AND write_id IS ?10)",params![self.id,self.run.line_id(),self.run.revision() as i64,self.message.nonce.as_str(),self.body_hash.as_slice(),self.run.nonce(),self.claim.as_str(),self.configuration.0.as_slice(),state,write],|r|r.get(0)).map_err(|_|STORAGE)?;
        if !yes{return Err(SCOPE);}
        if let Some(input)=&self.message.creation{super::super::group_create_store::require_input(c,self.run.line_id(),self.message.nonce.as_str(),self.id,input)?;}
        management::require(c,self.run.line_id(),self.message.nonce.as_str(),self.message.management,Some(self.id),write)
    }
    pub(in super::super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(in super::super) fn binds(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,message:&Arc<RelationMessage>)->bool{self.live()&&&self.owner==owner&&self.scope.matches(scope)&&Arc::ptr_eq(&self.message,message)&&self.message.budget.belongs_to(scope)}
    pub(in super::super) fn belongs_to(&self,database:&Arc<Database>,run:&Arc<RunToken>,configuration:&Arc<RelationConfiguration>,parent:&Arc<ImReceiptGate>,owner:&AttemptOwner,scope:&RpcConnectionScope,message:&Arc<RelationMessage>,deadline:Instant)->bool{
        self.binds(owner,scope,message)&&Arc::ptr_eq(database,&self.database)&&Arc::ptr_eq(run,&self.run)&&Arc::ptr_eq(configuration,&self.configuration)&&Arc::ptr_eq(parent,&self.parent)&&deadline==self.deadline
    }
    pub(in super::super) fn bind(self:&Arc<Self>,binding:Arc<ImBinding>,identity:Arc<RelationIdentity>,policy:impl Into<RelationWriteGrant>)->Result<PreparedRelation,&'static str>{
        let policy=policy.into();let mut slot=lock(&self.transition);
        if self.phase.load(Ordering::SeqCst)!=PREPARING||!self.live()||binding.owner()!=&self.owner||!binding.scope().matches(&self.scope){return Err(SCOPE);}
        let result=RelationAction::original_captured(self.database.clone(),self.run.clone(),binding,identity,self.parent.clone(),self.message.clone(),self.deadline,policy,self.configuration.clone(),Some(self.clone()))?;
        let PreparedRelation::Fresh(action)=&result else{return Err(SCOPE);};
        *slot=Arc::downgrade(action);self.phase.store(BOUND,Ordering::SeqCst);drop(slot);
        if !self.live(){action.cancel();return Err(SCOPE);}Ok(result)
    }
    pub(super) fn commit_binding(&self,c:&Connection,write:i64)->Result<(),&'static str>{
        if !self.live()||self.phase.load(Ordering::SeqCst)!=PREPARING{return Err(SCOPE);}self.fixed(c,0,None)?;
        let count=c.execute("UPDATE app_voice_relation_intents SET state=1,write_id=?2,updated_at=?3 WHERE id=?1 AND state=0 AND write_id IS NULL",params![self.id,write,chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;
        if count!=1{return Err(SCOPE);}self.fixed(c,1,Some(write))
    }
    pub(super) fn require_bound(&self,c:&Connection,write:i64)->Result<(),&'static str>{
        if !self.live()||self.phase.load(Ordering::SeqCst)!=BOUND{return Err(SCOPE);}self.fixed(c,1,Some(write))
    }
    pub(in super::super) fn cancel(&self){
        let action={let slot=lock(&self.transition);let phase=self.phase.load(Ordering::SeqCst);
            if phase==RETIRED{return;}
            if phase==PREPARING{
                let _=self.database.with_conn(|c|{let tx=Transaction::new_unchecked(c,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.fixed(&tx,0,None)?;
                    tx.execute("UPDATE app_voice_relation_intents SET state=2,updated_at=?2 WHERE id=?1 AND state=0 AND write_id IS NULL",params![self.id,chrono::Utc::now().to_rfc3339()]).map_err(|_|STORAGE)?;tx.commit().map_err(|_|STORAGE)});
            }
            self.phase.store(RETIRED,Ordering::SeqCst);slot.upgrade()
        };
        self.gate.retire();if let Some(action)=action{action.cancel();}
    }
}
impl Drop for RelationIntent{fn drop(&mut self){self.cancel();}}
fn validate_original(c:&Connection,run:&RunToken,scope:&RpcConnectionScope,parent:&ImReceiptGate,configuration:&RelationConfiguration,deadline:Instant)->Result<(),&'static str>{
    if !scope.is_open()||scope.session_revoked()||!parent.live()||Instant::now()>=deadline||account_store::metadata(c,run.line_id())?.is_none_or(|m|m.revision!=run.revision())||!account_runs::runtime_matches(c,run)?{return Err(SCOPE);}
    account_runs::require_live(c,run.line_id(),run.revision(),run.nonce())?;configuration.require_current(c,run.line_id(),run.revision(),scope)
}
pub(in super::super) struct RelationIntentWait(Arc<RelationIntent>);
impl RelationIntentWait{pub(in super::super) fn original(intent:Arc<RelationIntent>)->Self{Self(intent)}}
impl Drop for RelationIntentWait{fn drop(&mut self){self.0.cancel();}}
#[derive(Serialize)]
pub(crate) struct RelationIntentView{pub id:i64,pub nonce_tag:String,pub state:&'static str,pub write_id:Option<i64>,pub target_kind:&'static str,pub additions:usize,pub updates:usize,pub removals:usize,pub updated_at:String,#[serde(skip_serializing_if="Option::is_none")]pub management:Option<GroupManagementView>}
impl fmt::Debug for RelationIntentView{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("RelationIntentView(<redacted>)")}}
pub(in super::super) fn history(c:&Connection,line:&str,limit:usize)->Result<Vec<RelationIntentView>,&'static str>{
    if !(1..=128).contains(&limit)||account_store::metadata(c,line)?.is_none(){return Err(INVALID);}
    let mut statement=c.prepare("SELECT id,user_nonce,state,write_id,target_kind,additions,updates,removals,updated_at FROM app_voice_relation_intents WHERE line_id=?1 ORDER BY id DESC LIMIT ?2").map_err(|_|STORAGE)?;
    let rows=statement.query_map(params![line,limit as i64],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,Option<i64>>(3)?,r.get::<_,String>(4)?,r.get::<_,usize>(5)?,r.get::<_,usize>(6)?,r.get::<_,usize>(7)?,r.get::<_,String>(8)?))).map_err(|_|STORAGE)?;
    rows.map(|row|{let(id,nonce,state,write,kind,adds,updates,removes,at)=row.map_err(|_|STORAGE)?;let nonce=Zeroizing::new(nonce);
        if [adds,updates,removes].iter().any(|n|*n>128){return Err(STORAGE);}
        let descriptor=if adds+updates+removes==0{management::get(c,line,nonce.as_str(),Some(id),write)?}else{None};if descriptor.is_some_and(|_|kind!="group"||adds+updates+removes!=0){return Err(STORAGE);}
        if id<=0||nonce.len()!=32||!nonce.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))||at.len()>64||chrono::DateTime::parse_from_rfc3339(&at).is_err()||[adds,updates,removes].iter().any(|n|*n>128)||descriptor.is_none()&&adds+updates+removes==0||!matches!((state,write),(0,None)|(2,None)|(1,Some(1..=i64::MAX))){return Err(STORAGE);}
        let tag=hash(&[b"original-relation-intent-ui-nonce-v1",nonce.as_bytes()]).iter().map(|b|format!("{b:02x}")).collect();
        Ok(RelationIntentView{id,nonce_tag:tag,state:match state{0=>"preparing",1=>"bound",2=>"not_sent",_=>return Err(STORAGE)},write_id:write,target_kind:match kind.as_str(){"buddy"=>"buddy","group"=>"group",_=>return Err(STORAGE)},additions:adds,updates,removals:removes,updated_at:at,management:descriptor.map(|d|GroupManagementView{operation:d.operation.name(),properties:d.properties})})
    }).collect()
}
