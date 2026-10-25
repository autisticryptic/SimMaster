//! Actions-only original CallClientEx receiver/receipt integration. Real memory
//! TLS/Common peers; no SDK target, physical device/service or media-readiness.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as native;
use native::{call_binding::CallIncomingBinding,rpc_calls::{CallNoticeKind,NativeCallId,encode_notice_result}};

const CALL_ID:u64=0x0123_4567_89ab_cdef;
fn call_fixture(cert:&Certificates,enabled:bool)->(Arc<NativeHostingBackend>,mpsc::Receiver<BackendEvent>,Peer,Arc<AtomicBool>){
    let(unused,old_events,connector,peer,dropped,_)=fixture(cert,false);drop(unused);drop(old_events);
    let profile=if enabled{profile().with_call_receiver(Some(CallIncomingBinding{object_id:b"#fixture-call-client",domain:b"fixture-domain"})).unwrap()}else{profile()};
    let(backend,events)=NativeHostingBackend::new(profile,connector);(backend,events,peer,dropped)
}
fn args(kind:CallNoticeKind)->Vec<u8>{
    let mut writer=Writer::versioned().unwrap();writer.u64(CALL_ID).unwrap();if kind==CallNoticeKind::Invited{writer.bytes(&[0xff,0,7]).unwrap();}
    writer.properties(&[RpcProperty{key:b"callServerEx.oid",value:b"#fixture-call-server"},RpcProperty{key:b"RseEx.SDP",value:b"private-opaque-data-not-media"}]).unwrap();writer.finish().expose_to_transport().to_vec()
}
fn method(kind:CallNoticeKind)->&'static str{match kind{CallNoticeKind::Invited=>native::rpc_calls::INVITED_METHOD,CallNoticeKind::Released=>native::rpc_calls::RELEASED_METHOD,CallNoticeKind::Accepted=>native::rpc_calls::ACCEPTED_METHOD}}
async fn send_direct(peer:&mut Peer,kind:CallNoticeKind,id:i32,domain:&[u8]){
    let payload=args(kind);let magic=id.to_be_bytes();
    let request=native::rpc_envelope::encode_request(id,&[RpcProperty{key:b"id",value:b"#fixture-call-client"},RpcProperty{key:b"domain",value:domain},RpcProperty{key:b"__magic",value:&magic}],method(kind).as_bytes(),&payload).unwrap();
    peer.send_call(CallKind::Request,request.expose_to_transport()).await;
}

#[tokio::test]
async fn direct_invited_accepted_released_keep_original_ticket_and_exact_distinct_reply_shapes(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=call_fixture(&cert,true);
    let(go,resume)=oneshot::channel();let(checked,checked_rx)=oneshot::channel();
    let serving=tokio::spawn(async move{
        peer.authenticate().await;resume.await.unwrap();let mut checked=Some(checked);
        for(kind,id)in [(CallNoticeKind::Invited,1),(CallNoticeKind::Accepted,2),(CallNoticeKind::Released,3)]{
            send_direct(&mut peer,kind,id,b"fixture-domain").await;
            if id==1{assert!(tokio::time::timeout(Duration::from_millis(40),next_non_poll(&mut peer)).await.is_err());checked.take().unwrap().send(()).unwrap();}
            let reply=next_non_poll(&mut peer).await;assert_eq!(reply.kind,CallKind::Reply);
            let result=native::rpc_envelope::decode_reply(reply.payload.expose_to_transport()).unwrap();assert_eq!(result.call_id,id);
            assert_eq!(result.result_stream,encode_notice_result(kind,true,&[]).unwrap().expose_to_transport());
        }
        peer.drain_until_eof().await;
    });
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;go.send(()).unwrap();
    let mut checked_rx=Some(checked_rx);
    for kind in [CallNoticeKind::Invited,CallNoticeKind::Accepted,CallNoticeKind::Released]{
        let event=tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap();assert!(event.matches(&original,&scope));
        let BackendEventPayload::Call(call)=event.payload else{panic!("original call delivery expected")};
        assert_eq!(call.notice().kind(),kind);assert_eq!(call.notice().native_id(),NativeCallId::from_bits(CALL_ID));
        assert!(call.ticket().matches(&original,&scope));let data=call.notice().data_for(&original,&scope).unwrap();
        assert!(data.properties.iter().any(|p|p.key==b"RseEx.SDP"&&p.value==b"private-opaque-data-not-media"));
        if kind==CallNoticeKind::Invited{
            checked_rx.take().unwrap().await.unwrap();
            assert_eq!(backend.complete_call(&owner(&cert.root),&scope,call.ticket(),true,&[]).await.err(),Some(RuntimeError::Scope));
            assert_eq!(backend.complete_call(&original,&RpcConnectionScope::fresh(),call.ticket(),true,&[]).await.err(),Some(RuntimeError::Scope));
            assert_eq!(backend.complete_call(&original,&scope,call.ticket(),true,&[RpcProperty{key:b"not-invited-result",value:b"must-retain-original"}]).await.err(),Some(RuntimeError::Protocol));
            assert!(scope.is_open());assert!(call.ticket().is_open());
        }
        assert!(backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.unwrap().matches(&original,&scope));
        assert_eq!(backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.err(),Some(RuntimeError::Protocol));assert!(scope.is_open());
    }
    backend.retire(&original);backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}

