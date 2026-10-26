//! Actions-only actual Asterisk-side SIP UDP dialog joined to the original SDK
//! call, Endpoint TCP/TLS/Mpath and both RTP directions. No live PBX/service.
use super::*;
use crate::{platform::config::{TrunkProfileConfig,TrunkRegistrationMode,TrunkIncomingMode},connectivity::core::sip_frame};
async fn sip_frame_from(socket:&UdpSocket)->(Vec<u8>,std::net::SocketAddr){bounded(async{loop{let mut bytes=[0;8192];let(count,source)=socket.recv_from(&mut bytes).await.unwrap();
    if bytes[..count].iter().all(u8::is_ascii_whitespace){continue;}return(bytes[..count].to_vec(),source);}}).await}
async fn real_trunk(native_hangup:bool){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let asterisk=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();let asterisk_addr=asterisk.local_addr().unwrap();let rtp_peer=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();
    let reserved=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();let sip_port=reserved.local_addr().unwrap().port();drop(reserved);
    let listener=TcpListener::bind(("127.0.0.1",0)).await.unwrap();let endpoint=format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};",listener.local_addr().unwrap().port());
    let mut config=call_config(&sdk.root,&media.root);config.call_media.as_mut().unwrap().trunk=Some(TrunkProfileConfig{
        enabled:true,registration_mode:TrunkRegistrationMode::StaticPeer,asterisk_host:asterisk_addr.ip().to_string(),asterisk_port:asterisk_addr.port(),
        local_port:sip_port,username:"41000".into(),incoming_binding:"6108".into(),incoming_mode:TrunkIncomingMode::BoundPending,
        codec_allow:vec!["pcmu".into()],..TrunkProfileConfig::default()});
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);let account=manager.create(config,100).await.unwrap().account;
    let(go,invite)=oneshot::channel();let(native_end,end)=oneshot::channel();let(done,stop)=oneshot::channel();let(local,local_seen)=oneshot::channel();
    let sdk_task=tokio::spawn(async move{bounded(async move{
        peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;receipt(&mut peer,101,true).await;
        let(id,_)=operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true,None).await;
        let(id,answer)=operation(&mut peer,CallOperation::Accept).await;let address=PlainAudioOffer::trunk(&answer,true).unwrap().endpoint().unwrap();local.send(address).unwrap();reply_operation(&mut peer,id,CallOperation::Accept,true,Some(endpoint.as_bytes())).await;
        if native_hangup{
            end.await.unwrap();notice(&mut peer,CallNoticeKind::Released,102,&[],b"fixture-domain").await;
            let packet=next_non_poll(&mut peer).await;assert_eq!(packet.kind,CallKind::Reply);let reply=decode_reply(packet.payload.expose_to_transport()).unwrap();assert_eq!(reply.call_id,102);
            assert_eq!(reply.result_stream,encode_notice_result(CallNoticeKind::Released,true,&[]).unwrap().expose_to_transport());
        }else{let(id,_)=operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true,None).await;}
        stop.await.unwrap();finish_sdk(peer).await;
    }).await});
    let down=rtp(23,0x12);let up=rtp(24,0x34);let native_down=down.clone();let native_up=up.clone();let server=media.server(&rustls::version::TLS13);let(passed,saw_audio)=oneshot::channel();
    let media_task=tokio::spawn(async move{bounded(async move{
        let mut peer=MediaPeer::accept(listener,server).await;original_endpoint(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());peer.reply(true).await;
        let(packets,_)=tcp_mpath::encode_payload(0,22,1200,0,&native_down).unwrap();peer.send_raw(packets[0].expose_to_transport()).await;
        let raw=peer.next_raw().await.unwrap().unwrap();let mut decoder=tcp_mpath::Receiver::default();let Some(MpathInput::Payload{channel:0,payload,..})=decoder.on_packet(&raw,Instant::now()).unwrap()else{panic!("actual original uplink")};
        assert_eq!(payload.expose_to_transport(),native_up);passed.send(()).unwrap();peer.eof().await;
    }).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;let link=manager.call_operator(&account.line_id).await.unwrap();
    go.send(()).unwrap();let(invite,sip_addr)=sip_frame_from(&asterisk).await;assert!(invite.starts_with(b"INVITE sip:6108@127.0.0.1:"));assert_eq!(sip_addr.port(),sip_port);
    let offered=PlainAudioOffer::trunk(sip_frame::body(&invite),true).unwrap().endpoint().unwrap();assert!(sip_frame::body(&invite).windows(12).any(|window|window==b"a=rtcp-mux\r\n"));
    let via=sip_frame::header_value(&invite,"Via").unwrap();let from=sip_frame::header_value(&invite,"From").unwrap();assert!(from.contains("sip:fixture-caller@app-account.invalid"));let to=sip_frame::header_value(&invite,"To").unwrap();let call_id=sip_frame::header_value(&invite,"Call-ID").unwrap();let cseq=sip_frame::header_value(&invite,"CSeq").unwrap();
    let answer=sdp(rtp_peer.local_addr().unwrap());let response=format!("SIP/2.0 200 OK\r\nVia: {via}\r\nFrom: {from}\r\nTo: {to};tag=fixture-pbx\r\nCall-ID: {call_id}\r\nCSeq: {cseq}\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",answer.len(),std::str::from_utf8(&answer).unwrap());
    asterisk.send_to(response.as_bytes(),sip_addr).await.unwrap();let(ack,_)=sip_frame_from(&asterisk).await;assert!(ack.starts_with(b"ACK sip:"));assert_eq!(bounded(local_seen).await.unwrap(),offered);
    let call=bounded(async{loop{if let Some(call)=manager.calls(&account.line_id).await.unwrap().into_iter().find(|call|call.phase=="connected"){return call;}tokio::task::yield_now().await;}}).await;assert!(call.media_active);
    let mut bytes=[0;4096];let(count,source)=bounded(rtp_peer.recv_from(&mut bytes)).await.unwrap();assert_eq!(source,offered);assert_eq!(&bytes[..count],down);rtp_peer.send_to(&up,offered).await.unwrap();bounded(saw_audio).await.unwrap();
    // Same valid SIP peer still cannot turn basic receive readiness into an
    // outbound SDK call or paid IMS/CS fallback.
    let outgoing=format!("INVITE sip:fixture-outbound@simadmin SIP/2.0\r\nVia: SIP/2.0/UDP {asterisk_addr};branch=z9hG4bKfixture-out\r\nFrom: <sip:6108@pbx>;tag=fixture-out\r\nTo: <sip:41000@simadmin>\r\nCall-ID: fixture-outbound\r\nCSeq: 1 INVITE\r\nContent-Type: application/sdp\r\nContent-Length: {}\r\n\r\n{}",answer.len(),std::str::from_utf8(&answer).unwrap());
    asterisk.send_to(outgoing.as_bytes(),sip_addr).await.unwrap();let(trying,_)=sip_frame_from(&asterisk).await;assert!(trying.starts_with(b"SIP/2.0 100"));let(refused,_)=sip_frame_from(&asterisk).await;assert!(refused.starts_with(b"SIP/2.0 480"));
    if native_hangup{
        native_end.send(()).unwrap();let(bye,_)=sip_frame_from(&asterisk).await;assert!(bye.starts_with(b"BYE sip:"));let via=sip_frame::header_value(&bye,"Via").unwrap();let cseq=sip_frame::header_value(&bye,"CSeq").unwrap();
        asterisk.send_to(format!("SIP/2.0 200 OK\r\nVia: {via}\r\nFrom: {from}\r\nTo: {to};tag=fixture-pbx\r\nCall-ID: {call_id}\r\nCSeq: {cseq}\r\nContent-Length: 0\r\n\r\n").as_bytes(),sip_addr).await.unwrap();
    }else{
        drop(native_end);let bye=format!("BYE sip:41000@{sip_addr} SIP/2.0\r\nVia: SIP/2.0/UDP {asterisk_addr};branch=z9hG4bKfixture-bye\r\nFrom: {to};tag=fixture-pbx\r\nTo: {from}\r\nCall-ID: {call_id}\r\nCSeq: 2 BYE\r\nContent-Length: 0\r\n\r\n");
        asterisk.send_to(bye.as_bytes(),sip_addr).await.unwrap();let(response,_)=sip_frame_from(&asterisk).await;assert!(response.starts_with(b"SIP/2.0 200"));
    }
    let ended=phase(&manager,&account.line_id,&call.id,"ended").await;assert!(!ended.media_active);assert!(ended.last_error.is_none());bounded(media_task).await.unwrap();
    assert!(temp.database.get_call_by_id(call.history_id).unwrap().unwrap().end_time.is_some());assert_eq!(link.diagnostics().active_relays,0);
    done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert!(link.trunk_local_ip().is_none());let reused=UdpSocket::bind(sip_addr).await.unwrap();assert_eq!(reused.local_addr().unwrap(),sip_addr);let reused=UdpSocket::bind(offered).await.unwrap();assert_eq!(reused.local_addr().unwrap(),offered);
}
#[tokio::test]
async fn asterisk_sip_answer_bidirectional_audio_bye_releases_only_original_sdk_call_and_both_ports(){real_trunk(false).await;}
#[tokio::test]
async fn original_native_remote_release_acks_once_sends_asterisk_bye_and_never_replays_sdk_release(){real_trunk(true).await;}
