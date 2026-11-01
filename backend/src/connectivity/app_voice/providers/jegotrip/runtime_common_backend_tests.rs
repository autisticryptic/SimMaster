//! Actions-only native profile -> shared HostingBackend/actor integration.
use super::*;
use super::super::{common_runtime::tests::CommonPeer,netstream_runtime::tests::{owner,policy},
    tls_engine::tests::Certificates,rpc_envelope::{encode_reply,decode_request},rpc_stream::Writer,
    runtime_connector::{ConnectFuture,ConnectError}};
use std::sync::atomic::{AtomicUsize,Ordering};
use tokio::{net::TcpListener,io::AsyncReadExt};
use tokio_rustls::rustls::version::TLS13;
const ENTRY:&str="AccountEntry:sarc -h 42.7 -p 98;";
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
    let cert=Certificates::generate("IP:127.0.0.1");let p=profile(&cert.root,12345);assert!(p.authority.is_empty());let route=p.common.as_ref().unwrap();assert_eq!(route.reference.as_str(),ENTRY);
    assert_eq!(route.entry.identity(),b"AccountEntry");for fields in [&p.login_routing,&p.poll_routing,&p.logout_routing]{assert_eq!(fields.iter().find(|p|p.key==b"id").unwrap().value,b"AccountEntry");}
    assert!(p.gateway_metadata.is_empty());assert_eq!(p.domain_name.as_str(),"fixture.invalid");assert_eq!(p.domain_id.as_str(),"fixture-domain");assert!(p.bootstrap.is_none());
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
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=profile(&cert.root,listener.local_addr().unwrap().port());let config=cert.server_config(&TLS13);
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
async fn cancelled_native_backend_connect_retires_original_slot_and_releases_unattached_tcp(){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let p=profile(&cert.root,listener.local_addr().unwrap().port());let connector=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});let(backend,_events)=NativeHostingBackend::new(p,connector);let original=owner();
    let server=async move{let(mut tcp,_)=listener.accept().await.unwrap();let mut bytes=[0u8;8192];let n=tcp.read(&mut bytes).await.unwrap();assert!(n>0);loop{match tcp.read(&mut bytes).await{Ok(0)|Err(_)=>break,Ok(_)=>{}}}};
    let client=async{assert!(tokio::time::timeout(Duration::from_millis(80),backend.login(original.clone(),request(&cert.root))).await.is_err());let scope=backend.original_scope(&original).unwrap();assert!(!scope.is_open());assert!(backend.original(&original).unwrap().handle.lock().await.is_none());
        backend.logout(original.clone()).await.unwrap();assert!(lock(&backend.original(&original).unwrap().state).released);};tokio::join!(server,client);
}
