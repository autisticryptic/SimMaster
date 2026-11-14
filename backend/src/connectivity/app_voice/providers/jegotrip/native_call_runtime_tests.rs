//! Actions-only full Router/TLS/Common wire for the explicit default call route.
use super::*;
use n::rpc_calls::CallOperation;
const NATIVE_ID:u64=0x1_2345_6789;
fn invited_args(override_id:bool)->Vec<u8>{let mut w=Writer::versioned().unwrap();w.u64(NATIVE_ID).unwrap();w.bytes(b"native-peer").unwrap();
    let props=if override_id{vec![RpcProperty{key:b"callServerEx.oid",value:b"#original-per-call"}]}else{Vec::new()};
    w.properties(&props).unwrap();w.finish().expose_to_transport().to_vec()}
fn operation_result(operation:CallOperation,accepted:bool)->Vec<u8>{let mut w=Writer::new();w.boolean(accepted).unwrap();
    if operation==CallOperation::Accept{w.properties(&[]).unwrap();}w.bytes(b"").unwrap();w.finish().expose_to_transport().to_vec()}
async fn serve_control(peer:&mut CommonPeer,operation:CallOperation,target:&[u8],accepted:bool){
    loop{let packet=peer.next().await.unwrap();assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();
        if request.method==n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(){
            let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");assert_eq!(r.bytes().unwrap(),b"fixture-selected-session");
            let payload=n::rpc_batch::read_payload(&mut r).unwrap();peer.send(&[reply(request.call_id,&n::runtime_backend::tests::swap_result(None,true,payload.fields))]).await;continue;
        }
        assert_eq!(request.method,operation.method().as_bytes());assert_eq!(request.metadata.len(),2);
        assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,target);
        assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");
        assert!(!request.metadata.iter().any(|p|p.key==b"domain"||p.key==b"pwd"));
        let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.u64().unwrap(),NATIVE_ID);let fields=r.properties().unwrap();r.finish().unwrap();
        if operation==CallOperation::Inform{assert_eq!(fields.len(),1);assert_eq!(fields[0].key,b"RseEx.Info");assert_eq!(fields[0].value,b"Invited callId is exist?");}
        else if operation==CallOperation::Accept{assert_eq!(fields.len(),1);assert_eq!(fields[0].key,b"RseEx.SDP");assert_eq!(fields[0].value,b"signal-data-only");}
        else{assert!(fields.is_empty());}
        peer.send(&[reply(request.call_id,&operation_result(operation,accepted))]).await;return;
    }
}
async fn native_call_roundtrip(override_id:bool,accepted:bool){
    let cert=Certificates::generate("IP:127.0.0.1");let listener=TcpListener::bind((std::net::Ipv4Addr::LOCALHOST,0)).await.unwrap();let port=listener.local_addr().unwrap().port();let server=cert.server_config(&TLS13);
    let(complete,confirmed)=tokio::sync::oneshot::channel();let serving=tokio::spawn(async move{
        tokio::time::timeout(Duration::from_secs(12),async move{
            let mut peer=CommonPeer::open(listener,server).await;let packet=peer.next().await.unwrap();let login=decode_request(&packet[1..]).unwrap();
            peer.send(&[reply(login.call_id,&login_result()),direct(91,n::rpc_calls::INVITED_METHOD.as_bytes(),&invited_args(override_id),b"CallClientEx",None)]).await;
            assert_eq!(next_reply(&mut peer,91).await,[0,1,0,0,0,0]);
            serve_control(&mut peer,CallOperation::Inform,b"#CallServerEx",accepted).await;
            if accepted{
                let target=if override_id{b"#original-per-call".as_slice()}else{b"#CallServerEx".as_slice()};
                serve_control(&mut peer,CallOperation::Accept,target,true).await;
                serve_control(&mut peer,CallOperation::Release,target,true).await;
            }
            complete.send(()).unwrap();
            while let Some(packet)=peer.next().await{assert_eq!(packet[0],0);let request=decode_request(&packet[1..]).unwrap();
                assert_eq!(request.method,n::rpc_batch::SWAP_CALLS_METHOD.as_bytes(),"no repeated Inform/Accept or automatic login");}
        }).await.unwrap()
    });
    let legacy=Arc::new(LegacyDenied{calls:AtomicUsize::new(0)});
    let profile=native_profile(&cert.root,port,&config()).with_native_call_control().unwrap();let(backend,mut events)=NativeHostingBackend::new(profile,legacy.clone());
    let original=owner();backend.login(original.clone(),request(&cert.root)).await.unwrap();
    assert!(matches!(event(&mut events).await.payload,BackendEventPayload::Callback(callback) if callback.callback==(Callback::SdkLogin{success:true})));
    let call=event(&mut events).await;let BackendEventPayload::Call(call)=call.payload else{panic!("native call")};let scope=backend.original_scope(&original).unwrap();let ticket=call.control_ticket().unwrap();
    assert_eq!(ticket.native_id().expose_to_backend(),NATIVE_ID);assert!(backend.original_call_receiver_ready(&original,&scope));
    assert!(matches!(backend.call(&original,&scope,ticket,CallOperation::Inform,&[]).await,Err(n::transport_actor::RuntimeError::CallNotReady)));
    backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.unwrap();
    let checked=backend.call(&original,&scope,ticket,CallOperation::Inform,&[]).await;
    if accepted{
        assert!(checked.unwrap().accepted());
        let result=backend.call(&original,&scope,ticket,CallOperation::Accept,&[RpcProperty{key:b"RseEx.SDP",value:b"signal-data-only"}]).await.unwrap();
        assert!(result.accepted());assert!(result.original_call(ticket));assert!(result.properties_for(&original,&scope).unwrap().is_empty(),"no media endpoint/trust is invented");
        assert!(backend.call(&original,&scope,ticket,CallOperation::Release,&[]).await.unwrap().accepted());
    }else{
        assert!(matches!(checked,Err(n::transport_actor::RuntimeError::CallRejected)));
        assert!(backend.call(&original,&scope,ticket,CallOperation::Accept,&[]).await.is_err());
        assert!(backend.call(&original,&scope,ticket,CallOperation::Inform,&[]).await.is_err());assert!(scope.is_open());
    }
    assert!(!ticket.is_live());confirmed.await.unwrap();backend.retire(&original);backend.logout(original).await.unwrap();serving.await.unwrap();
    assert!(!scope.is_open());assert_eq!(legacy.calls.load(Ordering::SeqCst),0);
}
#[tokio::test]
async fn native_default_common_call_wire_preserves_ack_existence_handoff_and_release_without_media_grant(){
    for override_id in [false,true]{Box::pin(native_call_roundtrip(override_id,true)).await;}
}
#[tokio::test]
async fn native_default_common_false_existence_keeps_session_but_cannot_accept_or_retry(){
    Box::pin(native_call_roundtrip(true,false)).await;
}
