//! Read-only, subscription-bound IMS provisioning snapshots. This detects
//! changes at registration procedure boundaries; it is not a UICC REFRESH
//! indication subscriber and cannot prove the absence of a physical race.
use super::UiccCardBinding;
use crate::connectivity::modems::ims::{
    cellular_ims::identity::UiccApplications,
    vowifi::qmi_uim::{self, IsimImsMaterial, USIM_AID_PREFIX},
};
use std::{future::Future, time::Duration};

#[derive(Clone, PartialEq, Eq)]
pub struct UiccImsProvisioning {
    pub applications: UiccApplications,
    pub material: Option<IsimImsMaterial>,
}
impl std::fmt::Debug for UiccImsProvisioning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UiccImsProvisioning(<redacted>)")
    }
}
impl UiccImsProvisioning {
    pub fn new(applications: UiccApplications, material: Option<IsimImsMaterial>) -> Result<Self, &'static str> {
        if applications.usim_aid.as_ref().is_some_and(|aid| !aid.starts_with(USIM_AID_PREFIX) || aid.len() > 16) {
            return Err("ims_uicc_provisioning_invalid");
        }
        if let Some(aid) = applications.isim_aid.as_deref() { qmi_uim::isim::validate_isim_aid(aid)?; }
        if applications.isim_aid.is_some() != material.is_some() { return Err("ims_uicc_provisioning_invalid"); }
        if let Some(material) = material.as_ref() { qmi_uim::isim::validate_material(material)?; }
        Ok(Self { applications, material })
    }
    pub async fn verify_bound(&self, expected: &UiccCardBinding, modem_path: &str, proxy: &str) -> Result<(), &'static str> {
        if *self != read_bound(expected, modem_path, proxy).await? { return Err("ims_uicc_provisioning_changed"); }
        Ok(())
    }
}

/// Two equal reads, bracketed by fresh physical-owner checks. A malformed,
/// failed or changing sample never becomes an unconfigured USIM fallback.
/// No spawned hardware operation is detached by an outer timeout here.
pub async fn read_bound(expected: &UiccCardBinding, modem_path: &str, proxy: &str) -> Result<UiccImsProvisioning, &'static str> {
    read_bound_with(expected,
        || super::capture_card_binding(&expected.endpoint, expected.slot, modem_path),
        || read_once(expected, modem_path, proxy)).await
}

async fn read_once(expected: &UiccCardBinding, modem_path: &str, proxy: &str) -> Result<UiccImsProvisioning, &'static str> {
    #[cfg(test)]
    if let Some(value) = samples().lock().unwrap().get(&(expected.endpoint.clone(), expected.slot, modem_path.to_owned())).cloned() {
        return value;
    }
    let _ = modem_path;
    let endpoint = expected.endpoint.clone();
    let slot = expected.slot;
    let proxy = proxy.to_owned();
    tokio::task::spawn_blocking(move || {
        let (aids, material) = if endpoint.starts_with("pcsc://") {
            crate::hardware::devices::pcsc::read_ims_uicc(&endpoint)?
        } else {
            let aids = qmi_uim::read_uicc_application_aids_via_proxy_reason(&proxy, &endpoint, slot, Duration::from_secs(3))?;
            let applications = UiccApplications::from_aids(aids.clone())?;
            let material = applications.isim_aid.as_deref().map(|aid|
                qmi_uim::read_isim_ims_material_via_proxy_reason(&proxy, &endpoint, slot, aid, Duration::from_secs(3))).transpose()?;
            (aids, material)
        };
        UiccImsProvisioning::new(UiccApplications::from_aids(aids)?, material)
    }).await.map_err(|_| "isim_read_task_failed")?
}

async fn read_bound_with<O, OF, R, RF>(expected: &UiccCardBinding, mut observe: O, mut read: R)
    -> Result<UiccImsProvisioning, &'static str>
where O: FnMut() -> OF, OF: Future<Output = Result<UiccCardBinding, &'static str>>,
      R: FnMut() -> RF, RF: Future<Output = Result<UiccImsProvisioning, &'static str>> {
    expected.verify(&observe().await?)?;
    let first = read().await;
    expected.verify(&observe().await?)?;
    let first = first?;
    // Revalidate public fields even for injected or independently authored adapters.
    UiccImsProvisioning::new(first.applications.clone(), first.material.clone())?;
    let second = read().await;
    expected.verify(&observe().await?)?;
    let second = second?;
    UiccImsProvisioning::new(second.applications.clone(), second.material.clone())?;
    if first != second { return Err("ims_uicc_provisioning_unstable"); }
    Ok(first)
}

#[cfg(test)]
type Samples = std::collections::HashMap<(String, u8, String), Result<UiccImsProvisioning, &'static str>>;
#[cfg(test)]
fn samples() -> &'static std::sync::Mutex<Samples> {
    static VALUES: std::sync::OnceLock<std::sync::Mutex<Samples>> = std::sync::OnceLock::new();
    VALUES.get_or_init(Default::default)
}
#[cfg(test)]
pub(crate) fn seed_result_for_test(modem_path: &str, card: &UiccCardBinding, value: Result<UiccImsProvisioning, &'static str>) {
    samples().lock().unwrap().insert((card.endpoint.clone(), card.slot, modem_path.to_owned()), value);
}
#[cfg(test)]
pub(crate) fn seed_for_test(modem_path: &str, card: &UiccCardBinding, value: UiccImsProvisioning) {
    seed_result_for_test(modem_path, card, Ok(value));
}
#[cfg(test)]
pub(crate) fn default_for_test(modem_path: &str, card: &UiccCardBinding) -> UiccImsProvisioning {
    let value = UiccImsProvisioning::new(UiccApplications { usim_aid: Some(USIM_AID_PREFIX.to_vec()), isim_aid: None }, None).unwrap();
    seed_for_test(modem_path, card, value.clone());
    value
}

#[cfg(test)]
#[path = "uicc_provisioning_tests.rs"]
mod tests;
