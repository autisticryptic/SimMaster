//! VoWiFi IMS-only UICC binding. EAP deliberately uses the separate USIM path.
use super::*;
use crate::connectivity::modems::ims::uicc_ims::{self, ImsIdentitySource, UiccCardBinding};

#[derive(Clone)]
pub(super) struct LiveImsSelection {
    pub identity: LiveImsRegisterIdentity,
    pub target: LiveImsTarget,
    device: LiveSimDevice,
    card: UiccCardBinding,
    aid: Vec<u8>,
    isim: bool,
}

impl LiveImsSelection {
    pub fn verify_reader_mapping(&self, line_id: &str) -> Result<(), &'static str> {
        if sim_device_for_line(line_id) == self.device { Ok(()) } else { Err("ims_uicc_binding_changed") }
    }

    async fn observe(&self, line_id: &str) -> Result<UiccCardBinding, &'static str> {
        self.verify_reader_mapping(line_id)?;
        let endpoint = if self.device.pcsc_reader.is_empty() { &self.device.qmi_device } else { &self.device.pcsc_reader };
        uicc_ims::capture_card_binding(
            endpoint, self.device.uim_slot, &self.device.modem_path,
        ).await
    }

    pub async fn verify(&self, line_id: &str) -> Result<(), &'static str> {
        self.card.verify(&self.observe(line_id).await?)
    }

    pub async fn for_format(&self, line_id: &str, format: LiveRegisterIdentityFormat)
        -> Result<LiveImsRegisterIdentity, LiveStageError> {
        self.verify(line_id).await.map_err(live_stage_error)?;
        let phone = if !self.isim && matches!(format, LiveRegisterIdentityFormat::MsisdnPhoneUri) {
            let conn = zbus::Connection::system().await.map_err(|_| live_stage_error("ims_identity_unavailable"))?;
            let phone = read_live_msisdn_candidate(line_id, &conn).await?;
            self.verify(line_id).await.map_err(live_stage_error)?;
            Some(phone)
        } else { None };
        format_identity(&self.identity, &self.target, self.isim, format, phone)
    }

    pub async fn authenticate(&self, line_id: &str, rand: &[u8], autn: &[u8])
        -> Result<super::super::qmi_uim::UsimAkaApduResult, &'static str> {
        let proxy = live_runtime_config().qmi_proxy_socket;
        let device = self.device.clone();
        let rand = rand.to_vec();
        let autn = autn.to_vec();
        uicc_ims::authenticate_bound_with(&self.card, &self.aid, || self.observe(line_id), |aid| async move {
            tokio::task::spawn_blocking(move || {
                // TS 31.103 ISIM AKA and TS 31.102 USIM 3G AKA both use P2=81.
                // No application fallback on an authentication error.
                if !device.pcsc_reader.is_empty() {
                    return crate::hardware::devices::pcsc::authenticate_with_aid(&device.pcsc_reader, &aid, &rand, &autn);
                }
                execute_usim_authenticate_via_proxy_reason_with_retry(
                    &proxy, &device.qmi_device, device.uim_slot, &aid, &rand, &autn,
                    LIVE_SIM_AUTH_ATTEMPTS, LIVE_SIM_AUTH_TIMEOUT, LIVE_SIM_AUTH_RETRY_DELAY,
                )
            }).await.map_err(|_| "sim_auth_runtime_failed")?
        }).await
    }
}

