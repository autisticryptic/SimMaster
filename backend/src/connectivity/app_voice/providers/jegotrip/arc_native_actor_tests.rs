//! Actions-only original Router/TLS/Channel/Common -> ARC actor regressions.
#[path = "startup_diagnostics_tests.rs"]
mod startup_diagnostics;

use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{common_runtime::tests::CommonPeer,
    netstream_runtime::tests::{owner,policy as router_policy,timeouts,reference},object_reference::ReferenceTarget,
    tls_engine::tests::Certificates,rpc_envelope::{decode_request,encode_reply,encode_request,decode_reply},rpc_stream::{Writer,Reader},
    transport_packets::CallKind,controller::Callback,session::{STATUS_REGISTER,GatewayRegistration}};
use tokio::net::TcpListener;
use tokio_rustls::rustls::version::TLS13;

fn rpc_policy()->RuntimePolicy{RuntimePolicy{io_timeout:Duration::from_secs(2),rpc_timeout:Duration::from_secs(3)}}
fn login()->ArcLoginInputs<'static>{ArcLoginInputs{typed_identity:b"username:fixture-user",domain_name:b"fixture.invalid",selected_session_id:b"fixture-selected-session",
    phone_login_token:b"fixture-phone-token",application_key:b"fixture-app-key",domain_id:b"fixture-domain-id",application_id:b"fixture-app-id",device_info_cc:b"fixture-cc",client_properties:&[]}}
fn login_result(accepted:bool)->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();w.bytes(b"fixture-user").unwrap();w.bytes(b"fixture-auth").unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
fn reply(id:i32,result:&[u8])->Vec<u8>{let envelope=encode_reply(id,0,result).unwrap();let mut packet=vec![1];packet.extend_from_slice(envelope.expose_to_transport());packet}
async fn connected(p:&RouterPolicy,original:AttemptOwner,end:Instant)->ConnectedCommonRuntime{
    let prepared=prepare_common(original,rpc_policy(),end).unwrap();let scope=prepared.connection_scope();let r=reference(false);let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
    let connected=prepared.connect(p,&[],r.identity(),&endpoints[0],0,timeouts(),None).await.unwrap();assert!(connected.connection_scope().matches(&scope));connected
}
async fn authenticated(handle:&RuntimeHandle,events:&mut mpsc::Receiver<RuntimeEvent>,original:&AttemptOwner,scope:&RpcConnectionScope){
    let receipt=handle.login(original,scope,login(),&[RpcProperty{key:b"id",value:b"AccountEntry"}]).await.unwrap();assert!(receipt.submitted_on(original,scope));assert_eq!(receipt.call_id,Some(1));
    let event=tokio::time::timeout(Duration::from_secs(3),events.recv()).await.unwrap().unwrap();
    assert!(matches!(&event.events_for(original,scope).unwrap()[0],OfflineEvent::Callback(c) if c.callback==(Callback::SdkLogin{success:true})));
}

