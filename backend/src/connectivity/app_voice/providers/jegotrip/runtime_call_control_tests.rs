//! Actions-only real TLS/Common original incoming-call control exchanges.
//! Semantic acceptance is tested separately from actual media negotiation.
use super::*;
use native::{call_control::CallControlTicket,rpc_calls::CallOperation};
fn controlled_fixture(cert:&Certificates)->(Arc<NativeHostingBackend>,mpsc::Receiver<BackendEvent>,Peer,Arc<AtomicBool>){
    let(unused,old,connector,peer,dropped,_)=fixture(cert,false);drop(unused);drop(old);
    let profile=profile().with_call_receiver(Some(CallIncomingBinding{object_id:b"#fixture-call-client",domain:b"fixture-domain"})).unwrap()
        .with_call_control(&[RpcProperty{key:b"id",value:b"#fixture-base-server"},RpcProperty{key:b"domain",value:b"fixture-domain"}]).unwrap();
    let(backend,events)=NativeHostingBackend::new(profile,connector);(backend,events,peer,dropped)
}
async fn receive_operation(peer:&mut Peer,operation:CallOperation)->i32{
    let packet=next_non_poll(peer).await;let request=decode_request(packet.payload.expose_to_transport()).unwrap();assert_eq!(request.method,operation.method().as_bytes());
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");assert!(!request.metadata.iter().any(|p|p.key==b"pwd"));
    let expected=if operation==CallOperation::Inform{b"#fixture-base-server".as_slice()}else{b"#fixture-call-server".as_slice()};assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,expected);
    let mut reader=Reader::versioned(request.arguments).unwrap();assert_eq!(reader.u64().unwrap(),CALL_ID);reader.properties().unwrap();reader.finish().unwrap();request.call_id
}
async fn reply_operation(peer:&mut Peer,id:i32,operation:CallOperation,accepted:bool){
    let mut writer=Writer::new();writer.boolean(accepted).unwrap();if operation==CallOperation::Accept{writer.properties(&[RpcProperty{key:b"endpoint",value:b"data-not-media-active"}]).unwrap();}writer.bytes(b"").unwrap();peer.reply(id,writer.finish().expose_to_transport()).await;
}
async fn received(events:&mut mpsc::Receiver<BackendEvent>)->native::call_delivery::IncomingCallDelivery{
    let event=tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap();let BackendEventPayload::Call(call)=event.payload else{panic!("original CallEx capability")};call
}
async fn invoke(backend:&NativeHostingBackend,original:&AttemptOwner,scope:&RpcConnectionScope,ticket:&CallControlTicket,operation:CallOperation)->Result<native::call_control::OwnedCallOperationReply,RuntimeError>{
    backend.call(original,scope,ticket,operation,&[RpcProperty{key:b"RseEx.Info",value:b"fixture-operation"}]).await
}
#[tokio::test]
async fn original_inform_accept_release_wait_for_matched_semantics_and_do_not_alias_sdk_or_media(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=controlled_fixture(&cert);let(go,resume)=oneshot::channel();
    let serving=tokio::spawn(async move{
        peer.authenticate().await;resume.await.unwrap();send_direct(&mut peer,CallNoticeKind::Invited,1,b"fixture-domain").await;
        let receipt=next_non_poll(&mut peer).await;assert_eq!(receipt.kind,CallKind::Reply);
        let mut ids=Vec::new();for operation in [CallOperation::Inform,CallOperation::Accept,CallOperation::Release]{let id=receive_operation(&mut peer,operation).await;ids.push(id);reply_operation(&mut peer,id,operation,true).await;}
        assert!(ids.windows(2).all(|pair|pair[0]<pair[1]));peer.drain_until_eof().await;
    });
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;go.send(()).unwrap();let call=received(&mut events).await;
    let ticket=call.control_ticket().unwrap();assert!(ticket.matches(&original,&scope));
    assert_eq!(invoke(&backend,&original,&scope,ticket,CallOperation::Inform).await.err(),Some(RuntimeError::CallNotReady));assert!(scope.is_open());
    backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.unwrap();
    for operation in [CallOperation::Inform,CallOperation::Accept,CallOperation::Release]{let reply=invoke(&backend,&original,&scope,ticket,operation).await.unwrap();assert!(reply.matches(&original,&scope));assert!(reply.accepted());assert_eq!(reply.operation(),operation);}
    assert!(!ticket.is_live());backend.retire(&original);backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}
#[tokio::test]
async fn rejected_original_inform_keeps_sdk_open_but_cannot_answer_or_retry_that_call(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=controlled_fixture(&cert);let(go,resume)=oneshot::channel();
    let serving=tokio::spawn(async move{peer.authenticate().await;resume.await.unwrap();send_direct(&mut peer,CallNoticeKind::Invited,1,b"fixture-domain").await;assert_eq!(next_non_poll(&mut peer).await.kind,CallKind::Reply);let id=receive_operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,false).await;let id=receive_operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true).await;peer.drain_until_eof().await;});
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;go.send(()).unwrap();let call=received(&mut events).await;let ticket=call.control_ticket().unwrap();backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.unwrap();
    assert_eq!(invoke(&backend,&original,&scope,ticket,CallOperation::Inform).await.err(),Some(RuntimeError::CallRejected));assert!(scope.is_open());assert!(!ticket.is_live());
    assert_eq!(invoke(&backend,&original,&scope,ticket,CallOperation::Accept).await.err(),Some(RuntimeError::Scope));assert!(scope.is_open());invoke(&backend,&original,&scope,ticket,CallOperation::Release).await.unwrap();
    backend.retire(&original);backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}