fn format_identity(
    base: &LiveImsRegisterIdentity, target: &LiveImsTarget, isim: bool,
    format: LiveRegisterIdentityFormat, phone: Option<String>,
) -> Result<LiveImsRegisterIdentity, LiveStageError> {
    // Complete ISIM identity is immutable across the entire candidate ladder.
    if isim { return Ok(base.clone()); }
    let mut identity = base.clone();
    let imsi = base.contact_user.as_str();
    match format {
        LiveRegisterIdentityFormat::ImsiHomeDomain => {},
        LiveRegisterIdentityFormat::PrefixedImsiHomeDomain => {
            identity.shared.private_user = format!("0{imsi}@{}", target.realm);
            identity.shared.public_uri = format!("sip:0{imsi}@{}", target.domain);
            identity.shared.contact_user = format!("0{imsi}");
            identity.shape = "prefixed_imsi_home_domain";
        }
        LiveRegisterIdentityFormat::ImsiPhoneUri => {
            identity.shared.public_uri = format!("sip:{imsi}@{};user=phone", target.domain);
            identity.shared.contact_user_phone = true;
            identity.shape = "imsi_phone_uri";
        }
        LiveRegisterIdentityFormat::MsisdnPhoneUri => {
            let phone = phone.ok_or_else(|| live_stage_error("ims_msisdn_unavailable"))?;
            identity.shared.public_uri = format!("sip:{phone}@{};user=phone", target.domain);
            identity.shared.contact_user = phone;
            identity.shared.contact_user_phone = true;
            identity.shape = "msisdn_phone_uri";
        }
    }
    Ok(identity)
}

pub(super) async fn load(line_id: &str, profile: &'static CarrierProfile)
    -> Result<Arc<LiveImsSelection>, LiveStageError> {
    let device = sim_device_for_line(line_id);
    let endpoint = if device.pcsc_reader.is_empty() { &device.qmi_device } else { &device.pcsc_reader };
    let card = uicc_ims::capture_card_binding(endpoint, device.uim_slot, &device.modem_path)
        .await.map_err(live_stage_error)?;
    let proxy = live_runtime_config().qmi_proxy_socket;
    let reader = device.clone();
    let (applications, material) = tokio::task::spawn_blocking(move || {
        if !reader.pcsc_reader.is_empty() {
            let (aids, material) = crate::hardware::devices::pcsc::read_ims_uicc(&reader.pcsc_reader)?;
            return Ok((crate::connectivity::modems::ims::cellular_ims::identity::UiccApplications::from_aids(aids)?, material));
        }
        let applications = crate::connectivity::modems::ims::cellular_ims::identity::UiccApplications::from_aids(
            super::super::qmi_uim::read_uicc_application_aids_via_proxy_reason(
                &proxy, &reader.qmi_device, reader.uim_slot, Duration::from_secs(3),
            )?
        )?;
        let material = applications.isim_aid.as_ref().map(|aid| {
            super::super::qmi_uim::read_isim_ims_material_via_proxy_reason(
                &proxy, &reader.qmi_device, reader.uim_slot, aid, Duration::from_secs(3),
            )
        }).transpose()?;
        Ok::<_, &'static str>((applications, material))
    }).await.map_err(|_| live_stage_error("isim_read_task_failed"))?.map_err(live_stage_error)?;
    let imsi = effective_imsi_for_line(line_id, &card.imsi);
    if !imsi.starts_with(profile.meta.plmn) { return Err(live_stage_error("ims_identity_profile_mismatch")); }
    let mut target = live_ims_target(line_id, profile);
    let overrides = line_overrides(line_id);
    let explicit = !profiles::is_standard_derived_profile(profile);
    let usim_aid = applications.usim_aid.as_deref().unwrap_or(USIM_AID_PREFIX);
    let resolved = uicc_ims::resolve_ims_identity(
        &imsi, usim_aid, applications.isim_aid.as_deref().zip(material.as_ref()),
        &target.domain, &target.realm,
        explicit || overrides.ims_domain.is_some(), explicit || overrides.ims_realm.is_some(),
    ).map_err(live_stage_error)?;
    target.domain = resolved.identity.home_domain.clone();
    target.realm = resolved.realm;
    let isim = resolved.source == ImsIdentitySource::Isim;
    let selected = Arc::new(LiveImsSelection {
        identity: LiveImsRegisterIdentity { shared: resolved.identity, shape: if isim { "isim" } else { "imsi_home_domain" } },
        target, device, card, aid: resolved.auth_aid, isim,
    });
    selected.verify(line_id).await.map_err(live_stage_error)?;
    Ok(selected)
}

#[cfg(test)]
#[path = "live_identity_tests.rs"]
mod tests;
