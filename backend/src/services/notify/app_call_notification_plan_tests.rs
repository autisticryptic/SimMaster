//! Pure call-plan tests; no NotificationSender, modem, SDK or channel execution.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::account_call_notifications::CallNoticeSnapshot;
fn record()->CallRecord{CallRecord{id:91,line_id:Some(LINE.into()),direction:"incoming".into(),phone_number:"synthetic-caller".into(),duration:0,start_time:"2026-05-14T16:30:45Z".into(),end_time:None,answered:false,
    sip_status:None,failure_code:None,failure_category:None,q850_cause:None,failure_retryable:None,retry_after_seconds:None,carrier_reason:None}}
fn call_config()->NotificationConfig{let mut c=fixture();c.rules[0].event_type=NotificationEventType::Call;c.rules[0].template="{{line_id}}|{{caller}}|{{方向}}|{{answered}}|{{duration}}|{{start_time}}|{{unknown}}".into();c}
fn call_targets(c:NotificationConfig,r:&CallRecord)->Vec<(String,String,String)>{
    let snap=snapshot(c);let plan=CallNoticeSnapshot::plan(&snap,r).unwrap();plan.targets().iter().map(|t|(t.rule_id.clone(),t.channel_id.clone(),t.body.clone())).collect()
}
#[test]
fn app_call_plan_is_explicit_call_rule_and_keeps_original_account_not_sim_or_sdk_identity(){
    let r=record();let items=call_targets(call_config(),&r);assert_eq!(items.len(),1);assert!(items[0].2.starts_with(&format!("{}|synthetic-caller|来电|否|0|",LINE)));assert!(items[0].2.ends_with("|{{unknown}}"));assert!(!items[0].2.contains("SipPwd"));
    assert!(call_targets(fixture(),&r).is_empty());
}
#[test]
fn app_call_other_line_outgoing_answered_or_ended_records_never_become_incoming_alerts(){
    let mut c=call_config();c.rules[0].sim_channel_ids=vec![OTHER_LINE.into()];assert!(call_targets(c,&record()).is_empty());
    for which in 0..4{let mut r=record();match which{0=>r.direction="outgoing".into(),1=>r.answered=true,2=>r.end_time=Some(r.start_time.clone()),_=>r.line_id=None};assert!(CallNoticeSnapshot::plan(&snapshot(call_config()),&r).is_err());}
}
#[test]
fn app_call_disabled_quiet_and_duplicate_channel_references_remain_pure_no_send(){
    let mut c=call_config();c.rules[0].enabled=false;assert!(call_targets(c,&record()).is_empty());
    let mut c=call_config();c.rules[0].quiet_hours=vec![all_day_quiet()];assert!(call_targets(c,&record()).is_empty());
    let mut c=call_config();c.rules[0].channel_ids.push(c.rules[0].channel_ids[0].clone());assert_eq!(call_targets(c,&record()).len(),1);
}
#[test]
fn app_call_bounded_renderer_preserves_literal_tokens_without_recursive_replacement(){
    let mut c=call_config();c.rules[0].template="{{caller}}|{{是否接通}}|{{结束时间}}|{{线路ID}}|{{unknown}}".into();let items=call_targets(c,&record());assert_eq!(items[0].2,format!("synthetic-caller|false||{}|{{{{unknown}}}}",LINE));
}
#[test]
fn app_call_repeated_expansion_is_rejected_before_large_allocation_or_queue_admission(){
    let mut r=record();r.phone_number="A".repeat(128);let mut c=call_config();c.rules[0].template="{{caller}}".repeat(16000);assert!(CallNoticeSnapshot::plan(&snapshot(c),&r).is_err());
}
#[test]
fn app_call_destination_and_rule_changes_have_distinct_immutable_fingerprints(){
    let c=call_config();let a=CallNoticeSnapshot::plan(&snapshot(c.clone()),&record()).unwrap().targets()[0].fingerprint.clone();
    let mut changed=c.clone();changed.channels[0].config["url"]=json!("https://notice.invalid/new");let b=CallNoticeSnapshot::plan(&snapshot(changed),&record()).unwrap().targets()[0].fingerprint.clone();assert_ne!(a,b);
    let mut changed=c;changed.rules[0].template="new".into();let d=CallNoticeSnapshot::plan(&snapshot(changed),&record()).unwrap().targets()[0].fingerprint.clone();assert_ne!(a,d);
}
#[test]
fn app_call_capture_is_not_mutated_by_later_global_configuration_changes(){
    let mut c=call_config();let captured=snapshot(c.clone());c.rules[0].template="changed".into();let first=CallNoticeSnapshot::plan(&captured,&record()).unwrap();assert!(!first.targets()[0].body.contains("changed"));
}
#[test]
fn app_call_plan_retains_original_eight_target_and_256k_render_budgets(){
    let mut c=fanout(9);c.rules[0].event_type=NotificationEventType::Call;assert!(CallNoticeSnapshot::plan(&snapshot(c),&record()).is_err());
    let mut c=call_config();c.rules[0].template="X".repeat(128*1024+1);assert!(AppVoiceNotificationSnapshot::new(c).is_err());
}
