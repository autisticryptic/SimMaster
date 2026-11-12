//! Actions-only native profile -> shared HostingBackend/actor integration.
use super::*;
use super::super::{common_runtime::tests::CommonPeer,netstream_runtime::tests::{owner,policy},
    tls_engine::tests::Certificates,rpc_envelope::{encode_reply,encode_request,decode_reply,decode_request},rpc_stream::{Writer,Reader},
    runtime_connector::{ConnectFuture,ConnectError}};
use std::sync::atomic::{AtomicUsize,Ordering};
use tokio::{net::TcpListener,io::AsyncReadExt};
use tokio_rustls::rustls::version::TLS13;
const ENTRY:&str="AccountEntry:sarc -h 42.7 -p 98;";
#[path = "runtime_common_offline_tests.rs"]
mod offline;
const BOOTSTRAP_SDK:&str="BootEntry:sarc -h 42.7 -p 98;";
const DESTINATION:&str="DestEntry:sarc -h 42.7 -p 98;";
fn bootstrap_inputs()->RuntimeProfileInputs<'static>{RuntimeProfileInputs{domain_name:"",domain_id:"",application_id:"",device_info_cc:"",gateway_uri:"",..inputs()}}
fn bootstrap_common(root:&str,port:u16)->RuntimeCommonInputs<'static>{RuntimeCommonInputs{account_reference:"",..common(root,port)}}
fn bootstrap_route()->RuntimeCommonBootstrapInputs<'static>{RuntimeCommonBootstrapInputs{routing:&[],client_config_decode_key:None,handoff_timeout:Duration::from_secs(10)}}
fn bootstrap_result()->Vec<u8>{bootstrap_result_alive(b"")}
fn bootstrap_result_alive(alive:&[u8])->Vec<u8>{
    let properties=[RpcProperty{key:b"DomainName",value:b"account-domain.invalid"},RpcProperty{key:b"DomainId",value:b"fixture-domain"},
        RpcProperty{key:b"AppId",value:b"fixture-app"},RpcProperty{key:b"CC",value:b"fixture-cc"}];
    let mut w=Writer::new();w.boolean(true).unwrap();w.properties(&properties).unwrap();w.bytes(DESTINATION.as_bytes()).unwrap();
    w.bytes(alive).unwrap();w.bytes(b"").unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()
}
fn gateway()->GatewayRegistration<'static>{GatewayRegistration{authname_prefix:"fixture-prefix",channel_id:"fixture-channel",
    delete_int_code:"1",sip_number:"fixture-number",sip_password:"fixture-SipPwd"}}
