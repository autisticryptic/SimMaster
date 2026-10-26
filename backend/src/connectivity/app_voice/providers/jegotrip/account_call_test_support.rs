//! Shared Actions-only basic account call fixtures; synthetic SDK credentials,
//! original TLS/Common exchanges and independent media TCP peers only.
use super::{account_config::AccountConfig,account_test_support::config_json,rpc_calls::{CallNoticeKind,CallOperation},
    rpc_stream::{Reader,Writer,RpcProperty},rpc_envelope::{encode_request,decode_request},runtime_backend::tests::{Peer,next_non_poll},
    session::STATUS_UNREGISTER};
use std::net::SocketAddr;
pub(crate) const NATIVE_ID:u64=0x0123_4567_89ab_cdef;
pub(crate) fn call_config(sdk_root:&str,media_root:&str)->AccountConfig{
    serde_json::from_value(call_config_json(sdk_root,media_root)).unwrap()
}
pub(crate) fn call_config_json(sdk_root:&str,media_root:&str)->serde_json::Value{
    let mut config=config_json(sdk_root);
    config["profile"]["call_incoming"]=serde_json::json!({"object_id":b"#fixture-call-client","domain":b"fixture-domain"});
    config["profile"]["call_control"]=serde_json::json!([{"key":b"id","value":b"#fixture-base-server"},{"key":b"domain","value":b"fixture-domain"}]);
    config["call_media"]=serde_json::json!({"authority":"127.0.0.1","root_der_base64":media_root,"enable_sni":true,
        "routing":[{"key":b"id","value":b"#fixture-endpoint"},{"key":b"domain","value":b"fixture-domain"}],"features":[],"properties":[],
        "reference_decode_key":null,"audio_bind_ip":"127.0.0.1","allow_plain_media":true,"trunk":null});config
}
pub(crate) fn sdp(endpoint:SocketAddr)->Vec<u8>{format!("v=0\r\no=fixture 10 11 IN IP4 {}\r\ns=fixture\r\nc=IN IP4 {}\r\nt=0 0\r\nm=audio {} RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=sendrecv\r\na=rtcp-mux\r\n",endpoint.ip(),endpoint.ip(),endpoint.port()).into_bytes()}
pub(crate) async fn notice(peer:&mut Peer,kind:CallNoticeKind,id:i32,body:&[u8],domain:&[u8]){
    let mut data=Writer::versioned().unwrap();data.u64(NATIVE_ID).unwrap();if kind==CallNoticeKind::Invited{data.bytes(b"fixture-caller").unwrap();}
    data.properties(&[RpcProperty{key:b"callServerEx.oid",value:b"#fixture-call-server"},RpcProperty{key:b"RseEx.SDP",value:body}]).unwrap();let data=data.finish();
    let method=match kind{CallNoticeKind::Invited=>super::rpc_calls::INVITED_METHOD,CallNoticeKind::Accepted=>super::rpc_calls::ACCEPTED_METHOD,CallNoticeKind::Released=>super::rpc_calls::RELEASED_METHOD};
    let magic=id.to_be_bytes();let request=encode_request(id,&[RpcProperty{key:b"id",value:b"#fixture-call-client"},RpcProperty{key:b"domain",value:domain},RpcProperty{key:b"__magic",value:&magic}],method.as_bytes(),data.expose_to_transport()).unwrap();
    peer.send_call(super::transport_packets::CallKind::Request,request.expose_to_transport()).await;
}
pub(crate) async fn operation(peer:&mut Peer,operation:CallOperation)->(i32,Vec<u8>){
    let packet=next_non_poll(peer).await;let request=decode_request(packet.payload.expose_to_transport()).unwrap();assert_eq!(request.method,operation.method().as_bytes());
    let expected=if operation==CallOperation::Inform{b"#fixture-base-server".as_slice()}else{b"#fixture-call-server".as_slice()};
    assert_eq!(request.metadata.iter().find(|property|property.key==b"id").unwrap().value,expected);
    assert!(request.metadata.iter().any(|property|property.key==b"token"));assert!(!request.metadata.iter().any(|property|property.key==b"pwd"));
    let mut reader=Reader::versioned(request.arguments).unwrap();assert_eq!(reader.u64().unwrap(),NATIVE_ID);let properties=reader.properties().unwrap();reader.finish().unwrap();
    let sdp=properties.iter().find(|property|property.key==b"RseEx.SDP").map_or(Vec::new(),|property|property.value.to_vec());(request.call_id,sdp)
}
pub(crate) async fn reply_operation(peer:&mut Peer,id:i32,operation:CallOperation,accepted:bool,endpoint:Option<&[u8]>){
    let mut result=Writer::new();result.boolean(accepted).unwrap();if operation==CallOperation::Accept{
        let properties=endpoint.map(|endpoint|vec![RpcProperty{key:b"endpoint",value:endpoint},RpcProperty{key:b"content",value:b"fixture-original-content"}]).unwrap_or_default();
        result.properties(&properties).unwrap();
    }result.bytes(b"").unwrap();peer.reply(id,result.finish().expose_to_transport()).await;
}
pub(crate) async fn finish_sdk(mut peer:Peer){
    peer.swap(Some(STATUS_UNREGISTER),true).await;let packet=next_non_poll(&mut peer).await;let request=decode_request(packet.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method,super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());peer.reply(request.call_id,&[0,1,0,0,0,0]).await;assert!(peer.receive().await.is_none());
}