#[test]
fn common_prepare_claims_only_original_pre_network_scope_and_drop_retires_it(){
    let prepared=prepare_common(owner(),rpc_policy(),Instant::now()+Duration::from_secs(5)).unwrap();let scope=prepared.connection_scope();
    assert!(scope.is_open());assert!(prepared.backend.as_ref().unwrap().connection_scope().matches(&scope));drop(prepared);assert!(!scope.is_open());
}
#[test]
fn common_transport_slot_cannot_be_reclaimed_or_replaced_by_a_tls_engine(){
    let cert=Certificates::generate("DNS:fixture.invalid");let mut backend=OfflineBackend::new(owner());backend.claim_common_transport().unwrap();assert!(backend.claim_common_transport().is_err());
    assert!(backend.take_tls_engine(PacketRoute::DirectTlsCarrier,TlsParameters{authority:"fixture.invalid",root_der_base64:&cert.root,tls_only:true,enable_sni:true,handshake_timeout:Duration::from_secs(1),partial_timeout:Duration::from_secs(1)},Instant::now()).is_err());
    let mut backend=OfflineBackend::new(owner());backend.take_packet_transport(PacketRoute::DirectTlsCarrier).unwrap();assert!(backend.claim_common_transport().is_err());
}
#[test]
fn native_attempt_budget_rejects_unbounded_or_already_expired_setup(){
    let now=Instant::now();assert!(prepare_common(owner(),rpc_policy(),now).is_err());assert!(prepare_common(owner(),rpc_policy(),now+Duration::from_secs(121)).is_err());
    assert!(prepare_common(owner(),RuntimePolicy{io_timeout:Duration::ZERO,rpc_timeout:Duration::from_secs(2)},now+Duration::from_secs(5)).is_err());
}
#[tokio::test]
async fn unsupported_logical_endpoint_or_selection_fails_before_router_tcp_io(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());
    for (text,index,compression) in [(b"AccountEntry:sarc -h not-a-logical-host.invalid -p 98;".as_slice(),0,None),
        (b"AccountEntry:sarc -h 42.7 -p 98;".as_slice(),1,None),(b"AccountEntry:sarc -h 42.7 -p 98;".as_slice(),0,Some(0))]{
        let prepared=prepare_common(owner(),rpc_policy(),Instant::now()+Duration::from_secs(4)).unwrap();let scope=prepared.connection_scope();
        let r=super::super::super::object_reference::parse_object_reference(text).unwrap();let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
        assert_eq!(prepared.connect(&p,&[],r.identity(),&endpoints[0],index,timeouts(),compression).await.err(),Some(RuntimeError::InvalidConfiguration));assert!(!scope.is_open());
    }
    assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());
}
#[tokio::test]
async fn native_actor_login_uses_real_common_packets_and_logout_joins_original_socket(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;
        let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.method,b"loginSession.AccountEntry.Account");assert_eq!(request.call_id,1);
        assert_eq!(request.metadata.iter().find(|p|p.key==b"pwd").unwrap().value,b"fixture-phone-token");assert!(!request.metadata.iter().any(|p|p.key==b"token"));
        let mut reader=Reader::versioned(request.arguments).unwrap();assert_eq!(reader.bytes().unwrap(),b"[username:fixture-user@fixture.invalid]");assert_eq!(reader.bytes().unwrap(),b"fixture-selected-session");
        peer.send(&[reply(request.call_id,&login_result(true))]).await;
        let packet=peer.next().await.unwrap();let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.method,b"logoutSession.AccountEntry.Account");assert_eq!(request.call_id,2);
        assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");assert!(!request.metadata.iter().any(|p|p.key==b"pwd"));peer.send(&[reply(request.call_id,b"\0\x01\0\0\0\0")]).await;peer.no_more_common().await;
    }).await.unwrap()};
    let client=async move{let original=owner();let mut connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;
        // Tighten only the fixture's setup bound. Logout occurs AFTER this
        //bound, so successful auth must permanently retire it, not revive it.
        connection.attempt_end=Instant::now()+Duration::from_secs(1);let scope=connection.connection_scope();let(mut handle,mut events)=connection.attach().unwrap();
        authenticated(&handle,&mut events,&original,&scope).await;tokio::time::sleep(Duration::from_millis(1100)).await;
        let receipt=handle.logout(&original,&scope,&[]).await.unwrap();assert!(receipt.io_released());assert_eq!(receipt.reason,RuntimeError::LoggedOut);assert!(!scope.is_open());};tokio::join!(server,client);
}
#[tokio::test]
async fn idle_native_actor_poll_does_not_cancel_common_receive_or_invent_authentication(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{CommonPeer::open(listener,config).await.no_more_common().await;}).await.unwrap()};
    let client=async move{let original=owner();let connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();let(mut handle,mut events)=connection.attach().unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;assert!(scope.is_open());assert!(matches!(events.try_recv(),Err(mpsc::error::TryRecvError::Empty)));
        assert!(handle.close().await.io_released());assert!(!scope.is_open());};tokio::join!(server,client);
}
#[tokio::test]
async fn negative_native_login_retains_only_original_failure_not_a_positive_callback(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{let mut peer=CommonPeer::open(listener,config).await;let packet=peer.next().await.unwrap();let request=decode_request(&packet[1..]).unwrap();peer.send(&[reply(request.call_id,&login_result(false))]).await;peer.no_more_common().await;}).await.unwrap()};
    let client=async move{let original=owner();let connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();let(mut handle,mut events)=connection.attach().unwrap();
        handle.login(&original,&scope,login(),&[]).await.unwrap();let event=events.recv().await.unwrap();assert_eq!(event.terminal_failure(&original,&scope),Some(super::super::super::session::BackendFailure::AuthenticationRejected));
        assert!(handle.close().await.io_released());assert!(!scope.is_open());};tokio::join!(server,client);
}
#[tokio::test]
async fn malformed_later_rpc_in_common_group_suppresses_earlier_positive_login(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{let mut peer=CommonPeer::open(listener,config).await;let packet=peer.next().await.unwrap();let request=decode_request(&packet[1..]).unwrap();peer.send(&[reply(request.call_id,&login_result(true)),vec![1,0]]).await;peer.no_more_common().await;}).await.unwrap()};
    let client=async move{let original=owner();let connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();let(mut handle,mut events)=connection.attach().unwrap();
        handle.login(&original,&scope,login(),&[]).await.unwrap();assert!(events.recv().await.is_none());let closed=handle.close().await;assert!(closed.io_released());assert_eq!(closed.reason,RuntimeError::Protocol);};tokio::join!(server,client);
}
#[tokio::test]
async fn foreign_commands_cannot_consume_native_rpc_ids_or_send_on_original_io(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{let mut peer=CommonPeer::open(listener,config).await;let packet=peer.next().await.unwrap();let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.call_id,1);peer.send(&[reply(1,&login_result(true))]).await;peer.no_more_common().await;}).await.unwrap()};
    let client=async move{let original=owner();let connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();let(mut handle,mut events)=connection.attach().unwrap();
        assert_eq!(handle.login(&owner(),&scope,login(),&[]).await.err(),Some(RuntimeError::Scope));assert_eq!(handle.bootstrap(&original,&RpcConnectionScope::fresh(),b"fixture-app-key",&[]).await.err(),Some(RuntimeError::Scope));assert!(scope.is_open());
        authenticated(&handle,&mut events,&original,&scope).await;assert!(handle.close().await.io_released());};tokio::join!(server,client);
}
#[tokio::test]
async fn dropping_connected_unattached_native_owner_closes_tcp_without_spawning_actor(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{CommonPeer::open(listener,config).await.no_more_common().await;}).await.unwrap()};
    let client=async move{let connection=connected(&p,owner(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();drop(connection);assert!(!scope.is_open());};tokio::join!(server,client);
}
#[tokio::test]
async fn native_consumer_drop_retires_original_actor_and_join_releases_socket(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{CommonPeer::open(listener,config).await.no_more_common().await;}).await.unwrap()};
    let client=async move{let connection=connected(&p,owner(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();let(mut handle,events)=connection.attach().unwrap();drop(events);
        tokio::time::timeout(Duration::from_secs(2),scope.closed()).await.unwrap();let close=handle.close().await;assert!(close.io_released());assert_eq!(close.reason,RuntimeError::Closed);};tokio::join!(server,client);
}
#[tokio::test]
async fn native_rpc_queue_delay_is_not_reset_when_actor_dequeues_an_expired_command(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{CommonPeer::open(listener,config).await.no_more_common().await;}).await.unwrap()};
    let client=async move{let connection=connected(&p,owner(),Instant::now()+Duration::from_secs(6)).await;let(mut handle,_events)=connection.attach().unwrap();let(completion,result)=oneshot::channel();
        assert!(handle.commands.try_send(Command{operation:Operation::Bootstrap(Zeroizing::new(b"fixture-key".to_vec()),Vec::new()),completion,admitted:Instant::now()-Duration::from_secs(5)}).is_ok());
        assert_eq!(result.await.unwrap().err(),Some(RuntimeError::Timeout));assert!(handle.close().await.io_released());};tokio::join!(server,client);
}
#[tokio::test]
async fn real_native_batch_hosting_and_direct_message_keep_original_nested_ticket(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(14),async move{let mut peer=CommonPeer::open(listener,config).await;
        let packet=peer.next().await.unwrap();let request=decode_request(&packet[1..]).unwrap();peer.send(&[reply(request.call_id,&login_result(true))]).await;
        let packet=peer.next().await.unwrap();let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.method,b"swapCalls.AccountEntry.Account");let mut reader=Reader::versioned(request.arguments).unwrap();reader.bytes().unwrap();reader.bytes().unwrap();
        let payload=super::super::super::rpc_batch::read_payload(&mut reader).unwrap();assert_eq!(payload.requests.len(),1);let sequence=*payload.requests.keys().next().unwrap();assert_eq!(payload.requests[&sequence].metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#fixture-gateway");
        let mut result=Writer::new();result.boolean(true).unwrap();for word in [9,1,1,7,1,1]{result.u32(word).unwrap();}result.u32(0).unwrap();result.u32(1).unwrap();result.i32(sequence).unwrap();result.u32(0).unwrap();result.bytes(b"\0\x01\0\0\0\0").unwrap();result.properties(&[]).unwrap();result.bytes(b"").unwrap();
        peer.send(&[reply(request.call_id,result.finish().expose_to_transport())]).await;
        let mut args=Writer::versioned().unwrap();args.bytes(b"fixture.kind").unwrap();args.properties(&[]).unwrap();args.bytes(&[0xff,0,3]).unwrap();
        let envelope=encode_request(1,&[RpcProperty{key:b"id",value:b"#fixture-client"},RpcProperty{key:b"domain",value:b"fixture-domain"},RpcProperty{key:b"__magic",value:b"fixture-unique"}],b"recvOnlineMessage.MessageReceiver.Client",args.finish().expose_to_transport()).unwrap();let mut incoming=vec![0];incoming.extend_from_slice(envelope.expose_to_transport());peer.send(&[incoming]).await;
        let packet=peer.next().await.unwrap();assert_eq!(packet[0],1);let result=decode_reply(&packet[1..]).unwrap();assert_eq!(result.call_id,1);assert_eq!(result.result_stream,b"\0\x01\0\0\0\0");peer.no_more_common().await;
    }).await.unwrap()};
    let client=async move{let original=owner();let connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();let(mut handle,mut events)=connection.attach().unwrap();authenticated(&handle,&mut events,&original,&scope).await;
        handle.configure(&original,&scope,7,Some(IncomingBinding{object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:b"fixture.kind"})).await.unwrap();
        let gateway=GatewayRegistration{authname_prefix:"fixture-prefix",channel_id:"fixture-channel",delete_int_code:"fixture-code",sip_number:"fixture-number",sip_password:"fixture-password"};
        handle.hosting(&original,&scope,"fixture-uri",STATUS_REGISTER,gateway,&[RpcProperty{key:b"id",value:b"#fixture-gateway"},RpcProperty{key:b"domain",value:b"fixture-domain"}],&[]).await.unwrap();
        let(mut hosted,mut ticket)=(false,None);while !hosted||ticket.is_none(){let event=events.recv().await.unwrap();for event in event.into_events(&original,&scope).unwrap(){match event{
            OfflineEvent::Callback(c)=>{assert_eq!(c.callback,Callback::HostingAck{success:true});hosted=true;},OfflineEvent::DirectMessage(m)=>{assert_eq!(m.message().body,[0xff,0,3]);ticket=Some(m.into_parts().2);},_=>panic!("unexpected event")
        }}}
        handle.complete_direct(&original,&scope,&ticket.unwrap(),true).await.unwrap();assert!(handle.close().await.io_released());};tokio::join!(server,client);
}

