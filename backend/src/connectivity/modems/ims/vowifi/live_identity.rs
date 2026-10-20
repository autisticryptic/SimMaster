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
    provisioning: uicc_ims::provisioning::UiccImsProvisioning,
    /// Five-code software USIM. When set, the card is never read.
    virtual_usim: Option<Arc<crate::connectivity::modems::ims::virtual_sim::VirtualSim>>,
    #[cfg(test)]
    test_aka: Option<(super::super::qmi_uim::UsimAkaApduResult, Arc<std::sync::atomic::AtomicUsize>)>,
    isim: bool,
    diagnostic_identity: crate::connectivity::core::registration_diagnostics::IdentityOrigin,
    diagnostic_domain: crate::connectivity::core::registration_diagnostics::FactSource,
    diagnostic_realm: crate::connectivity::core::registration_diagnostics::FactSource,
}

impl LiveImsSelection {
    pub(super) fn diagnostic_sources(&self) -> (
        crate::connectivity::core::registration_diagnostics::IdentityOrigin,
        crate::connectivity::core::registration_diagnostics::FactSource,
        crate::connectivity::core::registration_diagnostics::FactSource,
    ) { (self.diagnostic_identity, self.diagnostic_domain, self.diagnostic_realm) }

    /// Network retry scope follows the physical subscription, not an MM owner
    /// generation or a freshly allocated selection Arc.
    pub(super) fn same_subscription(&self, other: &Self) -> bool {
        self.card.slot == other.card.slot && self.card.iccid == other.card.iccid && self.card.imsi == other.card.imsi
    }