/// Hosting is ONE nested updateStatus request inside swapCalls. Returns the
/// `current_user_uri` argument and the `SipFlag` value actually sent.
async fn hosting_call(peer:&mut CommonPeer)->(Vec<u8>,Vec<u8>){
    loop{
        let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let call=decode_request(&packet[1..]).unwrap();
        assert_eq!(call.method,super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        let mut args=Reader::versioned(call.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();
        let payload=super::super::rpc_batch::read_payload(&mut args).unwrap();
        if payload.requests.is_empty(){peer.send(&[reply(call.call_id,&super::tests::swap_result(None,true,payload.fields))]).await;continue;}
        // An un-acknowledged hosting request stays retained and is retransmitted
        // with the next one, so a batch may carry the previous registration plus
        // the new request. Every nested request must still target SipGateway; the
        // newest sequence is the one this call is waiting for.
        let mut chosen=None;
        for(sequence,nested) in payload.requests.iter(){
            assert_eq!(nested.metadata.iter().find(|p|p.key==b"id").unwrap().value,
                super::super::rpc_arguments::DEFAULT_GATEWAY_OBJECT.as_bytes());
            let mut fields=Reader::versioned(nested.arguments).unwrap();let uri=fields.bytes().unwrap().to_vec();
            let flag=fields.properties().unwrap().iter().find(|p|p.key==b"SipFlag").unwrap().value.to_vec();
            chosen=Some((*sequence,uri,flag));
        }
        let(sequence,uri,flag)=chosen.unwrap();
        peer.send(&[reply(call.call_id,&super::tests::swap_result(Some(sequence),true,payload.fields))]).await;
        return (uri,flag);
    }
}
#[tokio::test]
async fn matched_common_bootstrap_alive_state_is_preserved_without_inventing_registration_or_receivers(){
    for (alive,label) in [(b"".as_slice(),"absent"),(super::super::runtime_provision::AccountAliveState::DISABLED_REFERENCE,"disabled"),
        (b"AccountAlive:stcp -h private.fixture.invalid -p 8888;".as_slice(),"unsupported")] {
        let cert=Certificates::generate("IP:127.0.0.1");
        let front=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=front.local_addr().unwrap().port();
        let source_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let source_port=source_listener.local_addr().unwrap().port();
        let destination_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let destination_port=destination_listener.local_addr().unwrap().port();
        let server=cert.server_config(&TLS13);let source_server=server.clone();
        let relay=tokio::spawn(async move {
            let mut tasks=Vec::new();
            for target in [source_port,destination_port] {
                let(mut inbound,_)=front.accept().await.unwrap();let mut outbound=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,target)).await.unwrap();
                tasks.push(tokio::spawn(async move {let _=tokio::io::copy_bidirectional(&mut inbound,&mut outbound).await;}));
            }
            for task in tasks {task.await.unwrap();}
        });
        let source_task=tokio::spawn(async move {
            let mut source=CommonPeer::open(source_listener,source_server).await;let packet=source.next().await.unwrap();
            let call=decode_request(&packet[1..]).unwrap();assert_eq!(call.method,b"getAppConfigs.AccountEntry.Account");
            source.send(&[reply(call.call_id,&bootstrap_result_alive(alive))]).await;while source.next().await.is_some(){}
        });
        let destination_task=tokio::spawn(async move {
            let mut peer=CommonPeer::open(destination_listener,server).await;let packet=peer.next().await.unwrap();
            let call=decode_request(&packet[1..]).unwrap();assert_eq!(call.method,b"loginSession.AccountEntry.Account");
            peer.send(&[reply(call.call_id,&login_result())]).await;while peer.next().await.is_some(){}
        });
        let p=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,port),bootstrap_route()).unwrap();
        let connector=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,mut events)=NativeHostingBackend::new(p,connector.clone());let original=owner();
        backend.login(original.clone(),SdkLoginRequest{server_address:BOOTSTRAP_SDK,..request(&cert.root)}).await.unwrap();
        assert_eq!(backend.account_alive_observation(&original),Some(label));
        let event=tokio::time::timeout(Duration::from_secs(4),events.recv()).await.unwrap().unwrap();
        assert!(matches!(event.payload,BackendEventPayload::Callback(OwnedRpcCallback{callback:Callback::SdkLogin{success:true},..})));
        assert!(events.try_recv().is_err(),"observing Alive does not create a HostingAck");
        assert_eq!(connector.calls.load(Ordering::SeqCst),0);
        assert_eq!(backend.account_alive_observation(&owner()),None);
        backend.retire(&original);backend.logout(original).await.unwrap();
        tokio::time::timeout(Duration::from_secs(4),async {source_task.await.unwrap();destination_task.await.unwrap();relay.await.unwrap();}).await.unwrap();
    }
}
#[test]
fn common_bootstrap_profile_never_preknows_identity_destination_or_offsets(){
    let cert=Certificates::generate("IP:127.0.0.1");
    let p=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,12345),bootstrap_route()).unwrap();
    let route=p.common.as_ref().unwrap();assert!(route.entry.is_none());assert!(route.reference.is_none());assert!(route.access.is_empty());
    for fields in [&p.login_routing,&p.poll_routing,&p.logout_routing]{assert!(!fields.iter().any(|p|p.key==b"id"));}
    assert!(p.bootstrap.is_none());assert!(p.common_bootstrap.is_some());assert!(p.domain_name.is_empty()&&p.domain_id.is_empty()&&p.application_id.is_empty());
    // The real current_user_uri is derived from the matched session identity,
    // and hosting defaults to the original SipGateway object.
    assert!(p.gateway_uri.is_none());assert_eq!(p.gateway_metadata.len(),1);
    assert_eq!(p.gateway_metadata[0].key,b"id");assert_eq!(p.gateway_metadata[0].value,b"#SipGateway");
    // An already-known entry, identity, offset or destination field contradicts
    // "derived from the matched reply" and is refused before any network work.
    assert!(RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),common(&cert.root,12345),bootstrap_route()).is_err());
    assert!(RuntimeProfile::supplied_common_bootstrap(inputs(),bootstrap_common(&cert.root,12345),bootstrap_route()).is_err());
    assert!(RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,12345),
        RuntimeCommonBootstrapInputs{routing:&[RpcProperty{key:b"id",value:b"DestEntry"}],..bootstrap_route()}).is_err());
    assert!(RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),RuntimeCommonInputs{endpoint_index:1,..bootstrap_common(&cert.root,12345)},bootstrap_route()).is_err());
    assert!(RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),RuntimeCommonInputs{resolved_host_index:1,..bootstrap_common(&cert.root,12345)},bootstrap_route()).is_err());
    // A supplied URI stays supported; only an empty one requests derivation.
    let explicit=RuntimeProfile::supplied_common_bootstrap(RuntimeProfileInputs{gateway_uri:"fixture-supplied-uri",..bootstrap_inputs()},bootstrap_common(&cert.root,12345),bootstrap_route()).unwrap();
    assert_eq!(explicit.gateway_uri.as_ref().map(|uri|uri.as_str()),Some("fixture-supplied-uri"));
    // Explicitness of the SipGateway object is preserved, never duplicated.
    let custom=RuntimeProfile::supplied_common_bootstrap(RuntimeProfileInputs{gateway_metadata:&[RpcProperty{key:b"id",value:b"#CustomGateway"}],..bootstrap_inputs()},bootstrap_common(&cert.root,12345),bootstrap_route()).unwrap();
    assert_eq!(custom.gateway_metadata.len(),1);assert_eq!(custom.gateway_metadata[0].value,b"#CustomGateway");
    for handoff in [Duration::ZERO,Duration::from_secs(121)]{assert!(RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,12345),
        RuntimeCommonBootstrapInputs{handoff_timeout:handoff,..bootstrap_route()}).is_err());}
    let text=format!("{p:?}");for value in ["BootEntry","DestEntry","12345","fixture"]{assert!(!text.contains(value));}
}
#[test]
fn common_bootstrap_login_capture_keeps_the_sdk_reference_as_untrusted_data_only(){
    let cert=Certificates::generate("IP:127.0.0.1");
    let p=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,12345),bootstrap_route()).unwrap();
    let captured=LoginCapture::capture(SdkLoginRequest{server_address:BOOTSTRAP_SDK,..request(&cert.root)},&p).unwrap();
    assert_eq!(captured.reference.as_str(),BOOTSTRAP_SDK);assert_eq!(captured.password.as_str(),"fixture-phone-token");
    assert!(LoginCapture::capture(SdkLoginRequest{tls_only:false,..request(&cert.root)},&p).is_err());
    assert!(LoginCapture::capture(SdkLoginRequest{account_identity:"fixture:user",..request(&cert.root)},&p).is_err());
}
#[tokio::test]
async fn common_bootstrap_dials_only_the_returned_entry_after_releasing_its_own_source_actor(){
    let cert=Certificates::generate("IP:127.0.0.1");
    let front=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let bootstrap_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let destination_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let port=front.local_addr().unwrap().port();
    let bootstrap_port=bootstrap_listener.local_addr().unwrap().port();let destination_port=destination_listener.local_addr().unwrap().port();
    let config=cert.server_config(&TLS13);let bootstrap_config=config.clone();
    // Transparent byte relay: each dial is a REAL separate Router/Common
    // connection, but both can share the single supplied policy port.
    let relay=async move{for target in [bootstrap_port,destination_port]{
        let(mut inbound,_)=front.accept().await.unwrap();
        let mut outbound=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,target)).await.unwrap();
        tokio::spawn(async move{let _=tokio::io::copy_bidirectional(&mut inbound,&mut outbound).await;});
    }};
    let server=async move{
        let bootstrap=async move{let mut peer=CommonPeer::open(bootstrap_listener,bootstrap_config).await;
            let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let call=decode_request(&packet[1..]).unwrap();
            assert_eq!(call.method,b"getAppConfigs.AccountEntry.Account");assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"BootEntry");
            peer.send(&[reply(call.call_id,&bootstrap_result())]).await;while peer.next().await.is_some(){}
        };
        let destination=async move{let mut peer=CommonPeer::open(destination_listener,config).await;
            let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let call=decode_request(&packet[1..]).unwrap();
            assert_eq!(call.method,b"loginSession.AccountEntry.Account");
            assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"DestEntry");
            assert_eq!(call.metadata.iter().find(|p|p.key==b"pwd").unwrap().value,b"fixture-phone-token");
            let mut reader=Reader::versioned(call.arguments).unwrap();assert_eq!(reader.bytes().unwrap(),b"[username:fixture-user@account-domain.invalid]");
            assert_eq!(reader.bytes().unwrap(),b"fixture-selected-session");
            let fields=reader.properties().unwrap();assert_eq!(fields.iter().find(|p|p.key==b"__domain").unwrap().value,b"fixture-domain");
            assert_eq!(fields.iter().find(|p|p.key==b"__app").unwrap().value,b"fixture-app");
            assert_eq!(fields.iter().find(|p|p.key==b"DeviceInfo.CC").unwrap().value,b"fixture-cc");
            assert_eq!(fields.iter().find(|p|p.key==b"__appKey").unwrap().value,b"fixture-app-key");
            peer.send(&[reply(call.call_id,&login_result())]).await;
            // Auto-assembled hosting: the URI is the matched session identity and
            // the batch targets the original default SipGateway object.
            let(uri,flag)=hosting_call(&mut peer).await;
            assert_eq!(uri.as_slice(),b"[username:fixture-user@account-domain.invalid]");assert_eq!(flag.as_slice(),b"4");
            let(uri,flag)=hosting_call(&mut peer).await;
            assert_eq!(uri.as_slice(),b"[username:fixture-user@account-domain.invalid]");assert_eq!(flag.as_slice(),b"3");
            let packet=peer.next().await.unwrap();let call=decode_request(&packet[1..]).unwrap();
            assert_eq!(call.method,b"logoutSession.AccountEntry.Account");assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"DestEntry");
            peer.send(&[reply(call.call_id,b"\0\x01\0\0\0\0")]).await;while peer.next().await.is_some(){}
        };
        let _=tokio::join!(bootstrap,destination);
    };
    let client=async move{
        let p=RuntimeProfile::supplied_common_bootstrap(bootstrap_inputs(),bootstrap_common(&cert.root,port),bootstrap_route()).unwrap();
        let connector=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,mut events)=NativeHostingBackend::new(p,connector.clone());let original=owner();
        backend.login(original.clone(),SdkLoginRequest{server_address:BOOTSTRAP_SDK,..request(&cert.root)}).await.unwrap();
        let scope=backend.original_scope(&original).unwrap();backend.pause_original_polls_for_test(&original,&scope,true).unwrap();
        assert!(backend.original(&original).unwrap().bootstrap_handle.lock().await.is_none());
        let event=tokio::time::timeout(Duration::from_secs(4),events.recv()).await.unwrap().unwrap();
        assert!(event.matches(&original,&scope));
        assert!(matches!(event.payload,BackendEventPayload::Callback(c) if c.callback==(Callback::SdkLogin{success:true})));
        assert_eq!(connector.calls.load(Ordering::Acquire),0);
        backend.update_status(original.clone(),STATUS_REGISTER,gateway()).await.unwrap();
        backend.update_status(original.clone(),STATUS_UNREGISTER,gateway()).await.unwrap();
        backend.logout(original.clone()).await.unwrap();assert!(!scope.is_open());
        assert!(lock(&backend.original(&original).unwrap().state).released);
    };
    let _=tokio::join!(server,client,relay);
}

