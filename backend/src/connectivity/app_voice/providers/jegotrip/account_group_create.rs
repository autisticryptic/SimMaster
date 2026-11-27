//! Explicit owner/member creation on the original account, plus separately
//! requested property READ. A stored replay has NO fresh creation read brand.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
#[derive(serde::Serialize)]
pub(crate) struct GroupCreateView{pub write:RelationWriteView,pub properties_read:n::group_create_read::PropertiesReadView}
impl std::fmt::Debug for GroupCreateView{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("GroupCreateView(<redacted>)")}}
pub(crate) struct PreparedGroupCreate{write:PreparedRelationUpdate,read_properties:bool}
impl std::fmt::Debug for PreparedGroupCreate{fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("PreparedGroupCreate(<redacted>)")}}
impl AccountManager{
 pub(crate) fn reserve_group_create_http_scratch(&self,line:&str,revision:u64,bytes:usize,consent:bool,write_consent:bool)->Result<RelationHttpScratch,&'static str>{
  self.creation_http_scratch(line,revision,bytes,consent,write_consent,false)
 }
 pub(crate) fn reserve_owner_only_http_scratch(&self,line:&str,revision:u64,bytes:usize,consent:bool,write_consent:bool,session_owner_consent:bool)->Result<RelationHttpScratch,&'static str>{
  self.creation_http_scratch(line,revision,bytes,consent&&session_owner_consent,write_consent,true)
 }
 fn creation_http_scratch(&self,line:&str,revision:u64,bytes:usize,consent:bool,write_consent:bool,owner_only:bool)->Result<RelationHttpScratch,&'static str>{
  if !consent||!write_consent{return Err("app_account_relation_consent_required");}self.check_revision(line,revision)?;let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_relation_scope_invalid")?;
  if owner_only&&!run.backend.owner_only_installed(){return Err("app_account_group_owner_only_not_installed");}
  if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live(){return Err("app_account_relation_scope_invalid");}run.configuration_current()?;live_run(&run)?;
  let(scope,budget)=run.backend.reserve_creation_scratch(bytes)?;let cfg=run.relation_configuration.as_ref().ok_or("app_account_group_create_not_installed")?;run.database.with_conn(|c|cfg.require_current(c,line,revision,&scope))?;Ok(RelationHttpScratch(budget))
 }
 pub(crate) fn prepare_owner_only_group(&self,line:&str,revision:u64,name:&str,kind:u32,properties:&[n::group_management::PublicProperty<'_>],nonce:&str,consent:bool,write_consent:bool,session_owner_consent:bool,read_properties:bool)->Result<PreparedGroupCreate,&'static str>{
  self.prepare_creation(line,revision,name,kind,None,properties,nonce,consent&&session_owner_consent,write_consent,read_properties)
 }
 pub(crate) fn prepare_group_create(&self,line:&str,revision:u64,name:&str,kind:u32,owner:&[u8],members:&[Relation<'_>],properties:&[n::group_management::PublicProperty<'_>],nonce:&str,consent:bool,write_consent:bool,read_properties:bool)->Result<PreparedGroupCreate,&'static str>{
  self.prepare_creation(line,revision,name,kind,Some((owner,members)),properties,nonce,consent,write_consent,read_properties)
 }
 fn prepare_creation(&self,line:&str,revision:u64,name:&str,kind:u32,explicit:Option<(&[u8],&[Relation<'_>])>,properties:&[n::group_management::PublicProperty<'_>],nonce:&str,consent:bool,write_consent:bool,read_properties:bool)->Result<PreparedGroupCreate,&'static str>{
  if !consent||!write_consent{return Err("app_account_relation_consent_required");}self.check_revision(line,revision)?;let slot=self.slot(line)?;let run=lock(&slot.run).clone().ok_or("app_account_relation_scope_invalid")?;
  if !run.accepting.load(Ordering::SeqCst)||run.stopping.load(Ordering::SeqCst)||run.finalized.load(Ordering::SeqCst)||!run.im_gate.live()||!run.backend.group_creation_enabled(){return Err("app_account_group_create_not_installed");}
  run.configuration_current()?;live_run(&run)?;let deadline=Instant::now().checked_add(run.backend.relation_update_timeout()).ok_or("app_account_relation_timeout")?;
  let(owner,scope,message)=match explicit{Some((owner,members))=>run.backend.reserve_creation_message(name,kind,owner,members,properties,nonce,read_properties)?,None=>run.backend.reserve_owner_only_message(name,kind,properties,nonce,read_properties)?};let configuration=run.relation_configuration.as_ref().cloned().ok_or("app_account_group_create_not_installed")?;run.database.with_conn(|c|configuration.require_current(c,line,revision,&scope))?;let message=Arc::new(message);
  let admission=RelationIntent::capture(run.database.clone(),run.run_token.clone(),owner.clone(),scope.clone(),message.clone(),configuration.clone(),run.im_gate.clone(),deadline)?;let interest=match &admission{IntentAdmission::Fresh(i)=>Some(RelationIntentWait::original(i.clone())),IntentAdmission::Replay(_)=>None};
  Ok(PreparedGroupCreate{write:PreparedRelationUpdate{run,owner,scope,message,deadline,configuration,admission,_interest:interest},read_properties})
 }
 pub(crate) async fn finish_group_create(&self,prepared:PreparedGroupCreate)->Result<GroupCreateView,&'static str>{
  let PreparedGroupCreate{write,read_properties}=prepared;let run=write.run.clone();let owner=write.owner.clone();let scope=write.scope.clone();
  let mut result=self.finish_relation_update(write).await?;
  // Membership/history replay cannot reproduce this brand. Only the current
  // exact committed create result may feed the original property request.
  let properties_read=if !read_properties{n::group_create_read::PropertiesReadView::no_result("not_requested")}else if let Some(receipt)=result.creation_receipt.take(){
   let deadline=receipt.deadline();let live=||run.accepting.load(Ordering::SeqCst)&&!run.stopping.load(Ordering::SeqCst)&&!run.finalized.load(Ordering::SeqCst)&&run.im_gate.live();
   let read=Box::pin(async{
    let controller=tokio::select!{biased;_=run.im_gate.retired()=>return Err(()),_=scope.session_ended()=>return Err(()),r=run.controller.lock()=>r};
    if !live()||controller.phase()!=Phase::Hosted||controller.resource_owner().as_ref()!=Some(&owner)||!run.backend.original_scope(&owner).is_some_and(|s|s.matches(&scope))||!receipt.current(){return Err(());}drop(controller);
    let view=run.backend.read_created_properties(receipt).await.map_err(|_|())?;if !live()||Instant::now()>=deadline{return Err(());}Ok(view)
   });
   match tokio::time::timeout_at(deadline.into(),read).await{Ok(Ok(view))=>view,_=>n::group_create_read::PropertiesReadView::no_result("unverified")}
  }else{n::group_create_read::PropertiesReadView::no_result("not_attempted")};
  Ok(GroupCreateView{write:result,properties_read})
 }
}
impl RelationHttpScratch{pub(crate) fn matches_create(&self,p:&PreparedGroupCreate)->bool{self.0.belongs_to(&p.write.scope)}}