    pub(super) async fn subscription_is_current(&self, line_id: &str) -> Result<bool, &'static str> {
        let device = sim_device_for_line(line_id);
        let endpoint = if device.pcsc_reader.is_empty() { &device.qmi_device } else { &device.pcsc_reader };
        let observed = uicc_ims::capture_card_binding(endpoint, device.uim_slot, &device.modem_path).await?;
        // Renumbered control ports or MM objects are not evidence of a new SIM.
        Ok(self.card.slot == observed.slot && self.card.iccid == observed.iccid && self.card.imsi == observed.imsi)
    }

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
        if let Some(profile) = &self.virtual_usim {
            // Configuration identity, not a card read: the line must still own
            // the same installed profile.
            return match crate::connectivity::modems::ims::virtual_sim::for_line(line_id) {
                Some(current) if Arc::ptr_eq(&current, profile) => Ok(()),
                _ => Err("ims_virtual_sim_changed"),
            };
        }
        self.card.verify(&self.observe(line_id).await?)
    }

    pub async fn verify_registration(&self, line_id: &str) -> Result<(), &'static str> {
        if self.virtual_usim.is_some() { return self.verify(line_id).await; }
        self.verify_reader_mapping(line_id)?;
        self.provisioning.verify_bound(&self.card, &self.device.modem_path,
            &live_runtime_config().qmi_proxy_socket).await?;
        self.verify_reader_mapping(line_id)
    }

    pub async fn for_format(&self, line_id: &str, format: LiveRegisterIdentityFormat)
        -> Result<LiveImsRegisterIdentity, LiveStageError> {
        self.verify_registration(line_id).await.map_err(live_stage_error)?;
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
        self.verify_registration(line_id).await?;
        if let Some(profile) = &self.virtual_usim {
            // Five-code software USIM: compute the card-side AKA locally.
            let out = profile.authenticate(rand, autn)?;
            return Ok(super::super::qmi_uim::UsimAkaApduResult {
                res: out.res, ck: out.ck, ik: out.ik, auts: out.auts,
            });
        }
        let proxy = live_runtime_config().qmi_proxy_socket;
        let device = self.device.clone();
        let rand = rand.to_vec();
        let autn = autn.to_vec();
        #[cfg(test)]
        let test_aka = self.test_aka.clone();
        let result = uicc_ims::authenticate_bound_with(&self.card, &self.aid, || self.observe(line_id), |aid| async move {
            // Inject only the card's result in no-device tests. The real
            // before/after reader, slot, owner and material validation remains.
            #[cfg(test)]
            if let Some((aka, calls)) = test_aka {
                calls.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                return Ok(aka);
            }
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
        }).await;
        self.verify_registration(line_id).await?;
        result
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
    if let Some(virtual_usim) = crate::connectivity::modems::ims::virtual_sim::for_line(line_id) {
        return load_virtual(line_id, profile, virtual_usim);
    }
    let device = sim_device_for_line(line_id);
    let endpoint = if device.pcsc_reader.is_empty() { &device.qmi_device } else { &device.pcsc_reader };
    let card = uicc_ims::capture_card_binding(endpoint, device.uim_slot, &device.modem_path)
        .await.map_err(live_stage_error)?;
    let provisioning = uicc_ims::provisioning::read_bound(&card, &device.modem_path,
        &live_runtime_config().qmi_proxy_socket).await.map_err(live_stage_error)?;
    let applications = &provisioning.applications;
    let material = &provisioning.material;
    // One immutable settings snapshot supplies both the actual selection and
    // its source evidence; a concurrent override update cannot relabel it.
    let overrides = line_overrides(line_id);
    let imsi_override = overrides.effective_imsi.as_ref().filter(|value| !value.is_empty());
    let imsi = imsi_override.cloned().unwrap_or_else(|| card.imsi.trim().to_owned());
    if !imsi.starts_with(profile.meta.plmn) { return Err(live_stage_error("ims_identity_profile_mismatch")); }
    let mut target = live_ims_target_from_overrides(profile, overrides.clone());
    let explicit = !profiles::is_standard_derived_profile(profile);
    let usim_aid = applications.usim_aid.as_deref().unwrap_or(USIM_AID_PREFIX);
    let resolved = uicc_ims::resolve_ims_identity(
        &imsi, usim_aid, applications.isim_aid.as_deref().zip(material.as_ref()),
        &target.domain, &target.realm,
        explicit || overrides.ims_domain.is_some(), explicit || overrides.ims_realm.is_some(),
        uicc_ims::IdentitySourcePolicy::parse(profile.ims.identity_source),
    ).map_err(live_stage_error)?;
    target.domain = resolved.identity.home_domain.clone();
    target.realm = resolved.realm;
    let isim = resolved.source == ImsIdentitySource::Isim;
    use crate::connectivity::core::registration_diagnostics::{FactSource, IdentityOrigin};
    let baseline = super::diagnostics_live::profile_source(line_id, profile);
    let diagnostic_identity = if isim { IdentityOrigin::Isim } else if imsi_override.is_some() { IdentityOrigin::SimOverrideDerived } else { IdentityOrigin::UsimDerived };
    let diagnostic_domain = if overrides.ims_domain.is_some() { FactSource::SimOverride } else if isim && !explicit { FactSource::Isim } else { baseline };
    let diagnostic_realm = if overrides.ims_realm.is_some() { FactSource::SimOverride } else if isim && !explicit { FactSource::Isim } else { baseline };
    let selected = Arc::new(LiveImsSelection {
        identity: LiveImsRegisterIdentity { shared: resolved.identity, shape: if isim { "isim" } else { "imsi_home_domain" } },
        target, device, card, aid: resolved.auth_aid, provisioning, virtual_usim: None, isim, diagnostic_identity, diagnostic_domain, diagnostic_realm,
        #[cfg(test)]
        test_aka: None,
    });
    selected.verify(line_id).await.map_err(live_stage_error)?;
    Ok(selected)
}