#[tokio::test]
async fn remote_release_between_submit_and_true_result_revokes_original_call_and_never_answers_new_call(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=controlled_fixture(&cert);let(go,resume)=oneshot::channel();
    let serving=tokio::spawn(async move{peer.authenticate().await;resume.await.unwrap();send_direct(&mut peer,CallNoticeKind::Invited,1,b"fixture-domain").await;next_non_poll(&mut peer).await;let id=receive_operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true).await;let id=receive_operation(&mut peer,CallOperation::Accept).await;send_direct(&mut peer,CallNoticeKind::Released,2,b"fixture-domain").await;assert_eq!(next_non_poll(&mut peer).await.kind,CallKind::Reply);reply_operation(&mut peer,id,CallOperation::Accept,true).await;peer.drain_until_eof().await;});
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;go.send(()).unwrap();let call=received(&mut events).await;let ticket=call.control_ticket().unwrap();backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.unwrap();invoke(&backend,&original,&scope,ticket,CallOperation::Inform).await.unwrap();
    let answer=invoke(&backend,&original,&scope,ticket,CallOperation::Accept);tokio::pin!(answer);
    let released=tokio::select!{result=answer.as_mut()=>panic!("must observe original cancel first: {result:?}"),call=received(&mut events)=>call};
    assert_eq!(released.notice().kind(),CallNoticeKind::Released);assert!(!ticket.is_live());backend.complete_call(&original,&scope,released.ticket(),true,&[]).await.unwrap();
    assert_eq!(answer.await.err(),Some(RuntimeError::CallRejected));assert!(scope.is_open());backend.retire(&original);backend.logout(original.clone()).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}
#[tokio::test]
async fn cancelled_original_accept_wait_retires_only_original_scope_and_retains_join_cleanup(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=controlled_fixture(&cert);let(go,resume)=oneshot::channel();let(submitted,seen)=oneshot::channel();
    let serving=tokio::spawn(async move{peer.authenticate().await;resume.await.unwrap();send_direct(&mut peer,CallNoticeKind::Invited,1,b"fixture-domain").await;next_non_poll(&mut peer).await;let id=receive_operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true).await;receive_operation(&mut peer,CallOperation::Accept).await;submitted.send(()).unwrap();peer.drain_until_eof().await;});
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;go.send(()).unwrap();let call=received(&mut events).await;let ticket=call.control_ticket().unwrap();backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.unwrap();invoke(&backend,&original,&scope,ticket,CallOperation::Inform).await.unwrap();
    {let wait=invoke(&backend,&original,&scope,ticket,CallOperation::Accept);tokio::pin!(wait);tokio::select!{result=wait.as_mut()=>panic!("no semantic answer: {result:?}"),_=seen=>{}}}
    assert!(!scope.is_open());assert!(!ticket.is_live());backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}
#[tokio::test]
async fn batch_original_invitation_result_is_transmitted_before_inform_can_share_arc_allocator(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=controlled_fixture(&cert);
    let serving=tokio::spawn(async move{
        peer.authenticate().await;let packet=peer.receive().await.unwrap();let request=decode_request(packet.payload.expose_to_transport()).unwrap();
        let mut reader=Reader::versioned(request.arguments).unwrap();reader.bytes().unwrap();reader.bytes().unwrap();let payload=native::rpc_batch::read_payload(&mut reader).unwrap();
        let mut result=Writer::new();result.boolean(true).unwrap();for word in [9,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{result.u32(word).unwrap();}
        result.u32(1).unwrap();result.i32(1).unwrap();result.bytes(method(CallNoticeKind::Invited).as_bytes()).unwrap();result.bytes(&args(CallNoticeKind::Invited)).unwrap();
        result.properties(&[RpcProperty{key:b"id",value:b"#fixture-call-client"},RpcProperty{key:b"domain",value:b"fixture-domain"},RpcProperty{key:b"__magic",value:&[0,0xff,3]}]).unwrap();result.u32(0).unwrap();result.properties(&[]).unwrap();result.bytes(b"").unwrap();peer.reply(request.call_id,result.finish().expose_to_transport()).await;
        loop{
            let packet=peer.receive().await.unwrap();let request=decode_request(packet.payload.expose_to_transport()).unwrap();assert_eq!(request.method,native::rpc_batch::SWAP_CALLS_METHOD.as_bytes(),"Inform cannot overtake original callback receipt");
            let mut reader=Reader::versioned(request.arguments).unwrap();reader.bytes().unwrap();reader.bytes().unwrap();let payload=native::rpc_batch::read_payload(&mut reader).unwrap();let complete=payload.results.contains_key(&1);
            if complete{assert_eq!(payload.results[&1].result_stream,encode_notice_result(CallNoticeKind::Invited,true,&[]).unwrap().expose_to_transport());}
            peer.reply(request.call_id,&swap_result(None,true,payload.fields)).await;if complete{break;}
        }
        let id=receive_operation(&mut peer,CallOperation::Inform).await;reply_operation(&mut peer,id,CallOperation::Inform,true).await;
        let id=receive_operation(&mut peer,CallOperation::Release).await;reply_operation(&mut peer,id,CallOperation::Release,true).await;peer.drain_until_eof().await;
    });
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;let call=received(&mut events).await;let ticket=call.control_ticket().unwrap();
    backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2),async{loop{match invoke(&backend,&original,&scope,ticket,CallOperation::Inform).await{Err(RuntimeError::CallNotReady)=>tokio::time::sleep(Duration::from_millis(10)).await,Ok(reply)=>{assert!(reply.accepted());break;},Err(error)=>panic!("not retryable: {error:?}")}}}).await.unwrap();
    invoke(&backend,&original,&scope,ticket,CallOperation::Release).await.unwrap();backend.retire(&original);backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}
