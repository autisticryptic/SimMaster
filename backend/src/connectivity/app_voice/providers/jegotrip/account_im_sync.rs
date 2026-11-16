//! One bounded original IM read/received-write chain in the account consumer.
//! Readonly sync alone never writes; an independent installed receipt policy
//! and durable current span admit the separately owned WRITE future.
use super::*;
use super::super::{im_inbox::{CursorSession,GapPlan,GapCommitted,ReceivedAdmission},rpc_im::{ImQuery,OwnedImReply},rpc_im_received::OwnedReceivedReply,transport_actor::RuntimeError};
use std::{future::Future,pin::Pin};
type ReadFuture=Pin<Box<dyn Future<Output=Result<OwnedImReply,RuntimeError>>+Send>>;
type WriteFuture=Pin<Box<dyn Future<Output=Result<OwnedReceivedReply,RuntimeError>>+Send>>;
pub(super) enum Completion{Read(Result<OwnedImReply,RuntimeError>),Write(Result<OwnedReceivedReply,RuntimeError>)}
#[derive(Default)]
pub(super) struct State{reading:Option<ReadFuture>,writing:Option<WriteFuture>,received_span:Option<GapCommitted>,receipt_failed:bool,query:Option<Arc<ImQuery>>,gap:Option<GapPlan>,cursor:Option<CursorSession>,begun:bool,failed:bool}
impl State{
    pub(super) fn clear(&mut self){self.reading=None;self.writing=None;self.received_span=None;self.receipt_failed=true;self.query=None;self.gap=None;self.cursor=None;self.failed=true;}
    fn start_received(&mut self,run:&ManagedRun,controller:&HostingController,im:&mut super::im::State)->Result<(),&'static str>{
        let Some(policy)=run.backend.im_receipt_policy()else{self.received_span=None;im.clear_received_spans();return Ok(());};
        if self.receipt_failed{self.received_span=None;im.clear_received_spans();return Ok(());}
        if self.reading.is_some()||self.writing.is_some(){return Ok(());}
        let Some(span)=self.received_span.take().or_else(||im.next_received_span())else{return Ok(());};
        let admission=super::super::im_inbox::begin_received(run.database.clone(),&span,&policy);
        let permit=match admission{Ok(ReceivedAdmission::Fresh(permit))=>permit,Ok(ReceivedAdmission::AlreadyConfirmed)=>return Ok(()),Err(code)=>{
            self.receipt_failed=true;im.clear_received_spans();*lock(&run.error)=Some(code);return Ok(());
        }};
        let Some(owner)=controller.resource_owner()else{permit.cancel();return Err("app_account_im_scope_invalid");};
        let Some(scope)=run.backend.original_scope(&owner)else{permit.cancel();return Err("app_account_im_scope_invalid");};
        let backend=run.backend.clone();self.writing=Some(Box::pin(async move{backend.write_received(&owner,&scope,permit).await}));Ok(())
    }
    pub(super) fn received_write(&mut self,run:&ManagedRun,phase:Phase,result:Result<OwnedReceivedReply,RuntimeError>,im:&mut super::im::State)->Result<(),&'static str>{
        // Consume the completed heap future exactly once before another select.
        self.writing.take();
        if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live()||phase!=Phase::Hosted{self.clear();return Ok(());}run.configuration_current()?;
        let code=match result{
            Ok(reply)if reply.matches(reply.request().permit().binding().owner(),&reply.request().permit().binding().scope())=>{
                if reply.accepted().map_err(|_|"app_account_im_receipt_unconfirmed")?{None}else{Some("app_account_im_receipt_rejected")}
            }
            _=>Some("app_account_im_receipt_unconfirmed"),
        };
        if let Some(code)=code{self.receipt_failed=true;self.received_span=None;im.clear_received_spans();*lock(&run.error)=Some(code);}
        Ok(())
    }
    pub(super) fn pending(&self)->bool{self.reading.is_some()||self.writing.is_some()}
    pub(super) async fn next(&mut self)->Completion{
        // One selected mutable future owns the chain; read and remote write
        // waits cannot be polled concurrently or completed twice.
        if let Some(write)=self.writing.as_mut(){return Completion::Write(write.await);}
        match self.reading.as_mut(){Some(read)=>Completion::Read(read.await),None=>std::future::pending().await}
    }
    fn blocked_box(&self)->Option<&[u8]>{self.gap.as_ref().map(GapPlan::box_bytes)}
    fn fail(&mut self,run:&ManagedRun,code:&'static str){
        // Retire the read INTEREST, not a healthy fully-submitted SDK channel.
        // No automatic replay: this one chain stays paused for this run.
        self.reading=None;self.query=None;self.gap=None;self.failed=true;
        if run.accepting.load(Ordering::SeqCst){*lock(&run.error)=Some(code);}
    }
    pub(super) async fn drive(&mut self,run:&ManagedRun,controller:&HostingController,im:&mut super::im::State)->Result<(),&'static str>{
        if !run.backend.im_sync_enabled(){return Ok(());}
        if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live(){self.clear();return Ok(());}
        if controller.phase()!=Phase::Hosted{return Ok(());}run.configuration_current()?;
        self.start_received(run,controller,im)?;
        let write_enabled=!self.receipt_failed&&run.backend.im_receipt_policy().is_some();
        match super::im::ordered(run,im,if self.failed{None}else{self.cursor.as_ref()},self.blocked_box(),write_enabled).await{
            Ok(())=>{},Err(code @ ("app_account_im_type_unsupported"|"app_account_im_message_invalid"))=>{self.fail(run,code);return Ok(());},Err(code)=>return Err(code),
        }
        self.start_received(run,controller,im)?;
        if self.reading.is_some()||self.writing.is_some()||self.failed{return Ok(());}
        let query=if !self.begun{self.begun=true;Arc::new(ImQuery::refresh(&[],-1).map_err(|_|"app_account_im_cursor_invalid")?)}
        else if let Some(cursor)=&self.cursor{let Some(plan)=cursor.next_gap(&run.database)?else{return Ok(());};let query=plan.query();self.gap=Some(plan);query}
        else{return Ok(());};
        let owner=controller.resource_owner().ok_or("app_account_im_scope_invalid")?;let scope=run.backend.original_scope(&owner).ok_or("app_account_im_scope_invalid")?;
        let backend=run.backend.clone();let gate=run.im_gate.clone();let submitted=query.clone();self.query=Some(query);
        self.reading=Some(Box::pin(async move{backend.read_im(&owner,&scope,submitted,gate).await}));Ok(())
    }
    pub(super) async fn received(&mut self,run:&ManagedRun,phase:Phase,im:&mut super::im::State,result:Result<OwnedImReply,RuntimeError>)->Result<(),&'static str>{
        // Consume the completed future before another iteration can poll it.
        self.reading.take();let query=self.query.take();let plan=self.gap.take();
        if !run.accepting.load(Ordering::SeqCst)||!run.im_gate.live()||phase!=Phase::Hosted{self.clear();return Ok(());}
        run.configuration_current()?;
        let reply=match result{Ok(reply)=>reply,Err(error)=>{
            let code=match error{RuntimeError::Timeout=>"app_account_im_sync_read_timeout",RuntimeError::Cancelled=>"app_account_im_sync_read_cancelled",_=>"app_account_im_sync_read_failed"};self.fail(run,code);return Ok(());
        }};
        let query=query.ok_or("app_account_im_scope_invalid")?;
        if !Arc::ptr_eq(&query,reply.request().query_arc()){return Err("app_account_im_scope_invalid");}
        let result=if let Some(plan)=plan{
            let cursor=self.cursor.as_ref().ok_or("app_account_im_cursor_invalid")?;
            match cursor.commit_gap(&run.database,&plan,&reply){
                Ok(committed)=>{
                    super::im::synchronized(run,im,committed.proofs()).await?;
                    if !self.receipt_failed&&run.backend.im_receipt_policy().is_some(){self.received_span=Some(committed);}
                    Ok(())
                },
                Err(code @ ("app_account_im_type_unsupported"|"app_account_im_message_invalid"|"app_account_im_receive_rejected"))=>{self.fail(run,code);Ok(())},
                Err(code)=>Err(code),
            }
        }else{
            if self.cursor.is_none(){self.cursor=Some(CursorSession::from_refresh(run.run_token.clone(),run.im_gate.clone(),&reply)?);}
            let cursor=self.cursor.as_mut().ok_or("app_account_im_cursor_invalid")?;
            match cursor.observe_refresh(&run.database,&reply){
                Ok(_)=>{if !cursor.initial(){self.fail(run,"app_account_im_status_baseline_missing");}},
                Err(code @ "app_account_im_refresh_rejected")=>self.fail(run,code),
                Err(code)=>return Err(code),
            }
            Ok(())
        };
        result
    }
}