#[tokio::test]
async fn batch_call_completion_is_original_queued_result_not_a_direct_or_sms_ack(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=call_fixture(&cert,true);
    let(no_result,checked)=oneshot::channel();let(seen_result,result_seen)=oneshot::channel();
    let serving=tokio::spawn(async move{
        peer.authenticate().await;let call=peer.receive().await.unwrap();let request=decode_request(call.payload.expose_to_transport()).unwrap();
        let mut reader=Reader::versioned(request.arguments).unwrap();reader.bytes().unwrap();reader.bytes().unwrap();let payload=native::rpc_batch::read_payload(&mut reader).unwrap();
        let mut result=Writer::new();result.boolean(true).unwrap();for word in [9,1,1,payload.fields[0],payload.fields[1],payload.fields[2]]{result.u32(word).unwrap();}
        result.u32(1).unwrap();result.i32(1).unwrap();result.bytes(method(CallNoticeKind::Accepted).as_bytes()).unwrap();result.bytes(&args(CallNoticeKind::Accepted)).unwrap();
        result.properties(&[RpcProperty{key:b"id",value:b"#fixture-call-client"},RpcProperty{key:b"domain",value:b"fixture-domain"},RpcProperty{key:b"__magic",value:&[0,0xff,2]}]).unwrap();
        result.u32(0).unwrap();result.properties(&[]).unwrap();result.bytes(b"").unwrap();peer.reply(request.call_id,result.finish().expose_to_transport()).await;
        let next=peer.receive().await.unwrap();let request=decode_request(next.payload.expose_to_transport()).unwrap();let mut reader=Reader::versioned(request.arguments).unwrap();reader.bytes().unwrap();reader.bytes().unwrap();let pending=native::rpc_batch::read_payload(&mut reader).unwrap();
        assert!(!pending.results.contains_key(&1));peer.reply(request.call_id,&swap_result(None,true,pending.fields)).await;no_result.send(()).unwrap();
        loop{
            let call=peer.receive().await.unwrap();let request=decode_request(call.payload.expose_to_transport()).unwrap();let mut reader=Reader::versioned(request.arguments).unwrap();reader.bytes().unwrap();reader.bytes().unwrap();let pending=native::rpc_batch::read_payload(&mut reader).unwrap();
            let completed=pending.results.get(&1);
            if let Some(value)=completed{assert_eq!(value.result_stream,encode_notice_result(CallNoticeKind::Accepted,true,&[]).unwrap().expose_to_transport());}
            peer.reply(request.call_id,&swap_result(None,true,pending.fields)).await;if completed.is_some(){break;}
        }
        seen_result.send(()).unwrap();peer.drain_until_eof().await;
    });
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;
    let event=tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap();let BackendEventPayload::Call(call)=event.payload else{panic!("original batch call")};
    checked.await.unwrap();assert!(call.ticket().matches(&original,&scope));
    assert_eq!(backend.complete_call(&owner(&cert.root),&scope,call.ticket(),true,&[]).await.err(),Some(RuntimeError::Scope));
    assert!(backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.unwrap().matches(&original,&scope));
    result_seen.await.unwrap();backend.retire(&original);backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}

