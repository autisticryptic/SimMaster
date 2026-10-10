//! Session-local UICC IMS selection. Never merges an ISIM username with a
//! USIM authentication application, and never stores material in a global cache.
use crate::connectivity::core::context::ImsIdentity;
use super::vowifi::qmi_uim::{isim, IsimImsMaterial, USIM_AID_PREFIX};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImsIdentitySource { Isim, UsimDerived }

#[derive(Clone)]
pub struct ResolvedImsIdentity {
    pub identity: ImsIdentity,
    pub realm: String,
    pub auth_aid: Vec<u8>,
    pub source: ImsIdentitySource,
    pub isim_pcscf: Vec<String>,
}

/// Explicit catalog/database/line values are policy; only a standard-derived
/// domain/realm may be replaced. Identity strings themselves are not rewritten
/// to fit that policy. Partial, but well-formed provisioning is unconfigured.
pub fn resolve_ims_identity(
    imsi: &str,
    usim_aid: &[u8],
    isim: Option<(&[u8], &IsimImsMaterial)>,
    domain: &str,
    realm: &str,
    domain_explicit: bool,
    realm_explicit: bool,
) -> Result<ResolvedImsIdentity, &'static str> {
    if !(5..=16).contains(&imsi.len()) || !imsi.bytes().all(|b| b.is_ascii_digit())
        || !usim_aid.starts_with(USIM_AID_PREFIX) || usim_aid.len() > 16 {
        return Err("ims_uicc_binding_invalid");
    }
    let mut resolved = ResolvedImsIdentity {
        identity: ImsIdentity {
            private_user: format!("{imsi}@{realm}"),
            public_uri: format!("sip:{imsi}@{domain}"),
            contact_user: imsi.to_owned(),
            home_domain: domain.to_owned(),
            contact_user_phone: false,
        },
        realm: realm.to_owned(),
        auth_aid: usim_aid.to_vec(),
        source: ImsIdentitySource::UsimDerived,
        isim_pcscf: Vec::new(),
    };
    if let Some((aid, material)) = isim {
        isim::validate_isim_aid(aid)?;
        // Public material fields are constructible by adapters/tests. Revalidate
        // before any value can enter a SIP header, even when incomplete.
        isim::validate_material(material)?;
        resolved.isim_pcscf = material.pcscf.clone();
        if let (Some(impi), Some(isim_domain), Some(impu)) =
            (&material.impi, &material.domain, material.impus.first()) {
            resolved.identity.private_user = impi.clone();
            resolved.identity.public_uri = impu.clone();
            resolved.identity.contact_user = impi.split_once('@').ok_or("isim_identity_invalid")?.0.to_owned();
            if !domain_explicit { resolved.identity.home_domain = isim_domain.clone(); }
            if !realm_explicit { resolved.realm = isim_domain.clone(); }
            resolved.auth_aid = aid.to_vec();
            resolved.source = ImsIdentitySource::Isim;
        }
    }
    Ok(resolved)
}

/// Captured physical subscription, compared at read and authentication
/// boundaries. Reader paths alone are not card identities. Debug is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct UiccCardBinding {
    pub endpoint: String,
    pub slot: u8,
    pub iccid: String,
    pub imsi: String,
    pub owner: String,
}
impl std::fmt::Debug for UiccCardBinding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UiccCardBinding(<redacted>)")
    }
}
impl UiccCardBinding {
    pub fn verify(&self, current: &Self) -> Result<(), &'static str> {
        if self.iccid.is_empty() || self.imsi.is_empty() || self.endpoint.is_empty()
            || self.owner.is_empty() || self != current {
            Err("ims_uicc_binding_changed")
        } else { Ok(()) }
    }
}

