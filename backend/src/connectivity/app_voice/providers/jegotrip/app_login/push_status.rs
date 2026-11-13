//! Read-only layered push diagnostics. Configuration, armed receive paths,
//! retained observations and externally verified delivery are distinct facts.
use super::config::RuntimeTemplate;
use super::super::account_runtime::AccountView;
use rusqlite::Connection;
use serde::Serialize;

#[derive(Default,Serialize)]
pub(crate) struct PushTemplateStatus {
    pub runtime_template_present:bool,
    pub sms_receiver_present:bool, pub sip_request_receiver_selected:bool,
    pub call_receiver_present:bool, pub call_control_present:bool, pub call_media_present:bool,
    pub reconnect_configured:bool,
}
impl PushTemplateStatus {
    pub(super) fn inspect(template:Option<&RuntimeTemplate>)->Self {
        let Some(template)=template else {return Self::default();};
        let p=&template.profile;
        Self {runtime_template_present:true,sms_receiver_present:p.incoming.is_some() || p.native_receivers.is_some(),
            sip_request_receiver_selected:p.native_receivers.is_some() || p.incoming.as_ref().is_some_and(|b|b.receiver_kind.as_slice()==b"Sip.Request"),
            call_receiver_present:p.call_incoming.is_some() || p.native_receivers.is_some(),call_control_present:p.call_control.is_some(),
            call_media_present:template.call_media.is_some(),reconnect_configured:template.reconnect.is_some()}
    }
}
#[derive(Default,Serialize)]
pub(crate) struct PushStoredObservations {
    pub app_sms_records:Option<u64>,pub app_call_records:Option<u64>,
    pub retained_inbox_records:Option<u64>,pub pending_sms_notifications:Option<u64>,
}
fn count_if_present(conn:&Connection,table:&str,sql:&str)->Result<Option<u64>,&'static str> {
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[table],|r|r.get(0))
        .map_err(|_|"app_login_storage_failed")?;
    if !exists {return Ok(None);}
    let count:i64=conn.query_row(sql,[],|r|r.get(0)).map_err(|_|"app_login_storage_failed")?;
    Ok(Some(u64::try_from(count).map_err(|_|"app_login_storage_failed")?))
}
pub(super) fn stored_observations(conn:&Connection)->Result<PushStoredObservations,&'static str> {
    Ok(PushStoredObservations {
        app_sms_records:count_if_present(conn,"sms_messages","SELECT COUNT(*) FROM sms_messages WHERE transport='app_voice'")?,
        app_call_records:count_if_present(conn,"call_history","SELECT COUNT(*) FROM call_history WHERE app_call_token IS NOT NULL")?,
        retained_inbox_records:count_if_present(conn,"app_voice_inbox","SELECT COUNT(*) FROM app_voice_inbox")?,
        pending_sms_notifications:count_if_present(conn,"app_voice_notification_jobs","SELECT COUNT(*) FROM app_voice_notification_jobs WHERE state IN ('pending','sending')")?,
    })
}
#[derive(Serialize)]
pub(crate) struct PushImplementation {
    pub online_native_sms:bool,pub online_native_calls:bool,pub browser_event_stream:bool,
    pub platform_offline_wakeup:bool,pub sms_notification_forwarding:bool,pub call_notification_forwarding:bool,
}
#[derive(Serialize)]
pub(crate) struct PushStatusView {
    pub initialized:bool,pub observed_at_unix_seconds:i64,pub configuration:PushTemplateStatus,
    pub implementation:PushImplementation,pub native_accounts:Vec<AccountView>,
    pub stored_observations:PushStoredObservations,pub warnings:Vec<&'static str>,
    pub receiver_namespace_verification:&'static str,pub external_delivery_verification:&'static str,
}
impl PushStatusView {
    pub(super) fn assembled(initialized:bool,at:i64,configuration:PushTemplateStatus,native_accounts:Vec<AccountView>,stored_observations:PushStoredObservations)->Self {
        let mut warnings=Vec::new();
        if !initialized {warnings.push("login_manager_uninitialized");}
        if !configuration.runtime_template_present {warnings.push("runtime_template_missing");}
        if !configuration.sip_request_receiver_selected {warnings.push("sms_receiver_missing_or_unsupported");}
        if !configuration.call_receiver_present {warnings.push("call_receiver_missing");}
        if !configuration.call_control_present {warnings.push("call_control_missing");}
        if !configuration.call_media_present {warnings.push("call_media_missing");}
        if native_accounts.is_empty() {warnings.push("native_accounts_absent");}
        if !native_accounts.iter().any(|a|a.incoming_message_ready) {warnings.push("no_armed_sms_receiver");}
        if !native_accounts.iter().any(|a|a.incoming_call_ready) {warnings.push("no_armed_call_receiver");}
        if native_accounts.iter().any(|a|a.account_alive_state==Some("unsupported")) {warnings.push("account_alive_unimplemented");}
        warnings.push("platform_offline_wakeup_unimplemented");warnings.push("call_notifications_unimplemented");
        Self {initialized,observed_at_unix_seconds:at,configuration,native_accounts,stored_observations,warnings,
            implementation:PushImplementation {online_native_sms:true,online_native_calls:true,browser_event_stream:true,
                platform_offline_wakeup:false,sms_notification_forwarding:true,call_notification_forwarding:false},
            receiver_namespace_verification:"requires_original_binding_evidence",
            external_delivery_verification:"requires_external_sms_call_test"}
    }
}