fn inputs()->RuntimeProfileInputs<'static>{RuntimeProfileInputs{
    route:PacketRoute::NativeCommon,authority:"",enable_sni:false,identity_type:"username",domain_name:"fixture.invalid",domain_id:"fixture-domain",application_id:"fixture-app",device_info_cc:"fixture-cc",
    client_properties:&[RpcProperty{key:b"ClientId",value:b"fixture-client-id"}],include_device_id:false,login_routing:&[],gateway_uri:"fixture-uri",gateway_metadata:&[],poll_routing:&[],logout_routing:&[],incoming:None,
    reference_decode_key:None,handshake_timeout:Duration::from_secs(3),partial_timeout:Duration::from_secs(1),io_timeout:Duration::from_secs(2),rpc_timeout:Duration::from_secs(3),
}}
fn common(root:&str,port:u16)->RuntimeCommonInputs<'static>{RuntimeCommonInputs{router:policy(root,port),account_reference:ENTRY,access:&[],endpoint_index:0,resolved_host_index:0,
    connect_timeout:Duration::from_secs(2),startup_timeout:Duration::from_secs(5),attempt_timeout:Duration::from_secs(8),compress_above:None}}
fn profile(root:&str,port:u16)->RuntimeProfile{RuntimeProfile::supplied_common(inputs(),common(root,port)).unwrap()}
fn request(root:&str)->SdkLoginRequest<'_>{SdkLoginRequest{account_identity:"fixture-user",phone_login_token:"fixture-phone-token",application_key:"fixture-app-key",server_address:ENTRY,device_id:None,
    selected_session_id:Some("fixture-selected-session"),root_der_base64:root,tls_only:true}}
