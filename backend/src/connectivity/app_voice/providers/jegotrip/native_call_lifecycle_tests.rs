//! Actions-only native lifecycle policy/timing and shared-RPC correlation.
use super::*;
use n::native_call_lifecycle::{NativeCallLifecycleConfig,NativeCallPolicy,NativeCallTiming};
use std::time::{Duration,Instant};
fn prepared_timing(seconds:u32)->(ArcRpcSession,AttemptOwner,RpcConnectionScope,CallControlTicket,NativeCallTiming){
    let(mut rpc,owner,scope,d)=native_context();let ticket=native_ticket(&mut rpc,&owner,&scope,&d,Some(b"#per-call"));rpc.call_acknowledged(&ticket,true).unwrap();
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Inform,&[]).unwrap();finish(&mut rpc,&owner,&scope,id,CallOperation::Inform,true);
    let timing=NativeCallPolicy::capture(NativeCallLifecycleConfig{session_timer_seconds:seconds}).unwrap().bind(&ticket);(rpc,owner,scope,ticket,timing)
}
fn accepted_timing(seconds:u32)->(ArcRpcSession,AttemptOwner,RpcConnectionScope,CallControlTicket,NativeCallTiming,Instant){
    let(mut rpc,owner,scope,ticket,mut timing)=prepared_timing(seconds);let began=Instant::now();timing.begin_accept(&ticket,began).unwrap();
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Accept,&[]).unwrap();let result=finish(&mut rpc,&owner,&scope,id,CallOperation::Accept,true);
    timing.accepted(&ticket,&result,began+Duration::from_millis(10)).unwrap();(rpc,owner,scope,ticket,timing,began)
}
fn value<'a>(fields:&'a[(Vec<u8>,Vec<u8>)],key:&[u8])->Option<&'a[u8]>{fields.iter().find(|(k,_)|k==key).map(|(_,v)|v.as_slice())}
#[test]
fn native_call_timer_is_explicit_seconds_checked_for_signed_millisecond_overflow(){
    for seconds in [0,1,30,2147483]{assert!(NativeCallPolicy::capture(NativeCallLifecycleConfig{session_timer_seconds:seconds}).is_ok());}
    for seconds in [2147484,u32::MAX]{assert!(NativeCallPolicy::capture(NativeCallLifecycleConfig{session_timer_seconds:seconds}).is_err());}
    assert!(serde_json::from_str::<NativeCallLifecycleConfig>("{}").is_err());
    assert!(serde_json::from_str::<NativeCallLifecycleConfig>(r#"{"session_timer_seconds":30,"ready":true}"#).is_err());
}
#[test]
fn native_builtin_audio_accept_has_sdp_media_type_timer_and_new_stack_without_auth_material(){
    let(_rpc,_,_,ticket,timing)=prepared_timing(30);let fields=timing.accept_properties(&ticket,b"original-sdp").unwrap();
    assert_eq!(fields.len(),4);for (key,expected) in [(b"RseEx.SDP".as_slice(),b"original-sdp".as_slice()),(b"mediaType",b"audio"),(b"keepAlive",b"30000"),(b"SDK.IsNew",b"1")]{
        assert_eq!(fields.iter().find(|p|p.key==key).unwrap().value,expected);
    }
    assert!(!fields.iter().any(|p|matches!(p.key,b"token"|b"pwd"|b"RseEx.ClientHost"|b"RseEx.CryptoKey")));
    assert_eq!(format!("{timing:?}"),"NativeCallTiming(<redacted>)");
}
#[test]
fn native_call_timing_is_bound_to_the_original_ticket_not_just_its_numeric_call_id(){
    let(_rpc,_,_,ticket,mut timing)=prepared_timing(1);let(_foreign_rpc,_,_,foreign,_)=prepared_timing(1);let now=Instant::now();
    assert_eq!(ticket.native_id(),foreign.native_id());assert!(timing.begin_accept(&foreign,now).is_err());
    assert!(timing.accept_properties(&foreign,b"data").is_err());assert!(timing.next(&foreign,now).is_err());
    assert!(timing.begin_accept(&ticket,now).is_ok());ticket.connection_scope().close();assert!(timing.next(&ticket,now).is_err());
}
#[test]
fn native_call_timing_requires_one_matched_positive_accept_before_timer_or_connection(){
    let(mut rpc,owner,scope,ticket,mut timing)=prepared_timing(1);let now=Instant::now();
    assert!(timing.connected(&ticket,now).is_err());assert!(timing.take_due(&ticket,now).is_err());
    timing.begin_accept(&ticket,now).unwrap();assert!(timing.begin_accept(&ticket,now).is_err());assert!(timing.accept_properties(&ticket,b"retry").is_err());
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::Accept,&[]).unwrap();let result=finish(&mut rpc,&owner,&scope,id,CallOperation::Accept,true);
    timing.accepted(&ticket,&result,now+Duration::from_millis(10)).unwrap();assert!(timing.accepted(&ticket,&result,now+Duration::from_millis(20)).is_err());
}
#[test]
fn native_call_period_starts_at_accept_result_and_keeps_duration_as_one_shot_data(){
    let(_rpc,_,_,ticket,mut timing,start)=accepted_timing(1);
    assert!(timing.take_due(&ticket,start+Duration::from_millis(1009)).unwrap().is_none());
    let fields=timing.take_due(&ticket,start+Duration::from_millis(1010)).unwrap().unwrap();
    assert_eq!(value(&fields,b"keepAlive"),Some(b"1000".as_slice()));assert_eq!(value(&fields,b"RseEx.AcceptDuration"),Some(b"10".as_slice()));
    assert!(value(&fields,b"RseEx.ConnDuration").is_none());assert!(timing.take_due(&ticket,start+Duration::from_millis(1010)).unwrap().is_none());
    let next=timing.take_due(&ticket,start+Duration::from_millis(2010)).unwrap().unwrap();assert_eq!(next.len(),1);
}
#[test]
fn native_connected_report_is_immediate_without_resetting_the_original_periodic_deadline(){
    let(_rpc,_,_,ticket,mut timing,start)=accepted_timing(1);let connected=start+Duration::from_millis(210);timing.connected(&ticket,connected).unwrap();
    assert_eq!(timing.next(&ticket,connected).unwrap(),Some(connected));let fields=timing.take_due(&ticket,connected).unwrap().unwrap();
    assert_eq!(value(&fields,b"RseEx.AcceptDuration"),Some(b"10".as_slice()));assert_eq!(value(&fields,b"RseEx.ConnDuration"),Some(b"210".as_slice()));
    assert_eq!(timing.next(&ticket,connected).unwrap(),Some(start+Duration::from_millis(1010)));
    assert!(timing.take_due(&ticket,connected).unwrap().is_none());assert_eq!(timing.take_due(&ticket,start+Duration::from_millis(1010)).unwrap().unwrap().len(),1);
}
#[test]
fn native_call_missed_ticks_coalesce_without_a_burst_or_replaying_duration_fields(){
    let(_rpc,_,_,ticket,mut timing,start)=accepted_timing(1);let now=start+Duration::from_millis(5500);
    assert!(timing.take_due(&ticket,now).unwrap().is_some());assert!(timing.take_due(&ticket,now).unwrap().is_none());
    assert_eq!(timing.next(&ticket,now).unwrap(),Some(start+Duration::from_millis(6010)));
    assert_eq!(timing.take_due(&ticket,start+Duration::from_millis(6010)).unwrap().unwrap().len(),1);
}
#[test]
fn native_zero_timer_disables_periodic_io_but_keeps_the_single_original_connected_report(){
    let(_rpc,_,_,ticket,mut timing,start)=accepted_timing(0);assert!(timing.next(&ticket,start+Duration::from_secs(1)).unwrap().is_none());
    assert!(timing.take_due(&ticket,start+Duration::from_secs(100)).unwrap().is_none());timing.connected(&ticket,start+Duration::from_secs(1)).unwrap();
    let fields=timing.take_due(&ticket,start+Duration::from_secs(1)).unwrap().unwrap();assert_eq!(value(&fields,b"keepAlive"),Some(b"0".as_slice()));
    assert!(timing.take_due(&ticket,start+Duration::from_secs(100)).unwrap().is_none());assert!(timing.next(&ticket,start+Duration::from_secs(100)).unwrap().is_none());
}
#[test]
fn native_call_clock_rollback_does_not_wrap_a_duration_or_consume_the_original_tick(){
    let(_rpc,_,_,ticket,mut timing,start)=accepted_timing(1);assert!(timing.take_due(&ticket,start).is_err());
    assert!(timing.connected(&ticket,start).is_err());let fields=timing.take_due(&ticket,start+Duration::from_millis(1010)).unwrap().unwrap();assert_eq!(value(&fields,b"RseEx.AcceptDuration"),Some(b"10".as_slice()));
}
#[test]
fn native_keepalive_uses_inform_wire_but_original_per_call_route_and_own_pending_id(){
    let(mut rpc,owner,scope,ticket,_,_)=accepted_timing(1);
    let(id,bytes)=rpc.prepare_call(&ticket,CallOperation::KeepAlive,&[RpcProperty{key:b"keepAlive",value:b"1000"}]).unwrap();
    assert_target(&bytes,b"#per-call",CallOperation::KeepAlive);assert_eq!(CallOperation::KeepAlive.method(),CallOperation::Inform.method());
    assert!(finish(&mut rpc,&owner,&scope,id,CallOperation::KeepAlive,true).accepted());assert!(ticket.is_live());
}
#[test]
fn native_keepalive_cannot_overlap_and_negative_semantics_revoke_only_the_call(){
    let(mut rpc,owner,scope,ticket,_,_)=accepted_timing(1);let fields=[RpcProperty{key:b"keepAlive",value:b"1000"}];
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::KeepAlive,&fields).unwrap();assert!(rpc.prepare_call(&ticket,CallOperation::KeepAlive,&fields).is_err());
    assert!(!finish(&mut rpc,&owner,&scope,id,CallOperation::KeepAlive,false).accepted());assert!(!ticket.is_live());assert!(scope.is_open());assert!(rpc.prepare_call(&ticket,CallOperation::KeepAlive,&fields).is_err());
}
#[test]
fn native_late_keepalive_result_after_release_never_restores_call_or_media(){
    let(mut rpc,owner,scope,ticket,_,_)=accepted_timing(1);let(heartbeat,_)=rpc.prepare_call(&ticket,CallOperation::KeepAlive,&[RpcProperty{key:b"keepAlive",value:b"1000"}]).unwrap();
    let(release,_)=rpc.prepare_call(&ticket,CallOperation::Release,&[]).unwrap();assert!(!ticket.is_live());
    assert!(!finish(&mut rpc,&owner,&scope,heartbeat,CallOperation::KeepAlive,true).accepted());assert!(finish(&mut rpc,&owner,&scope,release,CallOperation::Release,true).accepted());assert_eq!(ticket.with_media_gate(||1),None);
}
#[test]
fn legacy_and_unaccepted_native_calls_cannot_gain_the_new_keepalive_operation(){
    let(mut legacy,owner,scope,d)=context();let incoming=notice(&d,&owner,&scope,CallNoticeKind::Invited,ID);let ticket=legacy.register_call_notice(&incoming).unwrap().unwrap();
    assert!(legacy.prepare_call(&ticket,CallOperation::KeepAlive,&[RpcProperty{key:b"keepAlive",value:b"1000"}]).is_err());
    let(mut native,_,_,ticket,_)=prepared_timing(1);assert!(native.prepare_call(&ticket,CallOperation::KeepAlive,&[RpcProperty{key:b"keepAlive",value:b"1000"}]).is_err());
}
#[test]
fn native_keepalive_rejects_auth_properties_duplicates_malformed_numbers_and_oversize_before_submission(){
    let(mut rpc,_,_,ticket,_,_)=accepted_timing(1);
    for fields in [vec![],vec![RpcProperty{key:b"token",value:b"secret"}],vec![RpcProperty{key:b"keepAlive",value:b"-1"}],
        vec![RpcProperty{key:b"keepAlive",value:b"2147483648"}],vec![RpcProperty{key:b"keepAlive",value:b"1"},RpcProperty{key:b"keepAlive",value:b"1"}],
        vec![RpcProperty{key:b"keepAlive",value:b"1"},RpcProperty{key:b"RseEx.AcceptDuration",value:b"18446744073709551616"}]]{
        assert!(rpc.prepare_call(&ticket,CallOperation::KeepAlive,&fields).is_err());
    }
    let(id,_)=rpc.prepare_call(&ticket,CallOperation::KeepAlive,&[RpcProperty{key:b"keepAlive",value:b"1000"}]).unwrap();assert_eq!(id,4);
}
#[test]
fn native_lifecycle_config_requires_native_control_and_keeps_old_private_serialization_unchanged(){
    let sdk=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");let mut config=n::account_call_test_support::call_config(&sdk.root,&sdk.root);
    let original=config.encode_private().unwrap();let loaded=n::account_config::AccountConfig::decode_private(&original).unwrap();assert!(loaded.call_media.as_ref().unwrap().native_lifecycle.is_none());
    assert_eq!(original.as_slice(),loaded.encode_private().unwrap().as_slice());
    config.call_media.as_mut().unwrap().native_lifecycle=Some(NativeCallLifecycleConfig{session_timer_seconds:30});assert!(config.validate(100).is_err());
}
#[test]
fn native_lifecycle_config_roundtrips_explicit_timer_without_replacing_media_tls_or_enabling_cellular(){
    let sdk=n::tls_engine::tests::Certificates::generate("IP:127.0.0.1");let mut config=n::account_config::tests::native_fixture(&sdk.root,"127.0.0.1",12345);
    config.profile.incoming=None;config.profile.native_receivers=Some(NativeReceiverConfig{direct_domains:None});config.profile.native_call_control=true;
    config.call_media=n::account_call_test_support::call_config(&sdk.root,&sdk.root).call_media;
    config.call_media.as_mut().unwrap().native_lifecycle=Some(NativeCallLifecycleConfig{session_timer_seconds:30});config.validate(100).unwrap();
    let raw=config.encode_private().unwrap();let restored=n::account_config::AccountConfig::decode_private(&raw).unwrap();
    assert_eq!(restored.call_media.as_ref().unwrap().native_lifecycle.unwrap().session_timer_seconds,30);assert_eq!(raw.as_slice(),restored.encode_private().unwrap().as_slice());
    config.call_media.as_mut().unwrap().allow_plain_media=false;assert!(config.validate(100).is_err());
}
