//! Explicit reuse of the phone App's already trusted UUID; never new VTD trust.
//! getCallStatus's ORIGINAL authentication_code gates every consequential phase.
use super::{AppLoginManager, ActiveGuard, DatabaseConfiguration, DeviceTrustObservation, Error, LoginIntent, LoginRequest, now, store};
use super::device_sync::DeviceSyncObservation;
use super::http::{AppClient, BoundNumber, CallStatus};
use super::super::account_config::AccountConfig;

fn require_current_trust(status:CallStatus)->Result<CallStatus,Error> {
    // This is the current authenticated response for this attempt's original
    // UUID, NOT a persisted observation/local safeNum or a UI trust assertion.
    if status.authentication_code!=Some(1) { return Err(Error::new("app_login_device_untrusted")); }
    Ok(status)
}
impl AppLoginManager {
    fn record_reuse_observation(&self,owner:&str,id:&str,guard:&ActiveGuard,status:Result<CallStatus,Error>,sync:DeviceSyncObservation)->Result<(),Error> {
        let observation=DeviceTrustObservation::capture_reuse(&guard.snapshot,now(),status,sync);
        self.database.with_conn(|c|store::set_device_trust(c,id,owner,&observation,now())).map_err(Error::new)
    }
    pub(super) async fn run_trusted_reuse(&self,owner:&str,request:&LoginRequest,intent:LoginIntent,configuration:&DatabaseConfiguration,
        client:&AppClient,guard:&ActiveGuard,bound:&BoundNumber)->Result<(),Error> {
        let id=request.attempt_id.as_str();
        if !intent.reuses_trusted_device() { return Err(Error::new("app_login_request_invalid")); }
        self.step(owner,id,"checking_device_trust",None)?;
        let mut sync=if client.device_sync_configured() {DeviceSyncObservation::not_requested()} else {DeviceSyncObservation::not_configured()};
        let initial=client.call_status().await;
        self.record_reuse_observation(owner,id,guard,initial,sync.clone())?;
        let initial=require_current_trust(initial?)?;
        if intent==LoginIntent::ReuseHosting && !initial.hosting_enabled {
            return Err(Error::new("app_login_business_not_enabled"));
        }
        // Validate observed report ownership before requesting more material,
        // but do NOT send a report until the native template and a fresh trust
        // readback below have both been checked.
        if client.device_sync_configured() { client.validate_device_sync_context(bound)?; }
        self.step(owner,id,"fetching_credentials",None)?;
        let material=client.phone_credentials(bound).await?;
        let runtime=configuration.runtime.clone().ok_or(Error::new("app_login_native_configuration_missing"))?;
        let config:AccountConfig=runtime.bind(&bound.identity(),material)?;
        config.validate(now() as u64).map_err(|_|Error::new("app_login_native_configuration_invalid"))?;
        let accounts=self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        accounts.login_preflight_create().map_err(|_|Error::new("app_login_account_creation_failed"))?;

        // Trust may have moved to another device while credentials/report were
        // exchanged. Check AGAIN before writing business or creating a receiver.
        self.step(owner,id,"checking_business",None)?;
        let latest=client.call_status().await;
        self.record_reuse_observation(owner,id,guard,latest,sync.clone())?;
        let mut latest=require_current_trust(latest?)?;
        if intent==LoginIntent::ReuseHosting && !latest.hosting_enabled { return Err(Error::new("app_login_business_not_enabled")); }
        if client.device_sync_configured() {
            // This explicit device report also writes remote state. Validate
            // the supported native context and CURRENT UUID trust before it.
            self.step(owner,id,"syncing_device",None)?;
            sync=DeviceSyncObservation::request_uncertain();
            self.record_reuse_observation(owner,id,guard,Ok(latest),sync.clone())?;
            let result=client.sync_device(bound,latest).await;
            sync=DeviceSyncObservation::capture(result);
            self.record_reuse_observation(owner,id,guard,Ok(latest),sync.clone())?;
            result?;
            self.step(owner,id,"checking_business",None)?;
            let after_sync=client.call_status().await;
            self.record_reuse_observation(owner,id,guard,after_sync,sync.clone())?;
            latest=require_current_trust(after_sync?)?;
        }
        if intent==LoginIntent::ReuseHosting {
            if !latest.hosting_enabled { return Err(Error::new("app_login_business_not_enabled")); }
            self.step(owner,id,"creating_account",Some("already_enabled"))?;
        } else if latest.hosting_enabled {
            self.step(owner,id,"creating_account",Some("already_enabled"))?;
        } else {
            // EnableTrustedHosting is a separate, explicitly consented business
            // action. It still NEVER enrols, mints a carrier token or writes VTD.
            self.step(owner,id,"enabling_business",Some("request_uncertain"))?;
            if let Err(error)=client.enable_business().await {
                if error.server_code.is_some() { self.step(owner,id,"enabling_business",Some("rejected"))?; }
                if error.server_code==Some(506) { return Err(Error {code:"app_login_device_untrusted",server_code:error.server_code}); }
                return Err(error);
            }
            self.step(owner,id,"confirming_business",Some("accepted_unconfirmed"))?;
            let confirmed=client.call_status().await;
            self.record_reuse_observation(owner,id,guard,confirmed,sync)?;
            let confirmed=require_current_trust(confirmed?)?;
            if !confirmed.hosting_enabled { return Err(Error::new("app_login_business_unconfirmed")); }
            self.step(owner,id,"creating_account",Some("enabled"))?;
        }
        // Local account/runtime construction is separate from App trust and
        // remote business acceptance. Existing SDK token/identity/ACK fences stay.
        let account=accounts.create(config,now() as u64).await.map_err(|_|Error::new("app_login_account_creation_failed"))?;
        let line_id=account.account.line_id.clone();
        self.database.with_conn(|c|store::set_line(c,id,owner,&line_id,now())).map_err(Error::new)?;
        if request.start_receiver {
            self.step(owner,id,"starting_receiver",None)?;
            accounts.start(&line_id,account.account.revision,now() as u64).await.map_err(|_|Error::new("app_login_receiver_start_failed"))?;
        }
        Ok(())
    }
}
