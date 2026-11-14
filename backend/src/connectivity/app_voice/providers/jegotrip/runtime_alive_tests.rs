//! Actions-only independent Alive runtime; no Provider/device/original SDK.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use n::{alive_profile::{AccountAliveConfig,AccountAliveEndpoint,AlivePolicy},alive_runtime::{AliveBudgets,AliveRun,AliveBackoff},
    alive_session::{AliveLink,AliveAgent,AliveResult},arc_session::{ArcRpcSession,ArcLoginInputs,ArcReply},rpc_pending::RpcConnectionScope,
    rpc_stream::RpcProperty,native_receiver::{NativeReceiverConfig,NativeReceiverPolicy},rpc_client_ex::CHECK_ALIVE_METHOD};
#[path="alive_session_tests.rs"]
mod session;
#[path="alive_io_tests.rs"]
mod io;
#[path="alive_account_tests.rs"]
mod account;
fn policy_config(port:u16,approved:bool)->AccountAliveConfig{AccountAliveConfig{allow_legacy_stcp:approved,interval_seconds:60,endpoints:vec![AccountAliveEndpoint{host:"127.0.0.1".into(),port}]}}
fn receiver()->Arc<NativeReceiverPolicy>{NativeReceiverPolicy::capture(&NativeReceiverConfig{direct_domains:None}).unwrap()}
fn authenticated()->(ArcRpcSession,AttemptOwner,RpcConnectionScope,Arc<AliveLink>){
    let original=owner();let mut rpc=ArcRpcSession::new(original.clone());rpc.claim_common_transport().unwrap();rpc.bind_common_identity(b"AccountEntry").unwrap();let scope=rpc.connection_scope();
    let(id,_)=rpc.prepare_login(ArcLoginInputs{typed_identity:b"username:fixture",domain_name:b"fixture.invalid",selected_session_id:b"fixture-alive-selector",
        phone_login_token:b"fixture-phone-token",application_key:b"fixture-app-key",domain_id:b"fixture-domain",application_id:b"fixture-app",device_info_cc:b"",client_properties:&[]},&[]).unwrap();
    let mut w=Writer::new();w.boolean(true).unwrap();w.bytes(b"fixture-alive-user").unwrap();w.bytes(b"fixture-alive-token").unwrap();
    w.properties(&[RpcProperty{key:b"mainAccountServer",value:b"AccountEntry"},RpcProperty{key:b"mainAccount",value:b"fixture-main-account"}]).unwrap();w.bytes(b"").unwrap();
    let wire=encode_reply(id,0,w.finish().expose_to_transport()).unwrap();assert!(matches!(rpc.on_reply(&original,&scope,wire.expose_to_transport()).unwrap(),Some(ArcReply::Login(_))));
    let link=rpc.alive_link();(rpc,original,scope,link)
}
fn alive_reply(id:i32,accepted:bool,reason:&[u8])->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();w.bytes(reason).unwrap();if !reason.is_empty(){w.properties(&[]).unwrap();}
    encode_reply(id,0,w.finish().expose_to_transport()).unwrap().expose_to_transport().to_vec()}
fn check_request(id:i32)->Vec<u8>{let arguments=Writer::versioned().unwrap().finish();encode_request(id,&[RpcProperty{key:b"id",value:b"MessageReceiver"}],CHECK_ALIVE_METHOD.as_bytes(),arguments.expose_to_transport()).unwrap().expose_to_transport().to_vec()}
fn registered()->(ArcRpcSession,AttemptOwner,RpcConnectionScope,Arc<AliveLink>,AliveAgent,Instant){
    let(rpc,owner,scope,link)=authenticated();link.claim(&owner,&scope,60).unwrap();let mut agent=link.agent().unwrap();let now=Instant::now();let request=agent.prepare(1700000000123,now,now+Duration::from_secs(2)).unwrap();
    let id=decode_request(request.expose_to_transport()).unwrap().call_id;assert!(matches!(agent.response(&alive_reply(id,true,b""),now).unwrap(),Some(AliveResult::Accepted)));
    (rpc,owner,scope,link,agent,now)
}