#[tokio::test]
async fn uninstalled_call_receiver_never_fabricates_call_channel_or_accepts_call_arguments(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=call_fixture(&cert,false);
    let(go,resume)=oneshot::channel();let serving=tokio::spawn(async move{peer.authenticate().await;resume.await.unwrap();send_direct(&mut peer,CallNoticeKind::Invited,1,b"fixture-domain").await;peer.drain_until_eof().await;});
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;go.send(()).unwrap();
    let event=tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap();assert!(matches!(event.payload,BackendEventPayload::Terminal(_)));assert!(!scope.is_open());
    backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}

#[tokio::test]
async fn wrong_call_receiver_domain_fails_before_owned_event_and_cannot_rebind_current_account(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=call_fixture(&cert,true);
    let(go,resume)=oneshot::channel();let serving=tokio::spawn(async move{peer.authenticate().await;resume.await.unwrap();send_direct(&mut peer,CallNoticeKind::Invited,1,b"foreign-domain").await;peer.drain_until_eof().await;});
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;go.send(()).unwrap();
    let event=tokio::time::timeout(Duration::from_secs(2),events.recv()).await.unwrap().unwrap();assert!(matches!(event.payload,BackendEventPayload::Terminal(_)));assert!(!scope.is_open());
    backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}

#[tokio::test]
async fn retired_call_reply_capability_never_acks_a_new_owner_or_scope(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(backend,mut events,mut peer,dropped)=call_fixture(&cert,true);
    let(go,resume)=oneshot::channel();let serving=tokio::spawn(async move{peer.authenticate().await;resume.await.unwrap();send_direct(&mut peer,CallNoticeKind::Invited,1,b"fixture-domain").await;peer.drain_until_eof().await;});
    let original=owner(&cert.root);let scope=logged_in(&backend,&mut events,&original,&cert.root).await;go.send(()).unwrap();
    let event=events.recv().await.unwrap();let BackendEventPayload::Call(call)=event.payload else{panic!("call ticket")};
    backend.retire(&original);assert!(!call.ticket().is_open());assert!(call.notice().data_for(&original,&scope).is_none());
    assert_eq!(backend.complete_call(&original,&scope,call.ticket(),true,&[]).await.err(),Some(RuntimeError::Scope));
    backend.logout(original).await.unwrap();assert!(dropped.load(Ordering::SeqCst));serving.await.unwrap();
}

#[test]
fn receiver_builder_rejects_ambiguous_wildcards_or_auth_substitutes_without_mutating_original_profile(){
    for value in [b"".as_slice(),b"*",b"ENC(unresolved)",b"has\0control"]{
        assert!(profile().with_call_receiver(Some(CallIncomingBinding{object_id:value,domain:b"fixture-domain"})).is_err());
        assert!(profile().with_call_receiver(Some(CallIncomingBinding{object_id:b"#fixture-call-client",domain:value})).is_err());
    }
    let profile=profile().with_call_receiver(Some(CallIncomingBinding{object_id:b"#fixture-call-client",domain:b"fixture-domain"})).unwrap();
    assert!(!format!("{profile:?}").contains("fixture"));assert!(profile.with_call_receiver(Some(CallIncomingBinding{object_id:b"#another",domain:b"fixture-domain"})).is_err());
}
