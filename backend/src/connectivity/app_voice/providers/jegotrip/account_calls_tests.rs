//! Actions-only original account SDK -> call/control -> actual TCP/TLS/Mpath/
//! UDP audio -> API/OperatorLink -> SQLite history and original Join tests.
use super::*;
use super::super::{account_runtime::AccountManager,account_test_support::{TempDatabase,QueuedFactory,peer as sdk_peer},
    account_call_test_support::{call_config,sdp,notice,operation,reply_operation,finish_sdk},
    runtime_backend::tests::next_non_poll,tcp_mpath_audio::tests::Peer as MediaPeer,
    rpc_calls::encode_notice_result,rpc_envelope::{decode_reply,decode_request},rpc_stream::Reader,
    tcp_mpath::{self,Input as MpathInput},tls_engine::tests::Certificates,transport_packets::CallKind};
use crate::connectivity::app_voice::registry::{ProviderAvailability, ProviderRegistry, VoiceCapability};
use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use std::{io::{Read, Write}, time::Duration};
use tokio::{net::{TcpListener,UdpSocket},sync::oneshot};
use tokio_rustls::rustls;
async fn bounded<F:Future>(future:F)->F::Output{tokio::time::timeout(Duration::from_secs(8),future).await.expect("bounded original account call fixture")}
async fn ready(manager:&AccountManager,line:&str){bounded(async{loop{if manager.get(line,100).await.unwrap().incoming_call_ready{break;}tokio::task::yield_now().await;}}).await;}
async fn phase(manager:&AccountManager,line:&str,id:&str,expected:&str)->CallView{bounded(async{loop{let view=manager.call(line,id).await.unwrap();if view.phase==expected{return view;}tokio::task::yield_now().await;}}).await}
async fn first_call(manager:&AccountManager,line:&str)->CallView{bounded(async{loop{let calls=manager.calls(line).await.unwrap();if let Some(call)=calls.into_iter().next(){if call.phase=="ringing"{return call;}}tokio::task::yield_now().await;}}).await}
fn rtp(sequence:u16,fill:u8)->Vec<u8>{let mut packet=vec![0x80,0,0,0,0,0,0,160,0x12,0x34,0x56,0x78];packet[2..4].copy_from_slice(&sequence.to_be_bytes());packet.resize(173,fill);packet}
fn compressed_sdp(input:&[u8])->Vec<u8>{let mut encoder=ZlibEncoder::new(Vec::new(),Compression::default());encoder.write_all(input).unwrap();STANDARD.encode(encoder.finish().unwrap()).into_bytes()}
fn inflated_sdp(input:&[u8])->Vec<u8>{let compressed=STANDARD.decode(input).unwrap();let mut decoder=ZlibDecoder::new(compressed.as_slice());let mut plain=Vec::new();decoder.read_to_end(&mut plain).unwrap();plain}
async fn receipt(peer:&mut super::super::runtime_backend::tests::Peer,id:i32,accepted:bool){let packet=next_non_poll(peer).await;assert_eq!(packet.kind,CallKind::Reply);
    let reply=decode_reply(packet.payload.expose_to_transport()).unwrap();assert_eq!(reply.call_id,id);assert_eq!(reply.result_stream,encode_notice_result(CallNoticeKind::Invited,accepted,&[]).unwrap().expose_to_transport());}
fn original_endpoint(wire:&[u8]){let request=decode_request(wire).unwrap();assert_eq!(request.method,b"connect.Endpoint.Endpoint");let mut reader=Reader::versioned(request.arguments).unwrap();
    assert_eq!(reader.bytes().unwrap(),b"fixture-original-content");let count=reader.u32().unwrap();for _ in 0..count{reader.bytes().unwrap();}
    assert!(!reader.properties().unwrap().iter().any(|property|property.key==b"Client.ArcPort"));reader.finish().unwrap();}