#[tokio::test]
async fn explicit_plain_arc_uses_outer_endpoint_tls_without_guessing_seeded_record_security(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{let mut peer=CommonPeer::open_mode(listener,config,super::super::super::netstream_records::RecordSecurity::Plain).await;
        let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.call_id,1);peer.send(&[reply(1,&login_result(true))]).await;peer.no_more_common().await;}).await.unwrap()};
    let client=async move{let original=owner();let prepared=prepare_common(original.clone(),rpc_policy(),Instant::now()+Duration::from_secs(6)).unwrap();let scope=prepared.connection_scope();
        let r=super::super::super::object_reference::parse_object_reference(b"AccountEntry:arc -h 42.7 -p 98;").unwrap();let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("endpoint")};
        let connected=prepared.connect(&p,&[],r.identity(),&endpoints[0],0,timeouts(),None).await.unwrap();let(mut handle,mut events)=connected.attach().unwrap();authenticated(&handle,&mut events,&original,&scope).await;assert!(handle.close().await.io_released());};tokio::join!(server,client);
}
#[tokio::test]
async fn continuous_slow_control_replies_cannot_starve_a_really_queued_native_login(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);let(tx,rx)=oneshot::channel();
    let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;peer.send(&[vec![2],vec![2],vec![2],vec![2]]).await;tx.send(()).unwrap();
        let(mut pongs,mut batches,mut logged)=(0usize,1usize,false);
        while let Some(packet)=peer.next().await{
            if packet==[14]{pongs+=1;if !logged&&pongs%4==1&&batches<16{peer.send(&[vec![2],vec![2],vec![2],vec![2]]).await;batches+=1;}}
            else{assert!(!logged);assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();assert_eq!(request.call_id,1);peer.send(&[reply(1,&login_result(true))]).await;logged=true;}
        }assert!(logged);
    }).await.unwrap()};
    let client=async move{let original=owner();let mut connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();
        let pauses=super::super::super::common_runtime::tests::delay_common_write(connection.common.as_mut().unwrap(),Duration::from_millis(50));
        let(mut handle,mut events)=connection.attach().unwrap();rx.await.unwrap();
        tokio::time::timeout(Duration::from_secs(2),authenticated(&handle,&mut events,&original,&scope)).await.unwrap();assert!(pauses.load(std::sync::atomic::Ordering::Acquire)>0);
        assert!(handle.close().await.io_released());};tokio::join!(server,client);
}
#[tokio::test]
async fn native_handle_drop_revokes_surviving_commands_and_retained_join_observes_io_release(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{CommonPeer::open(listener,config).await.no_more_common().await;}).await.unwrap()};
    let client=async move{let original=owner();let connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();let(mut handle,_events)=connection.attach().unwrap();let commands=handle.command_handle();
        // Test owns the very same Join while dropping the public handle; this
        //does not fabricate a RuntimeCloseReceipt or release an account lease.
        let mut task=handle.task.take().unwrap();drop(handle);assert!(!scope.is_open());assert_eq!(commands.bootstrap(&original,&scope,b"fixture-key",&[]).await.err(),Some(RuntimeError::Scope));
        match tokio::time::timeout(Duration::from_secs(2),&mut task).await{Ok(result)=>assert_eq!(result.unwrap(),RuntimeError::Closed),Err(_)=>{task.abort();let _=task.await;panic!("native task did not retire");}}
    };tokio::join!(server,client);
}
#[tokio::test]
async fn cancelling_actual_native_submission_stops_blocked_write_and_preserves_owning_join(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=router_policy(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(10),async move{CommonPeer::open(listener,config).await.no_more_common().await;}).await.unwrap()};
    let client=async move{let original=owner();let mut connection=connected(&p,original.clone(),Instant::now()+Duration::from_secs(6)).await;let scope=connection.connection_scope();
        let pauses=super::super::super::common_runtime::tests::delay_common_write(connection.common.as_mut().unwrap(),Duration::from_millis(300));let(mut handle,_events)=connection.attach().unwrap();let commands=handle.command_handle();
        assert!(tokio::time::timeout(Duration::from_millis(35),commands.bootstrap(&original,&scope,b"fixture-key",&[])).await.is_err());
        tokio::time::timeout(Duration::from_secs(2),scope.closed()).await.unwrap();assert!(pauses.load(std::sync::atomic::Ordering::Acquire)>0);
        let closed=handle.close().await;assert!(closed.io_released());assert_eq!(closed.reason,RuntimeError::Cancelled);
    };tokio::join!(server,client);
}
