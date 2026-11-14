//! Actions-only native account + independent TLS/Mpath + PCMA + heartbeat Join.
use super::*;
use n::{native_call_lifecycle::NativeCallLifecycleConfig,account_call_test_support::{call_config,sdp,NATIVE_ID},
    native_audio_sdp::PlainAudioOffer,tcp_mpath_audio::tests::Peer as MediaPeer,tcp_mpath::{self,Input as MpathInput},rpc_calls::CallOperation};
use std::{future::Future,sync::Arc,time::Instant};
use tokio::net::UdpSocket;
async fn bounded<F:Future>(future:F)->F::Output{tokio::time::timeout(Duration::from_secs(8),future).await.expect("native lifecycle fixture deadline")}
fn pcma_sdp(endpoint:std::net::SocketAddr)->Vec<u8>{String::from_utf8(sdp(endpoint)).unwrap().replace("RTP/AVP 0","RTP/AVP 8").replace("0 PCMU/8000","8 PCMA/8000").into_bytes()}
fn audio(sequence:u16)->Vec<u8>{let mut bytes=vec![0x80,8,0,0,0,0,0,160,0x12,0x34,0x56,0x78];bytes[2..4].copy_from_slice(&sequence.to_be_bytes());bytes.resize(172,0xd5);bytes}
async fn non_poll(peer:&mut CommonPeer)->Vec<u8>{loop{
    let packet=peer.next().await.unwrap();if packet[0]==1{return packet;}
    let request=decode_request(&packet[1..]).unwrap();if request.method!=n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(){return packet;}
    let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-original-selector");
    let payload=n::rpc_batch::read_payload(&mut r).unwrap();peer.send(&[reply(request.call_id,&n::runtime_backend::tests::swap_result(None,true,payload.fields))]).await;
}}
async fn control(peer:&mut CommonPeer,operation:CallOperation)->(i32,Vec<(Vec<u8>,Vec<u8>)>){
    let packet=non_poll(peer).await;assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.method,operation.method().as_bytes());
    assert_eq!(request.metadata.len(),2);assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");
    let target=if operation==CallOperation::Inform{b"#CallServerEx".as_slice()}else{b"#native-original-call".as_slice()};
    assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,target);assert!(!request.metadata.iter().any(|p|p.key==b"domain"||p.key==b"pwd"));
    let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.u64().unwrap(),NATIVE_ID);let fields=r.properties().unwrap().iter().map(|p|(p.key.to_vec(),p.value.to_vec())).collect();r.finish().unwrap();
    (request.call_id,fields)
}
async fn answer(peer:&mut CommonPeer,id:i32,operation:CallOperation,accepted:bool,endpoint:Option<&str>){
    let mut w=Writer::new();w.boolean(accepted).unwrap();if operation==CallOperation::Accept{
        let fields=endpoint.map(|endpoint|vec![RpcProperty{key:b"endpoint",value:endpoint.as_bytes()},RpcProperty{key:b"content",value:b"fixture-original-content"}]).unwrap_or_default();w.properties(&fields).unwrap();
    }w.bytes(b"").unwrap();peer.send(&[reply(id,w.finish().expose_to_transport())]).await;
}
fn field<'a>(fields:&'a[(Vec<u8>,Vec<u8>)],key:&[u8])->Option<&'a[u8]>{fields.iter().find(|(k,_)|k==key).map(|(_,v)|v.as_slice())}
async fn call_phase(manager:&AccountManager,line:&str,id:&str,wanted:&str){bounded(async{loop{if manager.call(line,id).await.unwrap().phase==wanted{return;}tokio::task::yield_now().await;}}).await;}
async fn scenario(mode:u8){
    let sdk=Certificates::generate("IP:127.0.0.1");let media=Certificates::generate("IP:127.0.0.1");
    let router=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=router.local_addr().unwrap().port();let server=sdk.server_config(&TLS13);
    let media_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let endpoint=format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};",media_listener.local_addr().unwrap().port());
    let operator=UdpSocket::bind("127.0.0.1:0").await.unwrap();let operator_addr=operator.local_addr().unwrap();
    let temp=TempDatabase::new();let manager=AccountManager::new(temp.database.clone());let mut config=account_config(&sdk.root,port,false);
    config.profile.native_call_control=true;config.call_media=call_config(&sdk.root,&media.root).call_media;
    config.call_media.as_mut().unwrap().native_lifecycle=Some(NativeCallLifecycleConfig{session_timer_seconds:1});
    let account=manager.create(config,100).await.unwrap().account;let line=account.line_id;let revision=account.revision;
    let(invite,go)=tokio::sync::oneshot::channel();let(heartbeat,seen_heartbeat)=tokio::sync::oneshot::channel();let(late,wait_late)=tokio::sync::oneshot::channel();let(done,stop)=tokio::sync::oneshot::channel();
    let serving=tokio::spawn(async move{bounded(async move{
        let mut peer=CommonPeer::open(router,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();peer.send(&[reply(login.call_id,&login_result())]).await;
        let(_,flag)=hosting_call(&mut peer).await;assert_eq!(flag,b"4");go.await.unwrap();
        let sdp=pcma_sdp("127.0.0.1:5006".parse().unwrap());let mut w=Writer::versioned().unwrap();w.u64(NATIVE_ID).unwrap();w.bytes(b"fixture-caller").unwrap();
        w.properties(&[RpcProperty{key:b"callServerEx.oid",value:b"#native-original-call"},RpcProperty{key:b"RseEx.SDP",value:&sdp}]).unwrap();
        peer.send(&[direct(91,n::rpc_calls::INVITED_METHOD.as_bytes(),w.finish().expose_to_transport(),b"CallClientEx",None)]).await;
        let p=non_poll(&mut peer).await;assert_eq!(p[0],1);assert_eq!(decode_reply(&p[1..]).unwrap().result_stream,[0,1,0,0,0,0]);
        let(id,fields)=control(&mut peer,CallOperation::Inform).await;assert_eq!(field(&fields,b"RseEx.Info"),Some(b"Invited callId is exist?".as_slice()));answer(&mut peer,id,CallOperation::Inform,true,None).await;
        let(id,fields)=control(&mut peer,CallOperation::Accept).await;assert_eq!(fields.len(),4);assert_eq!(field(&fields,b"mediaType"),Some(b"audio".as_slice()));
        assert_eq!(field(&fields,b"keepAlive"),Some(b"1000".as_slice()));assert_eq!(field(&fields,b"SDK.IsNew"),Some(b"1".as_slice()));
        let native=field(&fields,b"RseEx.SDP").unwrap();assert!(std::str::from_utf8(native).unwrap().contains("PCMA/8000"));assert!(PlainAudioOffer::trunk(native,true).is_ok());
        answer(&mut peer,id,CallOperation::Accept,true,Some(&endpoint)).await;
        let(id,fields)=control(&mut peer,CallOperation::KeepAlive).await;assert_eq!(field(&fields,b"keepAlive"),Some(b"1000".as_slice()));
        assert!(field(&fields,b"RseEx.AcceptDuration").is_some());
        if mode==2{heartbeat.send(()).unwrap();wait_late.await.unwrap();answer(&mut peer,id,CallOperation::KeepAlive,true,None).await;}
        else if mode==1{answer(&mut peer,id,CallOperation::KeepAlive,false,None).await;heartbeat.send(()).unwrap();}
        else{
            let mut conn_reports=usize::from(field(&fields,b"RseEx.ConnDuration").is_some());answer(&mut peer,id,CallOperation::KeepAlive,true,None).await;
            // An accepted-period tick may precede MediaReady. Observe the real
            // one-shot connected report, then at least one periodic-only tick.
            loop{let(id,fields)=control(&mut peer,CallOperation::KeepAlive).await;assert!(field(&fields,b"RseEx.AcceptDuration").is_none());
                if field(&fields,b"RseEx.ConnDuration").is_some(){conn_reports+=1;}assert!(conn_reports<=1);
                let only_timer=fields.len()==1;answer(&mut peer,id,CallOperation::KeepAlive,true,None).await;
                if conn_reports==1&&only_timer{break;}}
            heartbeat.send(()).unwrap();
        }
        let(id,fields)=control(&mut peer,CallOperation::Release).await;assert!(fields.is_empty());answer(&mut peer,id,CallOperation::Release,true,None).await;
        stop.await.unwrap();finish_account(&mut peer).await;
    }).await;});
    let down=audio(7);let up=audio(8);let down_wire=down.clone();let up_wire=up.clone();let(media_sent,media_observed)=tokio::sync::oneshot::channel();let media_server=media.server(&tokio_rustls::rustls::version::TLS13);
    let media_task=tokio::spawn(async move{bounded(async move{
        let mut peer=MediaPeer::accept(media_listener,media_server).await;let request=peer.next_rpc().await.unwrap().unwrap();n::account_call_test_support::assert_mpath_request(request.payload.expose_to_transport());peer.reply(true).await;
        if mode==0{let(packets,_)=tcp_mpath::encode_payload(0,0x3fff,1234,0,&down_wire).unwrap();peer.send_raw(packets[0].expose_to_transport()).await;
            let raw=peer.next_raw().await.unwrap().unwrap();let mut receiver=tcp_mpath::Receiver::default();let Some(MpathInput::Payload{channel:0,payload,..})=receiver.on_packet(&raw,Instant::now()).unwrap()else{panic!("PCMA Mpath payload")};assert_eq!(payload.expose_to_transport(),up_wire);}
        let _=media_sent.send(());peer.eof().await;
    }).await;});
    bounded(manager.start(&line,revision,100)).await.unwrap();bounded(async{loop{if manager.get(&line,100).await.unwrap().incoming_call_ready{return;}tokio::task::yield_now().await;}}).await;
    invite.send(()).unwrap();let call=bounded(async{loop{if let Some(call)=manager.calls(&line).await.unwrap().into_iter().find(|c|c.phase=="ringing"){return call;}tokio::task::yield_now().await;}}).await;
    let offered=PlainAudioOffer::trunk(call.sdp.as_ref().unwrap().as_bytes(),true).unwrap().endpoint().unwrap();let result=bounded(manager.answer_call(&line,revision,&call.id,pcma_sdp(operator_addr),100)).await;
    if mode==1{assert!(result.is_ok()||result.as_ref().err()==Some(&"app_account_call_scope_invalid"));bounded(seen_heartbeat).await.unwrap();call_phase(&manager,&line,&call.id,"ended").await;}
    else{
        assert!(result.unwrap().media_active);bounded(seen_heartbeat).await.unwrap();
        if mode==0{let mut data=[0;4096];let(count,from)=bounded(operator.recv_from(&mut data)).await.unwrap();assert_eq!(from,offered);assert_eq!(&data[..count],down);operator.send_to(&up,offered).await.unwrap();bounded(media_observed).await.unwrap();}
        if mode==2{
            let stopped=manager.hangup_call(&line,revision,&call.id,100);tokio::pin!(stopped);
            tokio::select!{result=stopped.as_mut()=>panic!("pending original heartbeat must be joined: {result:?}"),_=tokio::time::sleep(Duration::from_millis(30))=>{}}
            assert!(!manager.call(&line,&call.id).await.unwrap().media_active);late.send(()).unwrap();assert_eq!(bounded(stopped).await.unwrap().phase,"ended");
        }else{assert_eq!(bounded(manager.hangup_call(&line,revision,&call.id,100)).await.unwrap().phase,"ended");}
    }
    bounded(media_task).await.unwrap();assert!(!manager.call(&line,&call.id).await.unwrap().media_active);let rebound=UdpSocket::bind(offered).await.unwrap();assert_eq!(rebound.local_addr().unwrap(),offered);
    assert!(temp.database.get_call_by_id(call.history_id).unwrap().unwrap().end_time.is_some());done.send(()).unwrap();bounded(manager.stop(&line,revision,100)).await.unwrap();bounded(serving).await.unwrap();
    assert!(!temp.database.with_conn(|c|n::account_runs::ownership_pending(c,&line)).unwrap());
}
#[tokio::test]
async fn native_lifecycle_accept_pcma_bidirectional_media_periodic_keepalive_and_stop_join(){Box::pin(scenario(0)).await;}
#[tokio::test]
async fn native_lifecycle_negative_keepalive_retires_original_media_and_call_history(){Box::pin(scenario(1)).await;}
#[tokio::test]
async fn native_lifecycle_hangup_joins_pending_heartbeat_without_late_media_revival(){Box::pin(scenario(2)).await;}
