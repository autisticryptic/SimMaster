//! Actions-only new Info epoch/explicit-user-read ownership regressions.
//! Current payload consumption is necessary, never a WRITE consent or SDK grant.
use super::*;
use n::im_inbox::{BoundUserRead,UserReadAction};
fn user_action(f:&Fixture,id:i64,nonce:&str)->Arc<UserReadAction>{UserReadAction::original(f.temp.database.clone(),f.run.clone(),f.gate.clone(),f.owner.clone(),f.scope.clone(),id,nonce,Instant::now()+Duration::from_secs(3)).unwrap()}
fn user_state(f:&Fixture)->(i64,Option<i32>){f.temp.database.with_conn(|c|c.query_row("SELECT state,request_id FROM app_voice_im_user_reads",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"fixture")).unwrap()}
fn epoch(f:&Fixture,id:i64)->Option<String>{f.temp.database.with_conn(|c|c.query_row("SELECT info_epoch FROM app_voice_im_messages WHERE id=?1",[id],|r|r.get(0)).map_err(|_|"fixture")).unwrap()}
fn target_reply(f:&mut Fixture,a:&Arc<UserReadAction>,query:Arc<ImQuery>)->OwnedImReply{
    let p=f.backend.prepare_im_read(query,a.gate(),a.deadline()).unwrap();
    f.backend.on_plaintext_packet(&f.owner,&f.scope,&reply_packet(p.call_id,&refresh_result(7)),0).unwrap();
    let mut r=f.backend.take_im_replies();assert_eq!(r.len(),1);r.pop().unwrap()
}
#[test]
fn info_runtime_new_epoch_requires_real_opted_in_payload_before_a_tombstone_receipt(){
    for enabled in [false,true]{
        let mut f=Fixture::with_info(true,true);let first=info(&mut f,1,b"P2P/1_2",7,b"Info.Epoch",INFO);
        let original=f.inbox(&first).commit(&f.temp.database,&first).unwrap().unwrap();let row=views(&f).pop().unwrap();
        let old_epoch=epoch(&f,row.id).unwrap();assert_eq!(old_epoch,first.notice().binding().receipt_epoch());
        f.temp.database.with_conn(|c|n::im_inbox::delete(c,f.run.line_id(),row.id)).unwrap();
        assert_eq!(result(f.backend.complete_im(first.ticket(),Some(&original)).unwrap()),[0,1,0,0,0,0]);
        next_native_login(&mut f,enabled,b"fixture-selector");
        let notice=f.hint(1,100,b"P2P/1_2",7);assert_ne!(notice.notice().binding().receipt_epoch(),old_epoch);
        assert!(f.inbox(&notice).lookup(&f.temp.database,&notice).unwrap().is_none());
        assert!(!original.authorizes(notice.ticket(),Instant::now()));assert_eq!(epoch(&f,row.id).as_deref(),Some(old_epoch.as_str()));
        assert_eq!(result(f.backend.complete_im(notice.ticket(),None).unwrap()),[0;6]);
        let replay=info(&mut f,2,b"P2P/1_2",7,b"Info.Epoch",INFO);let proof=f.inbox(&replay).commit(&f.temp.database,&replay).unwrap();
        assert_eq!(proof.is_some(),enabled);
        assert_eq!(result(f.backend.complete_im(replay.ticket(),proof.as_ref()).unwrap()),if enabled{[0,1,0,0,0,0]}else{[0;6]});
        let later=f.hint(3,101,b"P2P/1_2",7);let receipt=f.inbox(&later).lookup(&f.temp.database,&later).unwrap();assert_eq!(receipt.is_some(),enabled);
        if enabled{assert_eq!(epoch(&f,row.id).as_deref(),Some(later.notice().binding().receipt_epoch()));}else{assert_eq!(epoch(&f,row.id).as_deref(),Some(old_epoch.as_str()));}
        assert_eq!(result(f.backend.complete_im(later.ticket(),receipt.as_ref()).unwrap()),if enabled{[0,1,0,0,0,0]}else{[0;6]});
        assert!(views(&f).is_empty());assert_eq!(f.count(),1);assert_eq!(event_count(&f),1);assert!(f.scope.is_open());no_other_grants(&f);
    }
}
#[test]
fn info_runtime_current_body_fresh_target_and_unknown_user_write_stay_independent_across_epochs(){
    let mut f=Fixture::with_info(true,true);let first=info(&mut f,1,b"P2P/1_2",7,b"Info.User",INFO);
    let proof=f.inbox(&first).commit(&f.temp.database,&first).unwrap().unwrap();let id=views(&f)[0].id;
    assert_eq!(result(f.backend.complete_im(first.ticket(),Some(&proof)).unwrap()),[0,1,0,0,0,0]);no_other_grants(&f);
    let a=user_action(&f,id,&"a".repeat(32));let BoundUserRead::Context(context)=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap()else{panic!("explicit action context")};
    let response=target_reply(&mut f,&a,context.query());let permit=context.admit(&response).unwrap();assert_eq!(user_state(&f),(0,None));
    let write=f.backend.prepare_user_read_write(permit.clone(),a.deadline()).unwrap();assert!(permit.begin_write(write.call_id));assert!(permit.submitted());
    a.cancel();assert!(!f.scope.is_open());assert_eq!(user_state(&f),(0,Some(write.call_id)));
    next_native_login(&mut f,true,b"fixture-selector");
    let before=user_action(&f,id,&"b".repeat(32));assert_eq!(before.bind_current(f.backend.captured_im_binding().unwrap()).err(),Some("app_account_im_user_read_message_missing"));
    let replay=info(&mut f,1,b"P2P/1_2",7,b"Info.User",INFO);let proof=f.inbox(&replay).commit(&f.temp.database,&replay).unwrap().unwrap();
    assert_eq!(result(f.backend.complete_im(replay.ticket(),Some(&proof)).unwrap()),[0,1,0,0,0,0]);
    let blocked=user_action(&f,id,&"b".repeat(32));assert_eq!(blocked.bind_current(f.backend.captured_im_binding().unwrap()).err(),Some("app_account_im_user_read_uncertain"));
    let exact=user_action(&f,id,&"a".repeat(32));let BoundUserRead::Replay(view)=exact.bind_current(f.backend.captured_im_binding().unwrap()).unwrap()else{panic!("metadata-only exact nonce")};
    assert_eq!(view.state,"uncertain");assert_eq!(view.message_id,id);assert_eq!(user_state(&f),(0,Some(write.call_id)));
    assert_eq!(f.count(),1);assert_eq!(event_count(&f),1);assert!(f.scope.is_open());
    let rows=f.temp.database.with_conn(|c|n::im_inbox::user_read_history(c,f.run.line_id(),128)).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].state,"uncertain");
}
#[test]
fn info_runtime_info_epoch_is_not_a_fresh_user_target_query_or_remote_write_permission(){
    let mut f=Fixture::with_info(true,true);let message=info(&mut f,1,b"P2P/1_2",7,b"Info.NoGrant",PRIVATE_INFO);
    f.inbox(&message).commit(&f.temp.database,&message).unwrap().unwrap();let id=views(&f)[0].id;
    let a=user_action(&f,id,&"a".repeat(32));let BoundUserRead::Context(context)=a.bind_current(f.backend.captured_im_binding().unwrap()).unwrap()else{panic!("only a context, not a permit")};
    let different_query=Arc::new(ImQuery::refresh(&[],-1).unwrap());assert!(!Arc::ptr_eq(&different_query,&context.query()));
    let response=target_reply(&mut f,&a,different_query);assert!(context.admit(&response).is_err());
    no_other_grants(&f);assert_eq!(f.count(),1);assert!(f.scope.is_open());
}
