//! Captured native incoming-call plans and explicit channel delivery. No SIM
//! lookup, stale template rerendering, SDK control or fake SMS record is used.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::account_call_notifications::{CallNoticeSnapshot,CallNoticeWork,CallNoticeOutcome,CallNoticeFuture};
fn valid_call(record:&CallRecord)->bool{
    record.id>0&&record.line_id.as_deref().is_some_and(valid_line_id)&&record.direction=="incoming"&&!record.answered&&record.end_time.is_none()&&record.duration==0
        &&record.phone_number.len()<=128&&record.phone_number.bytes().all(|b|b.is_ascii_alphanumeric()||matches!(b,b'+'|b'-'|b'_'|b'.'))&&record.start_time.len()<=64
}
fn render_call_bounded(template:&str,record:&CallRecord)->Result<String,&'static str>{
    if template.len()>MAX_BODY_BYTES||!valid_call(record){return Err(INVALID_MESSAGE);}
    let mut out=Zeroizing::new(String::with_capacity(MAX_BODY_BYTES));let mut remaining=template;
    let append=|out:&mut String,text:&str|->Result<(),&'static str>{if text.len()>MAX_BODY_BYTES-out.len(){return Err(PLAN_TOO_LARGE);}out.push_str(text);Ok(())};
    let id=record.id.to_string();let at=format_notification_time(&record.start_time);let line=record.line_id.as_deref().ok_or(INVALID_MESSAGE)?;
    while let Some(begin)=remaining.find("{{"){
        append(&mut out,&remaining[..begin])?;let next=&remaining[begin..];let Some(end)=next.find("}}")else{append(&mut out,next)?;return Ok(out.to_string());};
        let literal=&next[..end+2];let value=match literal{
            "{{id}}"=>id.as_str(),"{{line_id}}"|"{{线路ID}}"=>line,"{{phone_number}}"|"{{电话号码}}"|"{{caller}}"=>record.phone_number.as_str(),
            "{{direction}}"=>"incoming","{{方向}}"|"{{direction_cn}}"=>"来电","{{duration}}"|"{{时长}}"=>"0",
            "{{start_time}}"|"{{开始时间}}"|"{{time}}"|"{{时间}}"=>at.as_str(),"{{end_time}}"|"{{结束时间}}"=>"",
            "{{answered}}"|"{{已接听}}"=>"否","{{answered_bool}}"|"{{是否接通}}"=>"false",_=>literal,
        };append(&mut out,value)?;remaining=&next[end+2..];
    }append(&mut out,remaining)?;Ok(out.to_string())
}
impl CallNoticeSnapshot for AppVoiceNotificationSnapshot{
    fn plan(&self,record:&CallRecord)->Result<NoticePlan,&'static str>{
        if !valid_call(record){return Err(INVALID_MESSAGE);}
        let event=NotificationEvent::Call(record);let mut targets=Vec::new();let mut bytes=0usize;
        for rule in &self.config.rules{
            if !rule.enabled||rule.event_type!=NotificationEventType::Call||!rule_matches(rule,&event)||quiet_hours_active(&rule.quiet_hours){continue;}
            let mut seen=HashSet::new();let channels=self.config.channels.iter().filter(|c|c.enabled&&rule.channel_ids.iter().any(|id|id==&c.id)&&seen.insert(c.id.as_str())).collect::<Vec<_>>();
            if targets.len()+channels.len()>MAX_TARGETS{return Err(PLAN_TOO_LARGE);}
            if channels.is_empty(){continue;}
            let template=if rule.template.trim().is_empty(){crate::platform::config::default_rule_template(NotificationEventType::Call)}else{rule.template.clone()};
            let title=Zeroizing::new("SimAdmin 无忧行来电通知".to_owned());let body=Zeroizing::new(render_call_bounded(&template,record)?);
            for channel in channels{
                bytes=bytes.checked_add(title.len()+body.len()).filter(|n|*n<=MAX_PLAN_BYTES).ok_or(PLAN_TOO_LARGE)?;
                targets.push(NoticeTarget{rule_id:rule.id.clone(),channel_id:channel.id.clone(),fingerprint:pair_fingerprint(rule,channel)?,title:title.to_string(),body:body.to_string()});
            }
        }NoticePlan::bounded(targets)
    }
}
impl NotificationSender{
    pub(super) fn deliver_app_call<'a>(&'a self,work:&'a CallNoticeWork)->CallNoticeFuture<'a>{Box::pin(async move{
        if !valid_line_id(&work.line_id)||work.id<=0||work.call_history_id<=0||!valid_target_id(&work.rule_id)||!valid_target_id(&work.channel_id)
            ||work.title.len()>MAX_TITLE_BYTES||work.body.len()>MAX_BODY_BYTES{return CallNoticeOutcome::Failed;}
        let config=self.get_config();if validate_config(&config).is_err(){return CallNoticeOutcome::TargetChanged;}
        let Some(rule)=config.rules.iter().find(|r|r.enabled&&r.id==work.rule_id&&r.event_type==NotificationEventType::Call
            &&r.channel_ids.iter().any(|id|id==&work.channel_id)&&(r.sim_channel_ids.is_empty()||r.sim_channel_ids.iter().any(|id|id==&work.line_id)))else{return CallNoticeOutcome::TargetChanged;};
        let Some(channel)=config.channels.iter().find(|c|c.enabled&&c.id==work.channel_id)else{return CallNoticeOutcome::TargetChanged;};
        match pair_fingerprint(rule,channel){Ok(value)if value==work.fingerprint=>{},_=>return CallNoticeOutcome::TargetChanged,}
        if quiet_hours_active(&rule.quiet_hours){return CallNoticeOutcome::Deferred(Duration::from_secs(5));}
        match self.rate_limit_reason(channel){Ok(None)=>{},Ok(Some(reason))=>{drop(Zeroizing::new(reason));return CallNoticeOutcome::Deferred(Duration::from_secs(u64::from(channel.rate_limit.window_seconds).clamp(1,30)));},Err(error)=>{drop(Zeroizing::new(error));return CallNoticeOutcome::Failed;}}
        match self.send_text_to_channel(channel,work.title.as_str(),work.body.as_str()).await{
            Ok(receipt)=>{drop(Zeroizing::new(receipt));CallNoticeOutcome::Delivered},
            // A transport/channel error may follow actual remote acceptance.
            // Do not requeue a possibly delivered call alert on another nonce.
            Err(error)=>{drop(Zeroizing::new(error));CallNoticeOutcome::Uncertain},
        }
    })}
}
