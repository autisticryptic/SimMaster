//! Actions-only codec fixtures for original direct replay/compaction tests.
//! This is not actor-write or real-service proof; those use the native manager.
use super::{arc_protocol::{ArcProtocol,IncomingProtocolMessage,ProtocolEvent},arc_session::ArcLoginInputs,
    rpc_envelope::{encode_request,encode_reply},rpc_stream::{RpcProperty,Writer},rpc_incoming::{IncomingBinding,ONLINE_MESSAGE_METHOD},
    session::AttemptOwner,transport_packets::{encode_call_packets,CallKind,TLS_FRAGMENT_THRESHOLD}};

pub(super) fn authenticated(owner:AttemptOwner)->ArcProtocol {
    let mut protocol=ArcProtocol::new(owner.clone());let scope=protocol.connection_scope();
    let login=protocol.prepare_login(ArcLoginInputs{typed_identity:b"username:fixture-account",domain_name:b"fixture.invalid",
        selected_session_id:b"fixture-selector",phone_login_token:b"fixture-token",application_key:b"fixture-app-key",
        domain_id:b"fixture-domain",application_id:b"fixture-app",device_info_cc:b"",client_properties:&[]},&[]).unwrap();
    let mut result=Writer::new();result.boolean(true).unwrap();result.bytes(b"fixture-user").unwrap();result.bytes(b"fixture-auth").unwrap();
    result.properties(&[]).unwrap();result.bytes(b"").unwrap();
    let reply=encode_reply(login.call_id,0,result.finish().expose_to_transport()).unwrap();
    for packet in encode_call_packets(CallKind::Reply,reply.expose_to_transport(),TLS_FRAGMENT_THRESHOLD).unwrap(){
        protocol.on_plaintext_packet(&owner,&scope,packet.expose_to_transport(),|_|{}).unwrap();
    }
    protocol.install_incoming(IncomingBinding{object_id:b"#fixture-client",domain:b"fixture-domain",receiver_kind:b"Sip.Request"}).unwrap();
    protocol
}
pub(super) fn wire(id:i32,magic:&[u8],body:&[u8])->Vec<u8> {
    let mut arguments=Writer::versioned().unwrap();arguments.bytes(b"Sip.Request").unwrap();
    arguments.properties(&[RpcProperty{key:b"Sip.Message.From",value:b"fixture-sender"}]).unwrap();arguments.bytes(body).unwrap();
    encode_request(id,&[RpcProperty{key:b"id",value:b"#fixture-client"},RpcProperty{key:b"domain",value:b"fixture-domain"},
        RpcProperty{key:b"__magic",value:magic}],ONLINE_MESSAGE_METHOD.as_bytes(),arguments.finish().expose_to_transport()).unwrap().expose_to_transport().to_vec()
}
pub(super) fn incoming(protocol:&mut ArcProtocol,owner:&AttemptOwner,id:i32,body:&[u8])->IncomingProtocolMessage {
    let scope=protocol.connection_scope();let bytes=wire(id,&id.to_be_bytes(),body);let mut message=None;
    for packet in encode_call_packets(CallKind::Request,&bytes,TLS_FRAGMENT_THRESHOLD).unwrap(){
        protocol.on_plaintext_packet(owner,&scope,packet.expose_to_transport(),|event|match event{
            ProtocolEvent::Incoming(value)=>message=Some(value),_=>panic!("original direct message expected"),
        }).unwrap();
    }
    message.unwrap()
}
