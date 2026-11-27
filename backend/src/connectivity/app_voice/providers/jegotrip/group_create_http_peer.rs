//! Compiled only by the original Actions test target. A loopback peer accepts
//! one create and (only after its SQL commit) one separate properties request.
use super::*;
const GROUP:&[u8]=b"811111111-922222222";
async fn group_request(peer:&mut CommonPeer,known:&mut SdkWrites,method:&[u8])->Vec<u8>{
    loop{let raw=peer.next().await.unwrap();assert_eq!(raw[0],0);let request=decode_request(&raw[1..]).unwrap();
        if request.method==rpc_batch::SWAP_CALLS_METHOD.as_bytes(){assert!(!swap(peer,&raw,None,known).await);continue;}
        assert_eq!(request.method,method);assert_eq!(request.metadata.len(),2);assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");return raw;
    }
}
pub(crate) async fn serve_group_create(listener:TcpListener,server:Arc<ServerConfig>,db:Arc<Database>,line:String,mut finish:oneshot::Receiver<()>){
    tokio::time::timeout(Duration::from_secs(12),Box::pin(async move{
        let mut peer=CommonPeer::open(listener,server).await;let raw=peer.next().await.unwrap();let req=decode_request(&raw[1..]).unwrap();assert_eq!(req.method,rpc_login::LOGIN_SESSION_METHOD.as_bytes());
        let mut login=Writer::new();login.boolean(true).unwrap();login.bytes(b"fixture-user").unwrap();login.bytes(b"fixture-auth").unwrap();login.properties(&[]).unwrap();login.bytes(b"").unwrap();peer.send(&[reply(req.call_id,login.finish().expose_to_transport())]).await;
        let mut known=SdkWrites::default();loop{let raw=peer.next().await.unwrap();if swap(&mut peer,&raw,Some(b"4"),&mut known).await{break;}}
        let raw=group_request(&mut peer,&mut known,b"createOrganize.GroupServer.Group").await;let req=decode_request(&raw[1..]).unwrap();let id=req.call_id;let mut a=Reader::versioned(req.arguments).unwrap();assert_eq!(a.u32().unwrap(),1);assert_eq!(a.bytes().unwrap(),b"fixture-user");assert_eq!(a.u32().unwrap(),0);assert_eq!(a.bytes().unwrap(),b"explicit-first");assert_eq!(a.bytes().unwrap(),b"explicit-second");assert!(a.properties().unwrap().is_empty());let props=a.properties().unwrap();assert_eq!(props.len(),2);assert!(props.iter().any(|p|p.key==b"Group.Org.Name"&&p.value==b"private-http-create"));assert!(props.iter().any(|p|p.key==b"Group.Org.Type"&&p.value==b"Group"));a.finish().unwrap();
        let state=||db.with_conn(|c|c.query_row("SELECT w.state,d.group_id,d.initial_members FROM app_voice_relation_writes w JOIN app_voice_group_creations d ON d.write_id=w.id WHERE w.line_id=?1 AND w.request_id=?2",rusqlite::params![line,id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Option<Vec<u8>>>(1)?,r.get::<_,i64>(2)?))).map_err(|_|"fixture create row")).unwrap();assert_eq!(state(),(0,None,1));
        let mut result=Writer::new();result.boolean(true).unwrap();result.bytes(GROUP).unwrap();result.u32(1).unwrap();result.bytes(b"single-relation").unwrap();result.bytes(b"").unwrap();result.properties(&[]).unwrap();result.i64(i64::MIN).unwrap();result.i64(i64::MAX).unwrap();result.bytes(b"").unwrap();peer.send(&[reply(id,result.finish().expose_to_transport())]).await;
        let raw=group_request(&mut peer,&mut known,b"getPropsPrefixs.GroupServer.Group").await;let req=decode_request(&raw[1..]).unwrap();assert_ne!(req.call_id,id);let mut a=Reader::versioned(req.arguments).unwrap();assert_eq!(a.bytes().unwrap(),GROUP);assert_eq!(a.u32().unwrap(),1);assert_eq!(a.bytes().unwrap(),b"");a.finish().unwrap();assert_eq!(state(),(1,Some(GROUP.to_vec()),1));
        let mut read=Writer::new();read.boolean(true).unwrap();read.properties(&[super::super::rpc_stream::RpcProperty{key:b"private-prop",value:b"private-value"}]).unwrap();read.bytes(b"").unwrap();peer.send(&[reply(req.call_id,read.finish().expose_to_transport())]).await;
        loop{tokio::select!{biased;value=&mut finish=>{value.unwrap();break;},raw=peer.next()=>{assert!(!swap(&mut peer,&raw.unwrap(),None,&mut known).await);}}}
        loop{let raw=peer.next().await.unwrap();if swap(&mut peer,&raw,Some(b"3"),&mut known).await{break;}}
        loop{let raw=peer.next().await.unwrap();let req=decode_request(&raw[1..]).unwrap();if req.method==rpc_logout::LOGOUT_SESSION_METHOD.as_bytes(){peer.send(&[reply(req.call_id,&[0,1,0,0,0,0])]).await;break;}assert!(!swap(&mut peer,&raw,None,&mut known).await);}
        while let Some(raw)=peer.next_input_only().await{assert!(matches!(raw.as_slice(),[2]|[14]|[4]));}
    })).await.expect("original group-create HTTP peer deadline");
}