/// Read subscription/owner evidence without a reader-only cache. Native I/O
/// verifies its physical owner under the existing operation gate; MM ownership
/// is bracketed by unique bus-name reads. Callers additionally retain their
/// task/worker generation for the full registration lifetime.
pub async fn capture_card_binding(
    endpoint: &str, slot: u8, modem_path: &str,
) -> Result<UiccCardBinding, &'static str> {
    if endpoint.starts_with("pcsc://") {
        let sim = crate::hardware::devices::pcsc::read_identity_async(endpoint)
            .await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let binding = UiccCardBinding {
            endpoint: endpoint.to_owned(), slot,
            iccid: crate::platform::utils::normalize_iccid(&sim.iccid), imsi: sim.imsi,
            owner: format!("pcsc-reader:{endpoint}"),
        };
        binding.verify(&binding)?;
        return Ok(binding);
    }
    if endpoint.is_empty() || modem_path.is_empty() || slot == 0 {
        return Err("ims_uicc_binding_invalid");
    }
    let (iccid, imsi, owner) = if let Some(fleet) = crate::hardware::cellular::backends::active_native() {
        let device = fleet.by_control_device(endpoint).map_err(|_| "ims_uicc_binding_unavailable")?;
        if device.spec.uim_slot != slot { return Err("ims_uicc_binding_changed"); }
        let sim = device.sim_identity().await.map_err(|_| "ims_uicc_binding_unavailable")?;
        (sim.iccid, sim.imsi, device.controller_instance.clone())
    } else {
        let conn = zbus::Connection::system().await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let bus = zbus::fdo::DBusProxy::new(&conn).await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let before = bus.get_name_owner("org.freedesktop.ModemManager1".try_into().map_err(|_| "ims_uicc_binding_unavailable")?)
            .await.map_err(|_| "ims_uicc_binding_unavailable")?.to_string();
        let modem_path = if modem_path.starts_with('/') { modem_path.to_owned() }
            else if modem_path.bytes().all(|b| b.is_ascii_digit()) {
                format!("/org/freedesktop/ModemManager1/Modem/{modem_path}")
            } else { return Err("ims_uicc_binding_invalid"); };
        let modem = zbus::Proxy::new(&conn, before.as_str(), modem_path.as_str(), "org.freedesktop.ModemManager1.Modem")
            .await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let current_slot: u32 = modem.get_property("PrimarySimSlot").await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let ports: Vec<(String, u32)> = modem.get_property("Ports").await.map_err(|_| "ims_uicc_binding_unavailable")?;
        if current_slot != u32::from(slot) || !ports.iter().any(|(port, _)| {
            port == endpoint || format!("/dev/{port}") == endpoint
        }) { return Err("ims_uicc_binding_changed"); }
        let sim_path: zbus::zvariant::OwnedObjectPath = modem.get_property("Sim").await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let sim = zbus::Proxy::new(&conn, before.as_str(), sim_path.as_str(), "org.freedesktop.ModemManager1.Sim")
            .await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let iccid: String = sim.get_property("SimIdentifier").await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let imsi: String = sim.get_property("Imsi").await.map_err(|_| "ims_uicc_binding_unavailable")?;
        let after = bus.get_name_owner("org.freedesktop.ModemManager1".try_into().map_err(|_| "ims_uicc_binding_unavailable")?)
            .await.map_err(|_| "ims_uicc_binding_unavailable")?.to_string();
        if before != after { return Err("ims_uicc_binding_changed"); }
        (iccid, imsi, before.clone())
    };
    let binding = UiccCardBinding {
        endpoint: endpoint.to_owned(), slot,
        iccid: crate::platform::utils::normalize_iccid(&iccid), imsi, owner,
    };
    binding.verify(&binding)?;
    Ok(binding)
}

/// One bound AKA exchange, with no application fallback and no usable result
/// after an observed swap. Adapters supply owner-aware fresh observations.
pub async fn authenticate_bound_with<T, Observe, Observation, Authenticate, Authentication>(
    expected: &UiccCardBinding,
    aid: &[u8],
    mut observe: Observe,
    authenticate: Authenticate,
) -> Result<T, &'static str>
where
    Observe: FnMut() -> Observation,
    Observation: std::future::Future<Output = Result<UiccCardBinding, &'static str>>,
    Authenticate: FnOnce(Vec<u8>) -> Authentication,
    Authentication: std::future::Future<Output = Result<T, &'static str>>,
{
    expected.verify(&observe().await?)?;
    let result = authenticate(aid.to_vec()).await;
    expected.verify(&observe().await?)?;
    result
}

#[cfg(test)]
#[path = "uicc_ims_tests.rs"]
mod tests;