async fn positive(operator_commands:bool){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let(listener,endpoint)={let listener=TcpListener::bind(("127.0.0.1",0)).await.unwrap();let endpoint=format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};",listener.local_addr().unwrap().port());(listener,endpoint)};
    let asterisk=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();let answer=sdp(asterisk.local_addr().unwrap());let native=sdp("127.0.0.1:5006".parse().unwrap());
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=Arc::new(AccountManager::with_factory(temp.database.clone(),factory.clone()));
    let account=manager.create(call_config(&sdk.root,&media.root),100).await.unwrap().account;let line=account.line_id;let revision=account.revision;
    let(go,invite)=oneshot::channel();let(done,stop)=oneshot::channel();let(answered,seen_answer)=oneshot::channel();
    let sdk_task=tokio::spawn(async move{bounded(async move{
        peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&native,b"fixture-domain").await;receipt(&mut peer,101,true).await;
        let(id,_)=operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true,None).await;
        let(id,offer)=operation(&mut peer,CallOperation::Accept).await;let local=PlainAudioOffer::trunk(&offer,true).unwrap().endpoint().unwrap();answered.send(local).unwrap();
        reply_operation(&mut peer,id,CallOperation::Accept,true,Some(endpoint.as_bytes())).await;
        let(id,_)=operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true,None).await;
        stop.await.unwrap();finish_sdk(peer).await;
    }).await});
    let down=rtp(9,0xff);let up=rtp(10,0x80);let down_peer=down.clone();let up_peer=up.clone();let server=media.server(&rustls::version::TLS13);let(sent,observed)=oneshot::channel();
    let media_task=tokio::spawn(async move{bounded(async move{
        let mut peer=MediaPeer::accept(listener,server).await;original_endpoint(peer.next_rpc().await.unwrap().unwrap().payload.expose_to_transport());peer.reply(true).await;
        let(packets,_)=tcp_mpath::encode_payload(0,0x3fff,1234,0,&down_peer).unwrap();peer.send_raw(packets[0].expose_to_transport()).await;
        let raw=peer.next_raw().await.unwrap().unwrap();let mut receiver=tcp_mpath::Receiver::default();let Some(MpathInput::Payload{channel:0,payload,..})=receiver.on_packet(&raw,Instant::now()).unwrap()else{panic!("original native audio")};
        assert_eq!(payload.expose_to_transport(),up_peer);sent.send(()).unwrap();peer.eof().await;
    }).await});
    bounded(manager.start(&line,revision,100)).await.unwrap();ready(&manager,&line).await;
    let registry=ProviderRegistry::new();registry.install(Box::new(super::super::message_provider::MessagingProvider::for_accounts(&manager))).unwrap();
    assert_eq!(registry.capability_availability(super::super::PROVIDER_ID,&line,VoiceCapability::Voice),ProviderAvailability::Available);
    assert_eq!(registry.capability_availability(super::super::PROVIDER_ID,&line,VoiceCapability::Messaging),ProviderAvailability::Unsupported);
    assert_eq!(registry.capability_availability(super::super::PROVIDER_ID,&line,VoiceCapability::Conference),ProviderAvailability::Unsupported);
    assert_eq!(registry.implemented_operations(super::super::PROVIDER_ID).as_slice(),super::super::BASIC_INBOUND_OPERATIONS);
    let link=manager.call_operator(&line).await.unwrap();let mut events=link.subscribe_events();
    assert!(!manager.get(&line,100).await.unwrap().outbound_authorized);go.send(()).unwrap();let call=first_call(&manager,&line).await;
    assert_eq!(call.peer,"fixture-caller");assert!(!call.media_active);assert!(!temp.database.get_call_by_id(call.history_id).unwrap().unwrap().answered);
    let offered=PlainAudioOffer::trunk(call.sdp.as_ref().unwrap().as_bytes(),true).unwrap().endpoint().unwrap();
    if operator_commands{link.send_command(OperatorCommand::AcceptCall{call_id:call.id.clone(),body:answer}).unwrap();phase(&manager,&line,&call.id,"connected").await;}
    else{let accepted=bounded(manager.answer_call(&line,revision,&call.id,answer,100)).await.unwrap();assert_eq!(accepted.phase,"connected");}
    assert_eq!(bounded(seen_answer).await.unwrap(),offered);let view=phase(&manager,&line,&call.id,"connected").await;assert!(view.media_active);
    let public=serde_json::to_string(&view).unwrap();for private in ["fixture-original-content","#fixture-call","fixture-domain","phone_login_token","root_der_base64"]{assert!(!public.contains(private));}
    let mut bytes=[0;4096];let(count,source)=bounded(asterisk.recv_from(&mut bytes)).await.unwrap();assert_eq!(source,offered);assert_eq!(&bytes[..count],down);
    asterisk.send_to(&up,offered).await.unwrap();bounded(observed).await.unwrap();assert!(temp.database.get_call_by_id(call.history_id).unwrap().unwrap().answered);
    if operator_commands{link.send_command(OperatorCommand::HangupCall{call_id:call.id.clone()}).unwrap();phase(&manager,&line,&call.id,"ended").await;}
    else{assert_eq!(bounded(manager.hangup_call(&line,revision,&call.id,100)).await.unwrap().phase,"ended");}
    assert!(!manager.call(&line,&call.id).await.unwrap().media_active);bounded(media_task).await.unwrap();let rebound=UdpSocket::bind(offered).await.unwrap();assert_eq!(rebound.local_addr().unwrap(),offered);
    let row=temp.database.get_call_by_id(call.history_id).unwrap().unwrap();assert_eq!(row.line_id.as_deref(),Some(line.as_str()));assert!(row.end_time.is_some());
    let event_types=temp.database.get_recent_app_events(Some(&line),20).unwrap().into_iter().map(|event|event.event_type).collect::<Vec<_>>();for kind in ["call.started","call.connected","call.ended"]{assert!(event_types.iter().any(|value|value==kind));}
    bounded(async{loop{if matches!(events.recv().await.unwrap(),OperatorEvent::Ended{call_id}if call_id==call.id){break;}}}).await;
    assert_eq!(link.diagnostics().active_relays,0);assert_eq!(link.diagnostics().rtp_to_asterisk_packets,1);assert_eq!(link.diagnostics().rtp_from_asterisk_packets,1);
    assert_eq!(manager.reserve_outbound(&line,revision).err(),Some("app_account_outbound_unsupported"));
    done.send(()).unwrap();bounded(manager.stop(&line,revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
    assert!(!temp.database.with_conn(|conn|account_runs::ownership_pending(conn,&line)).unwrap());assert_eq!(factory.build_count(),1);
}
#[tokio::test]
async fn account_api_answer_original_sdk_mpath_both_audio_directions_history_and_hangup_join(){positive(false).await;}
#[tokio::test]
async fn original_operator_trunk_commands_answer_hangup_same_call_with_real_bidirectional_audio(){positive(true).await;}
#[tokio::test]
async fn original_compressed_invitation_is_plain_for_operator_and_zlib_base64_again_for_native_accept(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);
    let account=manager.create(call_config(&sdk.root,&media.root),100).await.unwrap().account;let(go,invite)=oneshot::channel();let(wire,observed)=oneshot::channel();let(done,stop)=oneshot::channel();
    let offered=compressed_sdp(&sdp("127.0.0.1:5006".parse().unwrap()));
    let sdk_task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();
        notice(&mut peer,CallNoticeKind::Invited,101,&offered,b"fixture-domain").await;receipt(&mut peer,101,true).await;
        let(id,_)=operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true,None).await;
        let(id,answer)=operation(&mut peer,CallOperation::Accept).await;wire.send(answer).unwrap();reply_operation(&mut peer,id,CallOperation::Accept,false,None).await;
        let(id,_)=operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true,None).await;
        stop.await.unwrap();finish_sdk(peer).await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;go.send(()).unwrap();let call=first_call(&manager,&account.line_id).await;
    let public=call.sdp.as_ref().unwrap();assert!(public.starts_with("v=0\r\n"));assert!(PlainAudioOffer::trunk(public.as_bytes(),true).is_ok());
    assert_eq!(bounded(manager.answer_call(&account.line_id,account.revision,&call.id,sdp("127.0.0.1:5008".parse().unwrap()),100)).await.err(),Some("app_account_call_rejected"));
    let native=bounded(observed).await.unwrap();assert!(!native.starts_with(b"v=0"));assert_eq!(native.len()%4,0);
    let plain=inflated_sdp(&native);let parsed=PlainAudioOffer::trunk(&plain,true).unwrap();assert_ne!(parsed.endpoint().unwrap(),"127.0.0.1:5008".parse().unwrap());
    assert_eq!(phase(&manager,&account.line_id,&call.id,"ended").await.phase,"ended");
    done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn unsupported_native_crypto_is_refused_on_original_receipt_before_history_inform_or_media(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);
    let account=manager.create(call_config(&sdk.root,&media.root),100).await.unwrap().account;let(go,invite)=oneshot::channel();let(done,stop)=oneshot::channel();let(seen,checked)=oneshot::channel();
    let sdk_task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();let mut offer=sdp("127.0.0.1:5006".parse().unwrap());offer.extend_from_slice(b"a=crypto:1 fixture\r\n");
        notice(&mut peer,CallNoticeKind::Invited,101,&offer,b"fixture-domain").await;receipt(&mut peer,101,false).await;seen.send(()).unwrap();stop.await.unwrap();finish_sdk(peer).await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;go.send(()).unwrap();bounded(checked).await.unwrap();
    assert!(manager.calls(&account.line_id).await.unwrap().is_empty());assert!(temp.database.get_call_history_for_line(&account.line_id,10,0).unwrap().is_empty());
    done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn cancelled_api_answer_waiter_never_activates_late_positive_endpoint_or_marks_answered(){
    let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");
    let listener=TcpListener::bind(("127.0.0.1",0)).await.unwrap();let endpoint=format!("#fixture-endpoint:tcp -h 127.0.0.1 -p {};",listener.local_addr().unwrap().port());
    let(mut peer,_,dropped)=sdk_peer(&sdk,&factory);let manager=AccountManager::with_factory(temp.database.clone(),factory);
    let account=manager.create(call_config(&sdk.root,&media.root),100).await.unwrap().account;let(go,invite)=oneshot::channel();let(submitted,seen)=oneshot::channel();let(late,released)=oneshot::channel();let(done,stop)=oneshot::channel();
    let sdk_task=tokio::spawn(async move{bounded(async move{peer.authenticate().await;peer.swap(Some(4),true).await;invite.await.unwrap();notice(&mut peer,CallNoticeKind::Invited,101,&sdp("127.0.0.1:5006".parse().unwrap()),b"fixture-domain").await;receipt(&mut peer,101,true).await;
        let(id,_)=operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true,None).await;
        let(id,_)=operation(&mut peer,CallOperation::Accept).await;submitted.send(()).unwrap();released.await.unwrap();reply_operation(&mut peer,id,CallOperation::Accept,true,Some(endpoint.as_bytes())).await;
        let(id,_)=operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true,None).await;stop.await.unwrap();finish_sdk(peer).await;}).await});
    bounded(manager.start(&account.line_id,account.revision,100)).await.unwrap();ready(&manager,&account.line_id).await;go.send(()).unwrap();let call=first_call(&manager,&account.line_id).await;
    let asterisk=UdpSocket::bind(("127.0.0.1",0)).await.unwrap();{
        let wait=manager.answer_call(&account.line_id,account.revision,&call.id,sdp(asterisk.local_addr().unwrap()),100);tokio::pin!(wait);
        tokio::select!{value=wait.as_mut()=>panic!("no original acceptance yet: {value:?}"),_=seen=>{}}
    }
    late.send(()).unwrap();let ended=phase(&manager,&account.line_id,&call.id,"ended").await;assert!(!ended.media_active);
    assert!(!temp.database.get_call_by_id(call.history_id).unwrap().unwrap().answered);assert!(tokio::time::timeout(Duration::from_millis(100),listener.accept()).await.is_err());
    done.send(()).unwrap();bounded(manager.stop(&account.line_id,account.revision,100)).await.unwrap();bounded(sdk_task).await.unwrap();assert!(dropped.load(Ordering::SeqCst));
}
#[test]
fn call_media_import_is_explicit_pure_secret_preserving_and_rejects_cellular_cost_flags(){
    let sdk=Certificates::generate("DNS:fixture.invalid");let media=Certificates::generate("IP:127.0.0.1");let mut config=call_config(&sdk.root,&media.root);
    assert!(config.validate(100).is_ok());let bytes=config.encode_private().unwrap();let loaded=super::super::account_config::AccountConfig::decode_private(&bytes).unwrap();assert_eq!(&*bytes,&*loaded.encode_private().unwrap());
    assert!(!format!("{config:?}").contains("127.0.0.1"));config.call_media.as_mut().unwrap().allow_plain_media=false;assert!(config.validate(100).is_err());config.call_media.as_mut().unwrap().allow_plain_media=true;
    let mut trunk=crate::platform::config::TrunkProfileConfig::default();trunk.vowifi_only=true;config.call_media.as_mut().unwrap().trunk=Some(trunk);assert!(config.validate(100).is_err());
    config.call_media.as_mut().unwrap().trunk.as_mut().unwrap().vowifi_only=false;config.call_media.as_mut().unwrap().trunk.as_mut().unwrap().allow_home_cellular_calls=true;assert!(config.validate(100).is_err());
    config.call_media.as_mut().unwrap().trunk=None;config.profile.call_control=None;assert!(config.validate(100).is_err());
}
#[path="account_calls_trunk_tests.rs"]
mod trunk;
#[path="account_calls_lifecycle_tests.rs"]
mod lifecycle;
#[path="account_calls_admission_tests.rs"]
mod admission;
#[path="account_calls_retirement_tests.rs"]
mod retirement;
