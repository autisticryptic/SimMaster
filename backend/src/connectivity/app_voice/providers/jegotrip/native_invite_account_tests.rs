//! Actions-only real Router/TLS/Common -> original CallService -> SQLite ->
//! original DIRECT/batch Invite receipts. No original SDK/phone/410 execution.
use super::*;
use n::{native_invite_notifications::NativeInviteConfig,account_call_test_support::{call_config,sdp,NATIVE_ID},rpc_calls::CallOperation};
use std::{future::Future,time::Duration};
#[path="native_invite_duplicate_account_tests.rs"]
mod duplicate_windows;
async fn bounded<F:Future>(future:F)->F::Output{tokio::time::timeout(Duration::from_secs(8),future).await.expect("original native Invite fixture budget")}
fn config(root:&str,port:u16,timeout_ms:u64)->AccountConfig{
    let mut cfg=account_config(root,port,false);cfg.profile.native_call_control=true;cfg.call_media=call_config(root,root).call_media;
    cfg.profile.native_receivers.as_mut().unwrap().invite_notifications=Some(NativeInviteConfig{timeout_ms});cfg
}
fn notice_arguments(id:u64)->Vec<u8>{let mut w=Writer::versioned().unwrap();w.u64(id).unwrap();w.properties(&[RpcProperty{key:b"Notify.Command",value:b"Invite"},RpcProperty{key:b"Notify.CallId",value:b"opaque-not-a-routing-grant"},RpcProperty{key:b"Notify.Caller",value:b"not-the-invitation-peer"}]).unwrap();w.finish().expose_to_transport().to_vec()}
fn notify(rpc:i32,id:u64)->Vec<u8>{let args=notice_arguments(id);let magic=rpc.to_be_bytes();let raw=encode_request(rpc,&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"token",value:b"fixture-auth"},RpcProperty{key:b"__magic",value:&magic}],n::rpc_client_ex::NOTIFY_CALLS_METHOD.as_bytes(),&args).unwrap();let mut packet=vec![0];packet.extend_from_slice(raw.expose_to_transport());packet}
fn invited(rpc:i32)->Vec<u8>{let sdp=String::from_utf8(sdp("127.0.0.1:5006".parse().unwrap())).unwrap().replace("RTP/AVP 0","RTP/AVP 8").replace("0 PCMU/8000","8 PCMA/8000");let mut w=Writer::versioned().unwrap();w.u64(NATIVE_ID).unwrap();w.bytes(b"fixture-caller").unwrap();w.properties(&[RpcProperty{key:b"callServerEx.oid",value:b"#native-invite-original"},RpcProperty{key:b"RseEx.SDP",value:sdp.as_bytes()}]).unwrap();direct(rpc,n::rpc_calls::INVITED_METHOD.as_bytes(),w.finish().expose_to_transport(),b"CallClientEx",None)}
async fn next(peer:&mut CommonPeer)->Vec<u8>{loop{let packet=peer.next().await.unwrap();if packet[0]==1{return packet;}let request=decode_request(&packet[1..]).unwrap();if request.method!=n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(){return packet;}let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-original-selector");let payload=n::rpc_batch::read_payload(&mut r).unwrap();assert!(payload.results.is_empty(),"no unobserved Invite result");peer.send(&[reply(request.call_id,&n::runtime_backend::tests::swap_result(None,true,payload.fields))]).await;}}
async fn expect_receipt(peer:&mut CommonPeer,id:i32,accepted:bool){let packet=next(peer).await;assert_eq!(packet[0],1);let result=decode_reply(&packet[1..]).unwrap();assert_eq!(result.call_id,id);assert_eq!(result.result_code,0);assert_eq!(result.result_stream,if accepted{[0,1,0,0,0,0]}else{[0;6]});}
fn control(packet:&[u8],operation:CallOperation)->i32{assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.method,operation.method().as_bytes());assert_eq!(request.metadata.len(),2);assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,if operation==CallOperation::Inform{b"#CallServerEx".as_slice()}else{b"#native-invite-original".as_slice()});let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.u64().unwrap(),NATIVE_ID);let properties=r.properties().unwrap();r.finish().unwrap();if operation==CallOperation::Inform{assert_eq!(properties.len(),1);assert_eq!(properties[0].key,b"RseEx.Info");assert_eq!(properties[0].value,b"Invited callId is exist?");}else{assert!(properties.is_empty());}request.call_id}
async fn answer(peer:&mut CommonPeer,id:i32,accepted:bool){peer.send(&[reply(id,if accepted{&[0,1,0,0,0,0]}else{&[0;6]})]).await;}
fn consumed(db:&crate::platform::db::Database,line:&str)->i64{db.with_conn(|conn|conn.query_row("SELECT COUNT(*) FROM app_voice_invite_notification_receipts WHERE line_id=?1",[line],|row|row.get(0)).map_err(|_|"test_receipt_query")).unwrap()}
async fn positive_batch_duplicate(peer:&mut CommonPeer,db:&crate::platform::db::Database,line:&str){
    let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let swap=decode_request(&packet[1..]).unwrap();assert_eq!(swap.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let mut r=Reader::versioned(swap.arguments).unwrap();r.bytes().unwrap();r.bytes().unwrap();let payload=n::rpc_batch::read_payload(&mut r).unwrap();assert!(payload.results.is_empty());
    let mut w=Writer::new();w.boolean(true).unwrap();for word in [9,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{w.u32(word).unwrap();}w.u32(1).unwrap();w.i32(1).unwrap();w.bytes(n::rpc_client_ex::NOTIFY_CALLS_METHOD.as_bytes()).unwrap();w.bytes(&notice_arguments(NATIVE_ID)).unwrap();w.properties(&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"token",value:b"fixture-auth"}]).unwrap();w.u32(0).unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();peer.send(&[reply(swap.call_id,w.finish().expose_to_transport())]).await;
    loop{let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let swap=decode_request(&packet[1..]).unwrap();assert_eq!(swap.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes());let mut r=Reader::versioned(swap.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-original-selector");let payload=n::rpc_batch::read_payload(&mut r).unwrap();let result=payload.results.get(&1);if let Some(result)=result{assert_eq!(result.result_stream,[0,1,0,0,0,0]);assert_eq!(consumed(db,line),1);}
        peer.send(&[reply(swap.call_id,&n::runtime_backend::tests::swap_result(None,true,payload.fields))]).await;if result.is_some(){return;}}
}
async fn scenario(mode:u8,reuse_deleted_id:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());
    let account=manager.create(config(&cert.root,port,if mode==1||mode==3{100}else{3000}),100).await.unwrap().account;let line=account.line_id.clone();let peer_line=line.clone();let db=temp.database.clone();
    let(go,wait)=tokio::sync::oneshot::channel();let(checking,at_inform)=tokio::sync::oneshot::channel();let(allow,confirmed)=tokio::sync::oneshot::channel();let(received,seen)=tokio::sync::oneshot::channel();let(stop,finish)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{
        let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");wait.await.unwrap();
        if mode==1 {peer.send(&[notify(91,NATIVE_ID)]).await;checking.send(()).unwrap();expect_receipt(&mut peer,91,false).await;assert_eq!(consumed(&db,&peer_line),0);received.send(()).unwrap();finish.await.unwrap();finish_account(&mut peer).await;return;}
        peer.send(&[notify(91,NATIVE_ID)]).await;if mode==0{peer.send(&[notify(92,NATIVE_ID)]).await;}peer.send(&[invited(93)]).await;
        // Neither the notification nor invitation receipt may complete the
        // business holder before the matching Inform/local call indication.
        expect_receipt(&mut peer,93,true).await;let inform=control(&next(&mut peer).await,CallOperation::Inform);assert_eq!(consumed(&db,&peer_line),0);checking.send(()).unwrap();
        if mode==3{expect_receipt(&mut peer,91,false).await;}
        confirmed.await.unwrap();answer(&mut peer,inform,mode!=2).await;
        if mode==0{expect_receipt(&mut peer,91,true).await;expect_receipt(&mut peer,92,true).await;assert_eq!(consumed(&db,&peer_line),1);positive_batch_duplicate(&mut peer,&db,&peer_line).await;received.send(()).unwrap();}
        else if mode==2{
            let mut nack=false;let mut released=false;while !nack||!released{let packet=next(&mut peer).await;if packet[0]==1{let reply=decode_reply(&packet[1..]).unwrap();assert_eq!(reply.call_id,91);assert_eq!(reply.result_stream,[0;6]);nack=true;}else{let release=control(&packet,CallOperation::Release);answer(&mut peer,release,true).await;released=true;}}
            assert_eq!(consumed(&db,&peer_line),0);received.send(()).unwrap();finish.await.unwrap();finish_account(&mut peer).await;return;
        }else{if mode==4{expect_receipt(&mut peer,91,false).await;}assert_eq!(consumed(&db,&peer_line),0);received.send(()).unwrap();}
        let release=control(&next(&mut peer).await,CallOperation::Release);answer(&mut peer,release,true).await;finish.await.unwrap();finish_account(&mut peer).await;
    }).await});
    bounded(manager.start(&line,account.revision,100)).await.unwrap();bounded(async{loop{if manager.get(&line,100).await.unwrap().incoming_call_ready{return;}tokio::task::yield_now().await;}}).await;go.send(()).unwrap();bounded(at_inform).await.unwrap();
    if mode!=1{assert_eq!(consumed(&temp.database,&line),0);let call=manager.calls(&line).await.unwrap().into_iter().next().unwrap();assert_eq!(call.phase,"checking");assert!(call.sdk_invite_notification.is_none());assert!(!call.media_active);
        if mode==4{temp.database.with_conn(|conn|{conn.execute("DELETE FROM call_history WHERE id=?1 AND line_id=?2",rusqlite::params![call.history_id,line]).map_err(|_|"test_delete")?;if reuse_deleted_id{conn.execute("INSERT INTO call_history(id,line_id,direction,phone_number,duration,start_time,answered,app_call_token) VALUES(?1,?2,'incoming','replacement',0,'fixture',0,?3)",rusqlite::params![call.history_id,line,"bb".repeat(16)]).map_err(|_|"test_replace")?;}Ok(())}).unwrap();}
        allow.send(()).unwrap();
    }
    bounded(seen).await.unwrap();
    if mode!=1&&mode!=2{let call=bounded(async{loop{if let Some(call)=manager.calls(&line).await.unwrap().into_iter().find(|call|call.phase=="ringing"){return call;}tokio::task::yield_now().await;}}).await;assert!(!call.media_active);assert_eq!(call.peer,"fixture-caller");assert_eq!(call.sdk_invite_notification,if mode==0{Some("consumed")}else{None});assert_eq!(consumed(&temp.database,&line),i64::from(mode==0));assert_eq!(manager.hangup_call(&line,account.revision,&call.id,100).await.unwrap().phase,"ended");}
    if mode==1{assert!(manager.calls(&line).await.unwrap().is_empty());assert_eq!(temp.database.get_call_history_for_line(&line,10,0).unwrap().len(),0);}
    assert!(manager.messages(&line,10).unwrap().is_empty(),"Invite must not create SMS rows");assert!(!manager.get(&line,100).await.unwrap().outbound_authorized);stop.send(()).unwrap();let stopped=bounded(manager.stop(&line,account.revision,100)).await.unwrap();assert!(!stopped.io_cleanup_required);bounded(serving).await.unwrap();assert!(!temp.database.with_conn(|conn|n::account_runs::ownership_pending(conn,&line)).unwrap());
}
#[tokio::test] async fn native_invite_actual_inform_indication_durable_association_precedes_all_direct_batch_receipts(){Box::pin(scenario(0,false)).await;}
#[tokio::test] async fn native_invite_missing_call_expires_negative_without_history_hint_sms_or_media(){Box::pin(scenario(1,false)).await;}
#[tokio::test] async fn native_invite_rejected_inform_nacks_and_preserves_actual_call_release_join(){Box::pin(scenario(2,false)).await;}
#[tokio::test] async fn native_invite_late_positive_inform_cannot_revive_expired_notification(){Box::pin(scenario(3,false)).await;}
#[tokio::test] async fn native_invite_deleted_call_history_prevents_positive_receipt_or_resurrection(){Box::pin(scenario(4,false)).await;}
#[tokio::test] async fn native_invite_reused_history_id_cannot_inherit_old_call_token_or_consumption(){Box::pin(scenario(4,true)).await;}
#[tokio::test]
async fn native_invite_stop_retires_pending_claim_without_ack_and_waits_real_sdk_join(){
    Box::pin(async {
        let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
        let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let account=manager.create(config(&cert.root,port,30000),100).await.unwrap().account;
        let(go,wait)=tokio::sync::oneshot::channel();let serving=tokio::spawn(async move{bounded(async move{
            let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");wait.await.unwrap();peer.send(&[notify(91,NATIVE_ID)]).await;
            // The original helper rejects any premature notification reply and
            // requires actual unregister3, logoutSession result and peer EOF.
            finish_account(&mut peer).await;
        }).await});
        bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();
        bounded(async{while !manager.get(&account.line_id,100).await.unwrap().incoming_call_ready{tokio::task::yield_now().await;}}).await;go.send(()).unwrap();
        let service=manager.original_call_service(&account.line_id).unwrap();bounded(async{while service.invite_holders_for_test()==0{tokio::task::yield_now().await;}}).await;
        assert_eq!(consumed(&temp.database,&account.line_id),0);let stopped=bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();assert!(!stopped.io_cleanup_required);bounded(serving).await.unwrap();
        assert_eq!(service.invite_holders_for_test(),0);assert!(service.resources_released());assert!(manager.calls(&account.line_id).await.is_err());assert!(!temp.database.with_conn(|conn|n::account_runs::ownership_pending(conn,&account.line_id)).unwrap());
    }).await;
}