/// Identity for a five-code subscription: no reader, no QMI/PCSC read. The
/// derived identity follows the same standard rules as a USIM without ISIM.
fn load_virtual(line_id: &str, profile: &'static CarrierProfile,
    virtual_usim: Arc<crate::connectivity::modems::ims::virtual_sim::VirtualSim>)
    -> Result<Arc<LiveImsSelection>, LiveStageError> {
    let overrides = line_overrides(line_id);
    let mut target = live_ims_target_from_overrides(profile, overrides.clone());
    let explicit = !profiles::is_standard_derived_profile(profile);
    let imsi = virtual_usim.codes().imsi.clone();
    if !imsi.starts_with(profile.meta.plmn) {
        return Err(live_stage_error("ims_identity_profile_mismatch"));
    }
    let resolved = uicc_ims::resolve_ims_identity(
        &imsi, USIM_AID_PREFIX, None, &target.domain, &target.realm,
        explicit || overrides.ims_domain.is_some(), explicit || overrides.ims_realm.is_some(),
        uicc_ims::IdentitySourcePolicy::UsimDerived,
    ).map_err(live_stage_error)?;
    target.domain = resolved.identity.home_domain.clone();
    target.realm = resolved.realm;
    let card = UiccCardBinding {
        endpoint: format!("virtual:{}", virtual_usim.profile_id()),
        slot: 1,
        owner: "virtual".into(),
        iccid: virtual_usim.codes().iccid.clone(),
        imsi: imsi.clone(),
    };
    let provisioning = uicc_ims::provisioning::UiccImsProvisioning::new(
        crate::connectivity::modems::ims::cellular_ims::identity::UiccApplications {
            usim_aid: Some(USIM_AID_PREFIX.to_vec()), isim_aid: None,
        }, None).map_err(live_stage_error)?;
    use crate::connectivity::core::registration_diagnostics::{FactSource, IdentityOrigin};
    Ok(Arc::new(LiveImsSelection {
        identity: LiveImsRegisterIdentity { shared: resolved.identity, shape: "virtual_five_codes" },
        target,
        device: sim_device_for_line(line_id),
        card,
        aid: USIM_AID_PREFIX.to_vec(),
        provisioning,
        virtual_usim: Some(virtual_usim),
        #[cfg(test)]
        test_aka: None,
        isim: false,
        diagnostic_identity: IdentityOrigin::UsimDerived,
        diagnostic_domain: FactSource::Unknown,
        diagnostic_realm: FactSource::Unknown,
    }))
}

#[cfg(test)]
pub(super) fn selection_for_test(
    line_id: &str, identity: LiveImsRegisterIdentity, target: LiveImsTarget,
    card: UiccCardBinding,
) -> Arc<LiveImsSelection> {
    let device = LiveSimDevice {
        qmi_device: card.endpoint.clone(), uim_slot: card.slot,
        modem_path: format!("synthetic-modem-{line_id}"), pcsc_reader: String::new(),
    };
    register_line_sim_device(line_id, &device.qmi_device, device.uim_slot, &device.modem_path);
    uicc_ims::register_test_binding(&device.modem_path, card.clone());
    let provisioning = uicc_ims::provisioning::default_for_test(&device.modem_path, &card);
    Arc::new(LiveImsSelection { identity, target, device, card, aid: USIM_AID_PREFIX.to_vec(), provisioning, virtual_usim: None, test_aka: None, isim: false,
        diagnostic_identity: crate::connectivity::core::registration_diagnostics::IdentityOrigin::UsimDerived,
        diagnostic_domain: crate::connectivity::core::registration_diagnostics::FactSource::Unknown,
        diagnostic_realm: crate::connectivity::core::registration_diagnostics::FactSource::Unknown })
}

#[cfg(test)]
pub(super) fn with_aka_for_test(
    selected: &Arc<LiveImsSelection>, aka: super::super::qmi_uim::UsimAkaApduResult,
    calls: Arc<std::sync::atomic::AtomicUsize>,
) -> Arc<LiveImsSelection> {
    let mut selected = (**selected).clone();
    selected.test_aka = Some((aka, calls));
    Arc::new(selected)
}

#[cfg(test)]
pub(super) fn seed_provisioning_for_test(selected: &LiveImsSelection,
    value: Result<uicc_ims::provisioning::UiccImsProvisioning, &'static str>) {
    uicc_ims::provisioning::seed_result_for_test(&selected.device.modem_path, &selected.card, value);
}

#[cfg(test)]
#[path = "live_identity_tests.rs"]
mod tests;
