//! Actions-only mixed Info/Text original cursor atomicity and scope regressions.
use super::*;
use n::im_inbox::OnlinePosition;
fn no_epoch(f:&Fixture)->i64{f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_messages WHERE info_epoch IS NOT NULL",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap()}
#[test]
fn info_cursor_mixed_sorted_original_gap_commits_actual_maximum_and_each_typed_event(){
    let mut f=Fixture::with_info(true,true);let c=cursor(&mut f,9);let plan=c.next_gap(&f.temp.database).unwrap().unwrap();
    let r=read_reply(&mut f,plan.query(),&recv_result(&[(3,b"System.Info.Mixed",INFO),(1,b"Text",br#"{"text":"legacy"}"#),(2,b"Info.Mixed",PRIVATE_INFO)]));
    let done=c.commit_gap(&f.temp.database,&plan,&r).unwrap();assert_eq!(done.end(),3);assert_eq!(done.proofs().len(),3);assert!(done.is_current(&f.temp.database));
    assert_eq!(position(&f),(0,3,9));assert_eq!(f.count(),3);assert_eq!(event_count(&f),3);assert_eq!(no_epoch(&f),2);
    let rows=views(&f);assert_eq!(rows[0].system_info,Some(true));assert_eq!(rows[1].system_info,Some(false));assert_eq!(rows[2].kind,None);
    private_projection(&serde_json::to_string(&rows).unwrap());
    let messages=f.temp.database.get_recent_app_events(Some(f.run.line_id()),16).unwrap();let kinds=messages.iter().map(|e|serde_json::from_str::<serde_json::Value>(&e.payload_json).unwrap()["kind"].as_str().unwrap().to_owned()).collect::<Vec<_>>();assert_eq!(kinds,["text","info","info"]);
    let delivery=info(&mut f,1,b"P2P/1_2",2,b"Info.Mixed",PRIVATE_INFO);let proof=done.proofs().iter().find(|p|p.authorizes(delivery.ticket(),Instant::now())).unwrap();
    assert_eq!(result(f.backend.complete_im(delivery.ticket(),Some(proof)).unwrap()),[0,1,0,0,0,0]);assert_eq!(f.count(),3);no_other_grants(&f);
}
#[test]
fn info_cursor_invalid_whole_gap_never_updates_an_old_info_consumption_epoch(){
    let mut f=Fixture::with_info(true,true);let original=info(&mut f,1,b"P2P/1_2",1,b"Info.Saved",INFO);let proof=f.inbox(&original).commit(&f.temp.database,&original).unwrap().unwrap();
    let old=original.notice().binding().receipt_epoch().to_owned();assert_eq!(result(f.backend.complete_im(original.ticket(),Some(&proof)).unwrap()),[0,1,0,0,0,0]);
    next_native_login(&mut f,true,b"fixture-selector");let c=cursor(&mut f,2);let plan=c.next_gap(&f.temp.database).unwrap().unwrap();
    let r=read_reply(&mut f,plan.query(),&recv_result(&[(1,b"Info.Saved",INFO),(2,b"Info.Bad",br#"{"content":"new","OnlineMsg":true}"#)]));
    assert_eq!(c.commit_gap(&f.temp.database,&plan,&r).err(),Some("app_account_im_message_invalid"));assert_eq!(position(&f),(0,0,2));assert_eq!(f.count(),1);assert_eq!(event_count(&f),1);
    let stored:String=f.temp.database.with_conn(|c|c.query_row("SELECT info_epoch FROM app_voice_im_messages",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(stored,old);
    let hint=f.hint(1,99,b"P2P/1_2",1);assert!(f.inbox(&hint).lookup(&f.temp.database,&hint).unwrap().is_none());assert!(!proof.authorizes(hint.ticket(),Instant::now()));assert!(f.scope.is_open());
}
#[test]
fn info_cursor_unknown_kind_even_with_info_enabled_rolls_back_the_entire_original_gap(){
    let mut f=Fixture::with_info(true,true);let c=cursor(&mut f,2);let plan=c.next_gap(&f.temp.database).unwrap().unwrap();
    for kind in [b"Info".as_slice(),b"Other.Info.X",b"Invite",b"Info.bad/type"]{
        let r=read_reply(&mut f,plan.query(),&recv_result(&[(1,b"Info.Known",INFO),(2,kind,INFO)]));
        assert!(c.commit_gap(&f.temp.database,&plan,&r).is_err());assert_eq!(f.count(),0);assert_eq!(event_count(&f),0);assert_eq!(position(&f),(0,0,2));assert_eq!(no_epoch(&f),0);
    }
    assert!(f.scope.is_open());no_other_grants(&f);
}
#[test]
fn info_cursor_contiguous_online_info_waits_for_original_missing_gap_and_never_jumps(){
    let mut f=Fixture::with_info(true,true);let c=cursor(&mut f,0);let first=info(&mut f,1,b"P2P/1_2",1,b"Info.Online",INFO);
    assert!(matches!(c.online_position(&f.temp.database,&first).unwrap(),OnlinePosition::Next));let proof=c.commit_online(&f.temp.database,&first).unwrap().unwrap();assert!(proof.authorizes(first.ticket(),Instant::now()));assert_eq!(position(&f),(0,1,1));
    let later=info(&mut f,2,b"P2P/1_2",3,b"System.Info.Online",INFO);assert!(matches!(c.online_position(&f.temp.database,&later).unwrap(),OnlinePosition::Pending));assert!(c.commit_online(&f.temp.database,&later).is_err());assert_eq!(position(&f),(0,1,3));assert_eq!(f.count(),1);
    let plan=c.next_gap(&f.temp.database).unwrap().unwrap();let r=read_reply(&mut f,plan.query(),&recv_result(&[(2,b"Info.Online",INFO)]));let done=c.commit_gap(&f.temp.database,&plan,&r).unwrap();assert_eq!(done.end(),2);assert_eq!(position(&f),(0,2,3));
    assert!(matches!(c.online_position(&f.temp.database,&later).unwrap(),OnlinePosition::Next));let proof=c.commit_online(&f.temp.database,&later).unwrap().unwrap();assert_eq!(position(&f),(0,3,3));assert_eq!(f.count(),3);assert_eq!(event_count(&f),3);assert_eq!(no_epoch(&f),3);
    let duplicate=info(&mut f,3,b"P2P/1_2",3,b"System.Info.Online",INFO);assert!(matches!(c.online_position(&f.temp.database,&duplicate).unwrap(),OnlinePosition::Replay));let replay=c.commit_online(&f.temp.database,&duplicate).unwrap().unwrap();assert!(proof.authorizes(duplicate.ticket(),Instant::now()));assert!(replay.authorizes(duplicate.ticket(),Instant::now()));assert_eq!(f.count(),3);assert_eq!(event_count(&f),3);no_other_grants(&f);
}
#[test]
fn info_cursor_exact_query_owner_gate_scope_and_sql_lease_each_precede_info_commit(){
    for mode in 0..5{
        let mut f=Fixture::with_info(true,true);let c=cursor(&mut f,1);let plan=c.next_gap(&f.temp.database).unwrap().unwrap();
        let response=if mode==0{read_reply(&mut f,Arc::new(ImQuery::receive(n::im_data::ImBox::parse(b"P2P/1_2").unwrap(),1,1).unwrap()),&recv_result(&[(1,b"Info.Scope",INFO)]))}
        else if mode==1{let mut foreign=Fixture::with_info(true,true);read_reply(&mut foreign,plan.query(),&recv_result(&[(1,b"Info.Scope",INFO)]))}
        else{read_reply(&mut f,plan.query(),&recv_result(&[(1,b"Info.Scope",INFO)]))};
        match mode{2=>f.gate.retire(),3=>f.scope.mark_session_revoked(),4=>{f.temp.database.with_conn(|c|c.execute("DELETE FROM app_voice_runtime_leases",[]).map_err(|_|"fixture")).unwrap();},_=>{}}
        assert!(c.commit_gap(&f.temp.database,&plan,&response).is_err());assert_eq!(f.count(),0);assert_eq!(event_count(&f),0);assert_eq!(no_epoch(&f),0);assert_eq!(position(&f),(0,0,1));
    }
}
#[test]
fn info_cursor_info_subtype_and_epoch_bytes_share_unchanged_global_quota_with_atomic_text(){
    let mut f=Fixture::with_info(true,true);let c=cursor(&mut f,2);let plan=c.next_gap(&f.temp.database).unwrap().unwrap();
    let first=f.message(1,b"P2P/1_2",1,r#"{"text":"one"}"#);let second=info(&mut f,2,b"P2P/1_2",2,b"Info.Quota",INFO);
    let available=first.notice().raw().unwrap().len()+"one".len()+first.notice().message().unwrap().unwrap().sender_uid.len()+second.notice().raw().unwrap().len()+"visible info".len()+second.notice().message().unwrap().unwrap().sender_uid.len();
    f.temp.database.with_conn(|conn|conn.execute("WITH RECURSIVE ids(n) AS(VALUES(1) UNION ALL SELECT n+1 FROM ids WHERE n<128) INSERT INTO app_voice_im_messages(line_id,revision,identity_key,box_id,box_kind,message_index,fingerprint,original_time,body,text,sender,attachment_count,created_at) SELECT ?1,1,zeroblob(32),CAST('P2P/9_9' AS BLOB),'p2p',n,zeroblob(32),0,zeroblob(65536),CAST(zeroblob(CASE WHEN n=128 THEN 65536-?2 ELSE 65536 END) AS TEXT),X'',0,'fixture' FROM ids",rusqlite::params![f.run.line_id(),available as i64]).map_err(|_|"fixture")).unwrap();
    let response=read_reply(&mut f,plan.query(),&recv_result(&[(1,b"Text",br#"{"text":"one"}"#),(2,b"Info.Quota",INFO)]));
    assert_eq!(c.commit_gap(&f.temp.database,&plan,&response).err(),Some("app_account_im_inbox_full"));assert_eq!(f.count(),128);assert_eq!(event_count(&f),0);assert_eq!(position(&f),(0,0,2));assert_eq!(no_epoch(&f),0);assert!(f.scope.is_open());no_other_grants(&f);
}
