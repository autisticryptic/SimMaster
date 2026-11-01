use super::*;
use super::super::transport_packets::TransportBytes;
fn message(kind:CallKind,bytes:&[u8],deadline:Instant)->CommonMessage{CommonMessage{kind,payload:TransportBytes::from_bytes(bytes.to_vec()),deadline}}
#[test]
fn received_ping_queues_only_empty_pong_with_original_deadline(){
 let now=Instant::now();let mut state=CommonControls::new(now);let end=now+Duration::from_secs(1);
 let plan=state.on_messages(vec![message(CallKind::Ping,&[],end)],now).unwrap();assert!(plan.calls.is_empty());assert_eq!(plan.pong_deadlines,[end]);assert_eq!(state.delay_ms(),None);assert!(state.pending.is_empty());
}
#[test]
fn unsolicited_pong_neither_replies_nor_invents_a_latency_sample(){
 let now=Instant::now();let mut state=CommonControls::new(now);let plan=state.on_messages(vec![message(CallKind::Pong,&[],now+Duration::from_secs(1))],now).unwrap();
 assert!(plan.calls.is_empty());assert!(plan.pong_deadlines.is_empty());assert_eq!(state.delay_ms(),None);
}
#[test]
fn pong_uses_original_pending_fifo_and_native_integer_average_rounding(){
 let now=Instant::now();let mut state=CommonControls::new(now);let end=now+Duration::from_secs(2);
 state.prepare_ping(end,now).unwrap();state.prepare_ping(end,now+Duration::from_millis(1)).unwrap();
 state.on_messages(vec![message(CallKind::Pong,&[],end)],now+Duration::from_millis(101)).unwrap();assert_eq!(state.delay_ms(),Some(101));
 state.on_messages(vec![message(CallKind::Pong,&[],end)],now+Duration::from_millis(201)).unwrap();assert_eq!(state.delay_ms(),Some(151));assert!(state.pending.is_empty());
}
#[test]
fn expired_ping_does_not_become_success_on_a_later_pong(){
 let now=Instant::now();let mut state=CommonControls::new(now);state.prepare_ping(now+Duration::from_millis(50),now).unwrap();
 assert_eq!(state.on_messages(vec![message(CallKind::Pong,&[],now+Duration::from_secs(1))],now+Duration::from_millis(50)).err(),Some(ControlError::Timeout));assert!(state.closed);assert_eq!(state.delay_ms(),None);
}
#[test]
fn remote_ping_traffic_never_renews_an_outstanding_local_ping_budget(){
 let now=Instant::now();let mut state=CommonControls::new(now);let end=now+Duration::from_millis(60);state.prepare_ping(end,now).unwrap();
 state.on_messages(vec![message(CallKind::Ping,&[],now+Duration::from_secs(3))],now+Duration::from_millis(59)).unwrap();assert_eq!(state.deadline(),Some(end));assert_eq!(state.tick(end),Err(ControlError::Timeout));
}
#[test]
fn release_suppresses_earlier_calls_and_pong_obligations_in_same_group(){
 let now=Instant::now();let end=now+Duration::from_secs(2);let mut state=CommonControls::new(now);
 assert_eq!(state.on_messages(vec![message(CallKind::Reply,b"not-yet-published",end),message(CallKind::Ping,&[],end),message(CallKind::Release,&[],end)],now).err(),Some(ControlError::RemoteReleased));assert!(state.closed);
 assert_eq!(state.on_messages(Vec::new(),now).err(),Some(ControlError::Closed));
}
#[test]
fn malformed_control_payload_poisoning_is_terminal_and_never_exposes_prior_call(){
 let now=Instant::now();let end=now+Duration::from_secs(2);
 for kind in [CallKind::Ping,CallKind::Pong,CallKind::Release]{let mut state=CommonControls::new(now);assert!(state.on_messages(vec![message(CallKind::Reply,b"x",end),message(kind,b"bad",end)],now).is_err());assert!(state.closed);}
}
#[test]
fn bounded_pong_plan_rejects_a_control_flood_before_any_response_is_sent(){
 let now=Instant::now();let end=now+Duration::from_secs(2);let mut state=CommonControls::new(now);
 let messages=(0..65).map(|_|message(CallKind::Ping,&[],end)).collect();assert_eq!(state.on_messages(messages,now).err(),Some(ControlError::Bound));assert!(state.closed);
}
#[test]
fn local_ping_admission_bound_is_atomic_and_does_not_extend_existing_pings(){
 let now=Instant::now();let end=now+Duration::from_secs(2);let mut state=CommonControls::new(now);
 for _ in 0..16{state.prepare_ping(end,now).unwrap();}assert_eq!(state.prepare_ping(end,now),Err(ControlError::Bound));assert_eq!(state.pending.len(),16);assert_eq!(state.deadline(),Some(end));
}
#[test]
fn original_clock_rollback_closes_control_state_without_resampling(){
 let now=Instant::now();let mut state=CommonControls::new(now);state.prepare_ping(now+Duration::from_secs(2),now).unwrap();assert_eq!(state.tick(now-Duration::from_millis(1)),Err(ControlError::ClockRollback));assert!(state.pending.is_empty());
}
#[test]
fn expired_completed_common_message_is_not_published_after_decode_or_queue_delay(){
 let now=Instant::now();let mut state=CommonControls::new(now);assert_eq!(state.on_messages(vec![message(CallKind::Reply,b"late",now)],now).err(),Some(ControlError::Timeout));assert!(state.closed);
}
#[test]
fn mixed_calls_preserve_kind_payload_and_deadline_without_becoming_business_ack(){
 let now=Instant::now();let end=now+Duration::from_secs(1);let mut state=CommonControls::new(now);
 let plan=state.on_messages(vec![message(CallKind::Request,b"request",end),message(CallKind::Reply,b"reply",end)],now).unwrap();
 assert_eq!(plan.calls.len(),2);assert_eq!(plan.calls[0].kind,CallKind::Request);assert_eq!(plan.calls[1].payload.expose_to_transport(),b"reply");assert_eq!(plan.calls[1].deadline,end);assert_eq!(state.delay_ms(),None);
}
#[test]
fn invalid_deadline_configuration_and_debug_expose_no_private_data(){
 let now=Instant::now();let mut state=CommonControls::new(now);for end in [now,now+Duration::from_secs(31)]{assert_eq!(state.prepare_ping(end,now),Err(ControlError::Configuration));}
 let plan=state.on_messages(vec![message(CallKind::Reply,b"private-call",now+Duration::from_secs(1))],now).unwrap();assert!(!format!("{state:?} {plan:?}").contains("private-call"));
}
