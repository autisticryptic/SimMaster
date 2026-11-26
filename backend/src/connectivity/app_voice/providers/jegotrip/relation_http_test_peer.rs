//! Actions-only original CommonPeer coordinator for authenticated HTTP tests.
//! Compiled only under cfg(test); no caller-supplied route, login or wire reply.
use super::{common_runtime::tests::CommonPeer,rpc_envelope::{decode_request,encode_reply},
    rpc_stream::{Reader,Writer},rpc_batch,rpc_arguments,rpc_login,rpc_logout,runtime_backend};
use crate::platform::db::Database;
use std::{sync::Arc,time::Duration};
use tokio::{net::TcpListener,sync::oneshot};
use tokio_rustls::rustls::ServerConfig;
fn reply(id:i32,stream:&[u8])->Vec<u8>{let raw=encode_reply(id,0,stream).unwrap();let mut p=vec![1];p.extend_from_slice(raw.expose_to_transport());p}
#[path="buddy_check_http_peer.rs"]
mod buddy_check;
pub(crate) use buddy_check::serve_buddy_check;
#[path="group_management_http_peer.rs"]
mod management;
pub(crate) use management::serve_group_management;
#[derive(Default)]struct SdkWrites{hosting:Option<i32>,unhosting:Option<i32>}
async fn swap(peer:&mut CommonPeer,p:&[u8],wanted:Option<&[u8]>,known:&mut SdkWrites)->bool{
    assert_eq!(p[0],0);let request=decode_request(&p[1..]).unwrap();assert_eq!(request.method,rpc_batch::SWAP_CALLS_METHOD.as_bytes(),"HTTP relation operation never becomes an unrelated RPC or nested relation resend");
    let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-original-selector");let payload=rpc_batch::read_payload(&mut r).unwrap();
    let mut selected=None;let mut found=false;
    for(id,nested)in &payload.requests{
        assert_eq!(nested.method,rpc_arguments::UPDATE_STATUS_METHOD.as_bytes());assert_eq!(nested.metadata.iter().find(|v|v.key==b"id").unwrap().value,b"#fixture-gateway");
        let mut r=Reader::versioned(nested.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-exact-gateway-uri");let props=r.properties().unwrap();let flag=props.iter().find(|p|p.key==b"SipFlag").unwrap().value;
        let slot=match flag{b"4"=>&mut known.hosting,b"3"=>&mut known.unhosting,_=>panic!("unrelated SDK business flag")};
        if let Some(original)=*slot{assert_eq!(*id,original,"only the same retained original SDK request, never a new business action");}else{assert!(*id>0);assert_eq!(Some(flag),wanted,"a new SDK request needs this exact current fixture phase");*slot=Some(*id);}
        if Some(flag)==wanted{found=true;}selected=Some(*id);
    }
    // Native batching may retain a previously acknowledged SDK request until
    // its own transport epoch ACK. Only its recorded ORIGINAL id is tolerated;
    // this does not tolerate any relation/IM RPC, new id or unknown replay.
    peer.send(&[reply(request.call_id,&runtime_backend::tests::swap_result(selected,true,payload.fields))]).await;found
}
pub(crate) async fn serve(listener:TcpListener,server:Arc<ServerConfig>,database:Arc<Database>,line:String,mut finish:oneshot::Receiver<()>) {
    tokio::time::timeout(Duration::from_secs(12),Box::pin(async move{
        let mut peer=CommonPeer::open(listener,server).await;let p=peer.next().await.unwrap();let login=decode_request(&p[1..]).unwrap();assert_eq!(login.method,rpc_login::LOGIN_SESSION_METHOD.as_bytes());assert_eq!(login.metadata.iter().find(|v|v.key==b"id").unwrap().value,b"AccountEntry");
        let mut w=Writer::new();w.boolean(true).unwrap();w.bytes(b"fixture-user").unwrap();w.bytes(b"fixture-auth").unwrap();w.properties(&[]).unwrap();w.bytes(b"").unwrap();peer.send(&[reply(login.call_id,w.finish().expose_to_transport())]).await;
        let mut known=SdkWrites::default();
        loop{let p=peer.next().await.unwrap();if swap(&mut peer,&p,Some(b"4"),&mut known).await{break;}}
        let request=loop{
            let p=peer.next().await.unwrap();let request=decode_request(&p[1..]).unwrap();if request.method==rpc_batch::SWAP_CALLS_METHOD.as_bytes(){assert!(!swap(&mut peer,&p,None,&mut known).await);continue;}
            assert_eq!(p[0],0);assert_eq!(request.method,b"setRelations.GroupServer.Group");assert_eq!(request.metadata.len(),2);assert_eq!(request.metadata.iter().find(|v|v.key==b"id").unwrap().value,b"#Group");assert_eq!(request.metadata.iter().find(|v|v.key==b"token").unwrap().value,b"fixture-auth");
            let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"private-http-label");assert_eq!(r.u32().unwrap(),1,"MTC258 maps to native1, not a role grant");assert_eq!(r.bytes().unwrap(),b"private-http-note");assert_eq!(r.bytes().unwrap(),b"");assert!(r.properties().unwrap().is_empty());assert_eq!(r.u32().unwrap(),0);assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"private-http-removal");assert!(!r.boolean().unwrap());r.finish().unwrap();break request.call_id;
        };
        let stored:(i64,Option<i32>)=database.with_conn(|c|c.query_row("SELECT state,request_id FROM app_voice_relation_writes WHERE line_id=?1",[&line],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|"fixture HTTP intent")).unwrap();assert_eq!(stored,(0,Some(request)));
        let mut w=Writer::new();w.boolean(true).unwrap();w.i64(i64::MIN).unwrap();w.i64(i64::MAX).unwrap();w.bytes(b"").unwrap();peer.send(&[reply(request,w.finish().expose_to_transport())]).await;
        loop{tokio::select!{biased;value=&mut finish=>{value.unwrap();break;},p=peer.next()=>{let p=p.unwrap();assert!(!swap(&mut peer,&p,None,&mut known).await,"no implicit business action or history resend");}}}
        loop{let p=peer.next().await.unwrap();if swap(&mut peer,&p,Some(b"3"),&mut known).await{break;}}
        loop{let p=peer.next().await.unwrap();let r=decode_request(&p[1..]).unwrap();if r.method==rpc_logout::LOGOUT_SESSION_METHOD.as_bytes(){peer.send(&[reply(r.call_id,&[0,1,0,0,0,0])]).await;break;}assert!(!swap(&mut peer,&p,None,&mut known).await);}
        while let Some(p)=peer.next_input_only().await{assert!(matches!(p.as_slice(),[2]|[14]|[4]),"no relation retry or late RPC on original EOF");}
    })).await.expect("original bounded HTTP fixture peer; no timeout extension or detached peer");
}
