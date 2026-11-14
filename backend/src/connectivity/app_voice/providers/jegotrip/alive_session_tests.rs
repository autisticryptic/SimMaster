//! Actions-only original auth/agent/pending/clock boundaries for Alive.
use super::*;
#[test]
fn alive_config_interval_is_bounded_separately_from_call_timer_and_defaults_to180(){
    let value:AccountAliveConfig=serde_json::from_str(r#"{"allow_legacy_stcp":false,"endpoints":[{"host":"127.0.0.1","port":8888}]}"#).unwrap();assert_eq!(value.interval_seconds,180);
    for interval in [0,60,180,600]{let mut value=policy_config(8888,true);value.interval_seconds=interval;assert!(AlivePolicy::capture(&value).is_ok());}
    for interval in [1,59,601,u32::MAX]{let mut value=policy_config(8888,true);value.interval_seconds=interval;assert!(AlivePolicy::capture(&value).is_err());}
}
#[test]
fn alive_legacy_permission_cannot_downgrade_tls_or_infer_an_unapproved_destination(){
    let denied=AlivePolicy::capture(&policy_config(8888,false)).unwrap();assert!(denied.select(b"AccountAlive:stcp -h 127.0.0.1 -p 8888;").is_err());
    let allowed=AlivePolicy::capture(&policy_config(8888,true)).unwrap();assert!(allowed.select(b"AccountAlive:stcp -h 127.0.0.1 -p 8888;").is_ok());
    for reference in [b"AccountAlive:tls -h 127.0.0.1 -p 8888;".as_slice(),b"AccountAlive:tcp -h 127.0.0.1 -p 8888;",b"AccountAlive:stcp -h 127.0.0.1 -p 8889;",b"Other:stcp -h 127.0.0.1 -p 8888;",b"AccountAlive@adapter",b"AccountAlive:stcp -h 127.0.0.1 -p 8888;stcp -h 127.0.0.1 -p 8888;",b"AccountAlive:stcp -h 127.0.0.1 -p 8888 -S 0;"]{assert!(allowed.select(reference).is_err());}
}
#[test]
fn alive_configuration_rejects_duplicate_peers_and_invalid_endpoint_fields(){
    let mut config=policy_config(8888,true);config.endpoints.push(config.endpoints[0].clone());assert!(AlivePolicy::capture(&config).is_err());
    config.endpoints.pop();config.endpoints[0].port=0;assert!(AlivePolicy::capture(&config).is_err());config.endpoints[0].port=8888;
    config.endpoints[0].host=["https:","//not-a-host/"].concat();assert!(AlivePolicy::capture(&config).is_err());
}
#[test]
fn alive_link_requires_actual_sdk_authentication_and_original_owner_scope_before_claim(){
    let original=owner();let rpc=ArcRpcSession::new(original.clone());let scope=rpc.connection_scope();let link=rpc.alive_link();assert!(link.claim(&original,&scope,60).is_err());
    let(_rpc,original,scope,link)=authenticated();assert!(link.claim(&owner(),&scope,60).is_err());assert!(link.claim(&original,&RpcConnectionScope::fresh(),60).is_err());
    link.claim(&original,&scope,60).unwrap();assert!(link.claim(&original,&scope,60).is_err());
}
#[test]
fn decoded_positive_alive_bit_or_main_sdk_login_never_registers_the_independent_agent(){
    let(_rpc,original,scope,link)=authenticated();let decoded=n::rpc_alive::decode_register_alive_result(0,&[0,1,0,0,0,0]).unwrap();assert!(decoded.returned_acceptance());
    assert_eq!(link.check(&original,&scope,b"fixture-alive-token",Instant::now()),(false,0));link.claim(&original,&scope,60).unwrap();let _agent=link.agent().unwrap();
    assert_eq!(link.check(&original,&scope,b"fixture-alive-token",Instant::now()),(false,0));
}
#[test]
fn alive_request_uses_actual_uid_selector_token_main_account_and_full_wall_timestamp(){
    let(_rpc,owner,scope,link)=authenticated();link.claim(&owner,&scope,60).unwrap();let mut agent=link.agent().unwrap();let now=Instant::now();let bytes=agent.prepare(u64::MAX,now,now+Duration::from_secs(2)).unwrap();
    let request=decode_request(bytes.expose_to_transport()).unwrap();assert_eq!(request.call_id,1);assert_eq!(request.method,n::rpc_alive::REGISTER_ALIVE_METHOD.as_bytes());
    assert_eq!(request.metadata.iter().map(|p|p.key).collect::<Vec<_>>(),vec![b"id".as_slice(),b"token",b"mainAccount"]);
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-alive-token");assert_eq!(request.metadata.iter().find(|p|p.key==b"mainAccount").unwrap().value,b"fixture-main-account");
    assert!(!request.metadata.iter().any(|p|p.key==b"pwd"||p.key==b"domain"));let args=n::rpc_alive::decode_register_alive_arguments(request.arguments).unwrap();
    assert_eq!(args.user_id,b"fixture-alive-user");assert_eq!(args.selected_session_id,b"fixture-alive-selector");assert_eq!(args.request_timestamp_ms,u64::MAX);assert_eq!(args.interval_seconds,60);
    assert!(agent.prepare(1,now,now+Duration::from_secs(3)).is_err());
}
#[test]
fn mismatched_alive_rpc_id_does_not_clear_pending_or_register_then_duplicate_is_ignored(){
    let(_rpc,owner,scope,link)=authenticated();link.claim(&owner,&scope,60).unwrap();let mut agent=link.agent().unwrap();let now=Instant::now();agent.prepare(1,now,now+Duration::from_secs(2)).unwrap();
    assert!(agent.response(&alive_reply(2,true,b""),now).unwrap().is_none());assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",now),(false,0));
    assert!(matches!(agent.response(&alive_reply(1,true,b""),now).unwrap(),Some(AliveResult::Accepted)));assert!(agent.response(&alive_reply(1,true,b""),now).unwrap().is_none());
}
#[test]
fn matched_registration_check_alive_returns_interval_and_refreshes_only_current_activity(){
    let(_rpc,owner,scope,link,_agent,now)=registered();assert_eq!(link.check(&owner,&scope,b"fixture-phone-token",now),(false,0));
    assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",now+Duration::from_secs(119)),(true,60));
    assert_eq!(link.due(now).unwrap(),now+Duration::from_secs(239));
    assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",now+Duration::from_secs(239)),(false,0));
}
#[test]
fn main_client_ex_uses_same_independent_registration_without_fabricating_a_poll_hint(){
    let(rpc,owner,scope,link,_agent,now)=registered();let dispatcher=rpc.native_client_ex_dispatcher(receiver()).unwrap();let bytes=check_request(7);let request=decode_request(&bytes).unwrap();
    let action=dispatcher.on_direct(&owner,&scope,&request).unwrap().unwrap();assert!(action.notice.is_none());let mut r=Reader::new(action.result.expose_to_transport()).unwrap();assert!(r.boolean().unwrap());assert_eq!(r.u32().unwrap(),60);
    assert!(dispatcher.on_direct(&super::owner(),&scope,&request).unwrap().is_none());link.close();assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",now),(false,0));
    assert_eq!(dispatcher.on_direct(&owner,&scope,&request).unwrap().unwrap().result.expose_to_transport(),[0;10]);
}
#[test]
fn independent_check_callback_is_false_before_registration_and_does_not_reset_deadline(){
    let(_rpc,owner,scope,link)=authenticated();link.claim(&owner,&scope,60).unwrap();let mut agent=link.agent().unwrap();let now=Instant::now();let end=now+Duration::from_secs(2);agent.prepare(1,now,end).unwrap();
    let result=agent.callback(&check_request(1),now,&receiver()).unwrap();let reply=decode_reply(result.expose_to_transport()).unwrap();assert_eq!(reply.result_stream,[0;10]);assert_eq!(agent.pending_deadline(),Some(end));
}
#[test]
fn duplicate_independent_check_replies_do_not_refresh_activity_or_accept_changed_payload(){
    let(_rpc,owner,scope,link,mut agent,now)=registered();let first=agent.callback(&check_request(7),now,&receiver()).unwrap();
    let again=agent.callback(&check_request(7),now+Duration::from_secs(90),&receiver()).unwrap();assert_eq!(first.expose_to_transport(),again.expose_to_transport());assert_eq!(link.due(now).unwrap(),now+Duration::from_secs(120));
    let args=Writer::versioned().unwrap().finish();let changed=encode_request(7,&[RpcProperty{key:b"id",value:b"MessageReceiver"},RpcProperty{key:b"domain",value:b"changed"}],CHECK_ALIVE_METHOD.as_bytes(),args.expose_to_transport()).unwrap();
    assert!(agent.callback(changed.expose_to_transport(),now,&receiver()).is_err());assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",now+Duration::from_secs(120)),(false,0));
}
#[test]
fn wrong_independent_callback_object_kind_or_body_is_rejected_before_status_changes(){
    let(_rpc,_,_,link,mut agent,now)=registered();let args=Writer::versioned().unwrap().finish();
    for (object,method,body) in [(b"Wrong".as_slice(),CHECK_ALIVE_METHOD.as_bytes(),args.expose_to_transport()),(b"MessageReceiver",b"other",args.expose_to_transport()),(b"MessageReceiver",CHECK_ALIVE_METHOD.as_bytes(),b"bad")]{
        let bytes=encode_request(7,&[RpcProperty{key:b"id",value:object}],method,body).unwrap();assert!(agent.callback(bytes.expose_to_transport(),now,&receiver()).is_err());}
    assert_eq!(link.due(now).unwrap(),now+Duration::from_secs(120));
}
#[test]
fn expired_or_foreign_agent_response_cannot_revive_registration_after_replacement(){
    let(_rpc,owner,scope,link,mut old,now)=registered();old.scope().close();let mut new=link.agent().unwrap();new.prepare(1,now,now+Duration::from_secs(2)).unwrap();
    assert!(old.response(&alive_reply(1,true,b""),now).is_err());drop(old);assert!(matches!(new.response(&alive_reply(1,true,b""),now).unwrap(),Some(AliveResult::Accepted)));
    assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",now),(true,60));
}
#[test]
fn alive_token_error_revokes_only_its_original_main_session_and_never_steals_another(){
    let(_rpc,owner,scope,link)=authenticated();let(_foreign,other,other_scope,other_link)=authenticated();
    link.claim(&owner,&scope,60).unwrap();let mut agent=link.agent().unwrap();let now=Instant::now();agent.prepare(1,now,now+Duration::from_secs(2)).unwrap();
    assert!(matches!(agent.response(&alive_reply(1,false,b"token-error:another-device-logined"),now).unwrap(),Some(AliveResult::SessionInvalid)));
    assert!(scope.session_revoked());assert!(!other_scope.session_revoked());assert_eq!(other_link.check(&other,&other_scope,b"fixture-alive-token",now),(false,0));assert!(link.agent().is_err());
}
#[test]
fn negative_alive_result_keeps_main_open_and_uses_original_pending_then_retry_id(){
    let(_rpc,owner,scope,link)=authenticated();link.claim(&owner,&scope,60).unwrap();let mut agent=link.agent().unwrap();let now=Instant::now();agent.prepare(1,now,now+Duration::from_secs(2)).unwrap();
    assert!(matches!(agent.response(&alive_reply(1,false,b""),now).unwrap(),Some(AliveResult::Rejected)));assert!(scope.is_open());assert!(!scope.session_revoked());
    let request=agent.prepare(2,now,now+Duration::from_secs(2)).unwrap();assert_eq!(decode_request(request.expose_to_transport()).unwrap().call_id,2);
}
#[test]
fn alive_stop_closes_original_agent_and_late_true_cannot_restore_check_alive(){
    let(_rpc,owner,scope,link,mut agent,now)=registered();let agent_scope=agent.scope();link.close();assert!(!agent_scope.is_open());assert!(scope.is_open());
    assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",now),(false,0));assert!(agent.response(&alive_reply(1,true,b""),now).is_err());assert!(link.agent().is_err());
}
#[test]
fn alive_rpc_expiry_does_not_extend_on_an_unmatched_or_partial_result(){
    let(_rpc,owner,scope,link)=authenticated();link.claim(&owner,&scope,60).unwrap();let mut agent=link.agent().unwrap();let now=Instant::now();let end=now+Duration::from_secs(2);agent.prepare(1,now,end).unwrap();
    assert!(agent.response(&alive_reply(99,true,b""),end).unwrap().is_none());assert_eq!(agent.pending_deadline(),Some(end));
    assert!(agent.response(&alive_reply(1,true,b""),end).is_err());assert_eq!(link.check(&owner,&scope,b"fixture-alive-token",end),(false,0));
}
#[test]
fn alive_backoff_uses_six_to_sixhundred_seconds_without_moving_original_deadlines(){
    let mut delay=AliveBackoff::new();let mut now=Instant::now();for expected in [6,12,24,48,96,192,384,600,600]{let next=delay.fail(now).unwrap();assert_eq!(next.duration_since(now),Duration::from_secs(expected));now=next;}
    delay.reset();assert_eq!(delay.fail(now).unwrap().duration_since(now),Duration::from_secs(6));
}