struct LegacyDenied{calls:AtomicUsize}
impl RuntimeConnector for LegacyDenied{fn connect<'a>(&'a self,_:&'a AttemptOwner,_:&'a RpcConnectionScope,_:ConnectionInput<'a>)->ConnectFuture<'a>{
    self.calls.fetch_add(1,Ordering::AcqRel);Box::pin(async{Err(ConnectError::UnsupportedProfile)})
}}
fn reply(id:i32,stream:&[u8])->Vec<u8>{let rpc=encode_reply(id,0,stream).unwrap();let mut packet=vec![1];packet.extend_from_slice(rpc.expose_to_transport());packet}
fn login_result()->Vec<u8>{let mut w=Writer::new();w.boolean(true).unwrap();w.bytes(b"fixture-user").unwrap();w.bytes(b"fixture-auth").unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}

#[test]
fn native_profile_captures_physical_and_logical_namespaces_and_fills_only_absent_ids(){
    let cert=Certificates::generate("IP:127.0.0.1");let p=profile(&cert.root,12345);assert!(p.authority.is_empty());let route=p.common.as_ref().unwrap();assert_eq!(route.reference.as_ref().unwrap().as_str(),ENTRY);
    assert_eq!(route.entry.as_ref().unwrap().identity(),b"AccountEntry");for fields in [&p.login_routing,&p.poll_routing,&p.logout_routing]{assert_eq!(fields.iter().find(|p|p.key==b"id").unwrap().value,b"AccountEntry");}
    // Native hosting targets the original default SipGateway object, not the
    // Account object and not a placeholder.
    assert_eq!(p.gateway_metadata.len(),1);assert_eq!(p.gateway_metadata[0].key,b"id");
    assert_eq!(p.gateway_metadata[0].value,super::super::rpc_arguments::DEFAULT_GATEWAY_OBJECT.as_bytes());
    assert_eq!(p.domain_name.as_str(),"fixture.invalid");assert_eq!(p.domain_id.as_str(),"fixture-domain");assert!(p.bootstrap.is_none());
}
#[test]
fn native_selector_cannot_be_consumed_by_old_inner_tls_carrier_adapter(){
    let owner=owner();let mut arc=super::super::arc_session::ArcRpcSession::new(owner);assert!(arc.take_packet_transport(PacketRoute::NativeCommon).is_err());
    assert!(arc.take_packet_transport(PacketRoute::DirectTlsCarrier).is_ok());assert!(RuntimeProfile::supplied(inputs()).is_err());
}
#[test]
fn conflicting_inner_tls_authority_or_reference_identity_is_rejected_not_silently_ignored(){
    let cert=Certificates::generate("IP:127.0.0.1");
    assert!(RuntimeProfile::supplied_common(RuntimeProfileInputs{authority:"127.0.0.1",..inputs()},common(&cert.root,12345)).is_err());
    assert!(RuntimeProfile::supplied_common(RuntimeProfileInputs{enable_sni:true,..inputs()},common(&cert.root,12345)).is_err());
    assert!(RuntimeProfile::supplied_common(RuntimeProfileInputs{login_routing:&[RpcProperty{key:b"id",value:b"ForeignEntry"}],..inputs()},common(&cert.root,12345)).is_err());
    assert!(RuntimeProfile::supplied_common(RuntimeProfileInputs{poll_routing:&[RpcProperty{key:b"id",value:b"ForeignEntry"}],..inputs()},common(&cert.root,12345)).is_err());
}
#[test]
fn unsupported_mixed_entry_access_alias_and_selection_fail_pure_profile_admission(){
    let cert=Certificates::generate("IP:127.0.0.1");for entry in ["AccountEntry:tcp -h 127.0.0.1 -p 98;","AccountEntry:sarc -h 42.7 -p 98;tcp -h 127.0.0.1 -p 98;","AccountEntry:sarc -h unknown.invalid -p 98;"]{
        assert!(RuntimeProfile::supplied_common(inputs(),RuntimeCommonInputs{account_reference:entry,..common(&cert.root,12345)}).is_err());}
    assert!(RuntimeProfile::supplied_common(inputs(),RuntimeCommonInputs{access:&[RpcProperty{key:b"pwd",value:b"not-router-access"}],..common(&cert.root,12345)}).is_err());
    assert!(RuntimeProfile::supplied_common(inputs(),RuntimeCommonInputs{resolved_host_index:1,..common(&cert.root,12345)}).is_err());
}
#[test]
fn native_profile_budgets_and_debug_reject_oversize_or_exposed_material(){
    let cert=Certificates::generate("IP:127.0.0.1");assert!(RuntimeProfile::supplied_common(inputs(),RuntimeCommonInputs{attempt_timeout:Duration::from_secs(121),..common(&cert.root,12345)}).is_err());
    assert!(RuntimeProfile::supplied_common(inputs(),RuntimeCommonInputs{connect_timeout:Duration::ZERO,..common(&cert.root,12345)}).is_err());
    let p=profile(&cert.root,12345);let text=format!("{p:?}");for value in ["AccountEntry","12345","fixture","BEGIN CERTIFICATE"]{assert!(!text.contains(value));}
}
#[test]
fn sdk_request_cannot_retarget_captured_native_profile_to_an_equal_looking_new_entry(){
    let cert=Certificates::generate("IP:127.0.0.1");let p=profile(&cert.root,12345);assert!(LoginCapture::capture(request(&cert.root),&p).is_ok());
    for entry in ["OtherEntry:sarc -h 42.7 -p 98;","AccountEntry:sarc -h 42.8 -p 98;","AccountEntry:arc -h 42.7 -p 98;"]{
        assert!(LoginCapture::capture(SdkLoginRequest{server_address:entry,..request(&cert.root)},&p).is_err());}
}
#[tokio::test]
async fn shared_hosting_backend_selects_native_actor_without_calling_legacy_connector(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let stored=super::super::account_config::tests::native_fixture(&cert.root,"127.0.0.1",listener.local_addr().unwrap().port());
    let encoded=stored.encode_private().unwrap();let loaded=super::super::account_config::AccountConfig::decode_private(&encoded).unwrap();assert_eq!(loaded.validate(100),Ok(()));
    let p=loaded.runtime_profile().unwrap();let config=cert.server_config(&TLS13);
    let server=async move{tokio::time::timeout(Duration::from_secs(12),async move{let mut peer=CommonPeer::open(listener,config).await;let packet=peer.next().await.unwrap();let call=decode_request(&packet[1..]).unwrap();assert_eq!(call.method,b"loginSession.AccountEntry.Account");assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"AccountEntry");peer.send(&[reply(call.call_id,&login_result())]).await;
        let packet=peer.next().await.unwrap();let call=decode_request(&packet[1..]).unwrap();assert_eq!(call.method,b"logoutSession.AccountEntry.Account");assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"AccountEntry");peer.send(&[reply(call.call_id,b"\0\x01\0\0\0\0")]).await;peer.no_more_common().await;}).await.unwrap()};
    let client=async move{let connector=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,mut events)=NativeHostingBackend::new(p,connector.clone());let original=owner();backend.login(original.clone(),request(&cert.root)).await.unwrap();let scope=backend.original_scope(&original).unwrap();backend.pause_original_polls_for_test(&original,&scope,true).unwrap();
        let event=tokio::time::timeout(Duration::from_secs(3),events.recv()).await.unwrap().unwrap();assert!(event.matches(&original,&scope));assert!(matches!(event.payload,BackendEventPayload::Callback(c) if c.callback==(Callback::SdkLogin{success:true})));
        assert!(lock(&backend.original(&original).unwrap().state).configured);assert_eq!(connector.calls.load(Ordering::Acquire),0);assert_eq!(backend.login(owner(),request(&cert.root)).await,Err(BackendFailure::Unavailable));
        backend.logout(original.clone()).await.unwrap();assert!(!scope.is_open());assert!(lock(&backend.original(&original).unwrap().state).released);
    };tokio::join!(server,client);
}
#[tokio::test]
async fn changed_native_sdk_reference_is_rejected_before_any_tcp_or_backend_slot(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=profile(&cert.root,listener.local_addr().unwrap().port());let connector=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,_events)=NativeHostingBackend::new(p,connector.clone());
    assert_eq!(backend.login(owner(),SdkLoginRequest{server_address:"AccountEntry:sarc -h 42.8 -p 98;",..request(&cert.root)}).await,Err(BackendFailure::Unavailable));assert!(lock(&backend.state).slot.is_none());assert_eq!(connector.calls.load(Ordering::Acquire),0);
    assert!(tokio::time::timeout(Duration::from_millis(20),listener.accept()).await.is_err());
}
#[tokio::test]
async fn native_bootstrap_account_hosts_and_persists_received_sip_message_end_to_end(){
    let cert=Certificates::generate("IP:127.0.0.1");
    let front=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let bootstrap_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let destination_listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();
    let port=front.local_addr().unwrap().port();
    let bootstrap_port=bootstrap_listener.local_addr().unwrap().port();let destination_port=destination_listener.local_addr().unwrap().port();
    let config=cert.server_config(&TLS13);let bootstrap_config=config.clone();
    let relay=async move{for target in [bootstrap_port,destination_port]{
        let(mut inbound,_)=front.accept().await.unwrap();
        let mut outbound=tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST,target)).await.unwrap();
        tokio::spawn(async move{let _=tokio::io::copy_bidirectional(&mut inbound,&mut outbound).await;});
    }};
    let server=async move{
        let bootstrap=async move{let mut peer=CommonPeer::open(bootstrap_listener,bootstrap_config).await;
            let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let call=decode_request(&packet[1..]).unwrap();
            assert_eq!(call.method,b"getAppConfigs.AccountEntry.Account");
            assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"BootEntry");
            peer.send(&[reply(call.call_id,&bootstrap_result())]).await;while peer.next().await.is_some(){}
        };
        let destination=async move{let mut peer=CommonPeer::open(destination_listener,config).await;
            let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let call=decode_request(&packet[1..]).unwrap();
            assert_eq!(call.method,b"loginSession.AccountEntry.Account");
            assert_eq!(call.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"DestEntry");
            peer.send(&[reply(call.call_id,&login_result_for_account())]).await;
            // The account runtime registers hosting by itself once authenticated.
            let(uri,flag)=hosting_call(&mut peer).await;
            assert_eq!(uri.as_slice(),b"[username:fixture-account@account-domain.invalid]");
            assert_eq!(flag.as_slice(),b"4");
            // One original Sip.Request text message on the configured receiver.
            let mut args=Writer::versioned().unwrap();args.bytes(b"Sip.Request").unwrap();
            args.properties(&[RpcProperty{key:b"Sip.Message.From",value:b"fixture-sender"},
                RpcProperty{key:b"Sip.Message.Time",value:b"1700000000123"},
                RpcProperty{key:b"Sip.MessageGw.Oid",value:b"fixture-gateway-oid"}]).unwrap();
            args.bytes("fixture-original-sms 🥘".as_bytes()).unwrap();
            let envelope=encode_request(91,&[RpcProperty{key:b"id",value:b"#fixture-client"},
                RpcProperty{key:b"domain",value:b"fixture-domain"},RpcProperty{key:b"__magic",value:&[0,0xff]}],
                super::super::rpc_incoming::ONLINE_MESSAGE_METHOD.as_bytes(),args.finish().expose_to_transport()).unwrap();
            let mut incoming=vec![0];incoming.extend_from_slice(envelope.expose_to_transport());peer.send(&[incoming]).await;
            // Polls may arrive first; only the original DIRECT reply proves delivery.
            loop{
                let packet=peer.next().await.unwrap();
                if packet[0]==1{let reply=decode_reply(&packet[1..]).unwrap();assert_eq!(reply.call_id,91);
                    assert_eq!(reply.result_stream,b"\0\x01\0\0\0\0");break;}
                let call=decode_request(&packet[1..]).unwrap();
                let mut args=Reader::versioned(call.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();
                let payload=super::super::rpc_batch::read_payload(&mut args).unwrap();
                peer.send(&[reply(call.call_id,&super::tests::swap_result(None,true,payload.fields))]).await;
            }
            let(_uri,flag)=hosting_call(&mut peer).await;assert_eq!(flag.as_slice(),b"3");
            // The unregister request stays retained until its ACK, so polls or a
            // retransmission may still arrive before the SDK logout.
            let logout_id=loop{
                let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);
                let call=decode_request(&packet[1..]).unwrap();
                if call.method==b"logoutSession.AccountEntry.Account"{break call.call_id;}
                assert_eq!(call.method,super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
                let mut args=Reader::versioned(call.arguments).unwrap();args.bytes().unwrap();args.bytes().unwrap();
                let payload=super::super::rpc_batch::read_payload(&mut args).unwrap();
                let mut sequence=None;
                for(key,nested) in payload.requests.iter(){
                    assert!(nested.metadata.iter().any(|p|p.key==b"id"&&p.value==super::super::rpc_arguments::DEFAULT_GATEWAY_OBJECT.as_bytes()));
                    let mut fields=Reader::versioned(nested.arguments).unwrap();fields.bytes().unwrap();
                    let flag=fields.properties().unwrap().iter().find(|p|p.key==b"SipFlag").unwrap().value.to_vec();
                    assert_eq!(flag.as_slice(),b"3");sequence=Some(*key);
                }
                peer.send(&[reply(call.call_id,&super::tests::swap_result(sequence,true,payload.fields))]).await;
            };
            peer.send(&[reply(logout_id,b"\0\x01\0\0\0\0")]).await;while peer.next().await.is_some(){}
        };
        let _=tokio::join!(bootstrap,destination);
    };
    let client=async move{
        let temp=super::super::account_test_support::TempDatabase::new();
        let manager=super::super::account_runtime::AccountManager::new(temp.database.clone());
        let created=manager.create(bootstrap_account_config(&cert.root,port),100).await.unwrap();
        let line=created.account.line_id.clone();
        manager.start(&line,1,100).await.unwrap();
        let hosted=tokio::time::timeout(Duration::from_secs(10),async{loop{
            let state=manager.get(&line,100).await.unwrap();
            if state.phase=="hosted"{return state;}
            tokio::time::sleep(Duration::from_millis(5)).await;
        }}).await.unwrap();
        assert!(hosted.incoming_message_ready);assert!(!hosted.incoming_call_ready);
        assert_eq!(hosted.provider_availability,"available");
        let inbox=tokio::time::timeout(Duration::from_secs(6),async{loop{
            let rows=manager.inbox(&line,50).unwrap();
            if rows.first().is_some_and(|row|row.reply_state=="direct_submitted"){return rows;}
            tokio::time::sleep(Duration::from_millis(5)).await;
        }}).await.unwrap();
        assert_eq!(inbox.len(),1);assert_eq!(inbox[0].classification,"smgw_text");
        let messages=manager.messages(&line,50).unwrap();assert_eq!(messages.len(),1);
        assert_eq!(messages[0].classification,"smgw_text");
        assert_eq!(messages[0].message.content,"fixture-original-sms 🥘");
        assert_eq!(messages[0].message.phone_number,"fixture-sender");
        manager.stop(&line,1,100).await.unwrap();
        assert_eq!(manager.get(&line,100).await.unwrap().phase,"stopped");
    };
    let _=tokio::join!(server,client,relay);
}
fn login_result_for_account()->Vec<u8>{
    let mut w=Writer::new();w.boolean(true).unwrap();w.bytes(b"fixture-user").unwrap();w.bytes(b"fixture-auth").unwrap();
    w.properties(&[RpcProperty{key:b"mainAccountServer",value:b"DestEntry"},
        RpcProperty{key:b"mainAccount",value:b"fixture-main-account"}]).unwrap();
    w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()
}
/// Stored config for the SAME native bootstrap route, expressed only as private JSON:
/// the account manager must assemble it without any hand-typed URI or destination.
fn bootstrap_account_config(root:&str,port:u16)->super::super::account_config::AccountConfig{
    let mut json=super::super::account_test_support::config_json(root);
    json["credentials"]["sdk"]["server_address"]=serde_json::json!(BOOTSTRAP_SDK);
    json["credentials"]["sdk"]["root_der_base64"]=serde_json::json!(root);
    json["profile"]["route"]=serde_json::json!({"router_common":{
        "reference":format!("Router:tls -h 127.0.0.1 -p {port} -ri 12001;"),
        "anchors":[{"router_id":12001,"host":"127.0.0.1","port":port,"authority":"127.0.0.1","root_der_base64":root,"enable_sni":false}],
        "access":[],"endpoint_index":0,"resolved_host_index":0,"startup_timeout_ms":5000,"attempt_timeout_ms":8000,"compress_above":null}});
    json["profile"]["authority"]=serde_json::json!("");json["profile"]["enable_sni"]=serde_json::json!(false);
    for key in ["domain_name","domain_id","application_id","device_info_cc"]{json["profile"][key]=serde_json::Value::Null;}
    json["profile"]["gateway_uri"]=serde_json::json!("");
    json["profile"]["gateway_metadata"]=serde_json::json!([]);
    json["profile"]["bootstrap_via_router"]=serde_json::json!({"routing":[],"handoff_timeout_ms":10000});
    json["profile"]["incoming"]["receiver_kind"]=serde_json::json!(b"Sip.Request");
    serde_json::from_value(json).unwrap()
}
#[tokio::test]
async fn cancelled_native_backend_connect_retires_original_slot_and_releases_unattached_tcp(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=profile(&cert.root,listener.local_addr().unwrap().port());let connector=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,_events)=NativeHostingBackend::new(p,connector);let original=owner();
    let server=async move{let(mut tcp,_)=listener.accept().await.unwrap();let mut bytes=[0u8;8192];let n=tcp.read(&mut bytes).await.unwrap();assert!(n>0);loop{match tcp.read(&mut bytes).await{Ok(0)|Err(_)=>break,Ok(_)=>{}}}};
    let client=async{assert!(tokio::time::timeout(Duration::from_millis(80),backend.login(original.clone(),request(&cert.root))).await.is_err());let scope=backend.original_scope(&original).unwrap();assert!(!scope.is_open());assert!(backend.original(&original).unwrap().handle.lock().await.is_none());
        backend.logout(original.clone()).await.unwrap();assert!(lock(&backend.original(&original).unwrap().state).released);};tokio::join!(server,client);
}
