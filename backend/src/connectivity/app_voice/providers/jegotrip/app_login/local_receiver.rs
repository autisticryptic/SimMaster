//! Explicit original-device LOCAL receiver preparation, not a forwarding switch.
//!
//! Actual 8.9.4 JustalkManager.login uses current phone/SIP material; its matched
//! SDK login drives RegisterTimer.updateStatus(4). getCallStatus.cs_forward is a
//! separate App business observation. This path neither relaxes the old remote
//! business intents nor converts a stored/other-endpoint code into an SDK grant.
use super::{AppLoginManager,ActiveGuard,DatabaseConfiguration,DeviceTrustObservation,Error,LoginRequest,now,store};
use super::http::{AppClient,BoundNumber,CallStatus,TrustDecision};
use super::super::account_config::AccountConfig;

fn require_current_device(result:Result<Option<TrustDecision>,Error>)->Result<(),Error> {
    match result? {
        Some(TrustDecision {authentication_code:Some(1)})=>Ok(()),
        _=>Err(Error::new("app_login_device_untrusted")),
    }
}

impl AppLoginManager {
    fn record_local_receiver(&self,owner:&str,id:&str,guard:&ActiveGuard,existing:Result<Option<TrustDecision>,Error>,
        status:Option<Result<CallStatus,Error>>)->Result<(),Error> {
        let observation=DeviceTrustObservation::capture_local_receiver(&guard.snapshot,now(),existing,status);
        self.database.with_conn(|c|store::set_device_trust(c,id,owner,&observation,now())).map_err(Error::new)
    }

    pub(super) async fn run_local_receiver(&self,owner:&str,request:&LoginRequest,configuration:&DatabaseConfiguration,
        client:&AppClient,guard:&ActiveGuard,bound:&BoundNumber)->Result<(),Error> {
        if !client.is_local_receiver() || request.enable_hosting {return Err(Error::new("app_login_request_invalid"));}
        let id=request.attempt_id.as_str();
        self.step(owner,id,"checking_device_trust",None)?;
        // A current existing-device response for this AppClient/original UUID
        // is required for preparation. No MobileAuth credential is requested.
        let initial=client.receiver_device_recognition().await;
        self.record_local_receiver(owner,id,guard,initial,None)?;
        require_current_device(initial)?;
        let runtime=configuration.runtime.clone().ok_or(Error::new("app_login_native_configuration_missing"))?;
        if runtime.profile.native_receivers.is_none() && runtime.profile.incoming.is_none() && runtime.profile.call_incoming.is_none() {
            return Err(Error::new("app_login_native_configuration_invalid"));
        }
        self.step(owner,id,"fetching_credentials",None)?;
        let material=client.receiver_phone_credentials(bound).await?;
        let config:AccountConfig=runtime.bind(&bound.identity(),material)?;
        config.validate(now() as u64).map_err(|_|Error::new("app_login_native_configuration_invalid"))?;
        let accounts=self.accounts.upgrade().ok_or(Error::new("app_login_unavailable"))?;
        accounts.login_preflight_create().map_err(|_|Error::new("app_login_account_creation_failed"))?;

        self.step(owner,id,"checking_device_trust",None)?;
        let latest=client.receiver_device_recognition().await;
        self.record_local_receiver(owner,id,guard,latest,None)?;
        require_current_device(latest)?;
        // Observe forwarding without treating its code/switch as local SDK
        // readiness. A malformed, revoked or failed HTTP response is still an
        // error; only a valid observation may accompany account creation.
        let status=client.call_status().await;
        self.record_local_receiver(owner,id,guard,latest,Some(status))?;
        let _=status?;

        self.step(owner,id,"creating_account",None)?;
        let account=accounts.create(config,now() as u64).await.map_err(|_|Error::new("app_login_account_creation_failed"))?;
        let line_id=account.account.line_id.clone();
        self.database.with_conn(|c|store::set_line(c,id,owner,&line_id,now())).map_err(Error::new)?;
        if request.start_receiver {
            self.step(owner,id,"starting_receiver",None)?;
            accounts.start(&line_id,account.account.revision,now() as u64).await.map_err(|_|Error::new("app_login_receiver_start_failed"))?;
        }
        // No device report or setVoip here. The original native controller still
        // requires its OWN SDK login + registration ACK + armed channels.
        Ok(())
    }
}
