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
        if self.iccid.is_empty() || self.imsi.is_empty() || self.endpoint.is_empty() || self.slot == 0
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
    #[cfg(test)]
    if let Some(binding) = test_bindings().lock().unwrap().get(&(endpoint.to_owned(), slot, modem_path.to_owned())).cloned() {
        return Ok(binding);
    }
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
        let properties = read_mm_properties(&conn, &before, &modem_path, "org.freedesktop.ModemManager1.Modem").await?;
        let (current_slot, ports, sim_path) = mm_card_location(&properties)?;
        if current_slot != slot || !ports.iter().any(|(port, _)| {
            port == endpoint || format!("/dev/{port}") == endpoint
        }) { return Err("ims_uicc_binding_changed"); }
        let sim = read_mm_properties(&conn, &before, sim_path.as_str(), "org.freedesktop.ModemManager1.Sim").await?;
        let iccid = mm_string(&sim, "SimIdentifier")?.ok_or("ims_uicc_binding_unavailable")?;
        let advertised_imsi = mm_string(&sim, "Imsi")?.filter(|value| !value.is_empty());
        let imsi = if let Some(imsi) = advertised_imsi.as_ref() { imsi.clone() } else {
            // Keep the existing read-only identity fallback reachable. The
            // unique owner, slot, SIM path and ICCID are rechecked afterward.
            let output = crate::hardware::cellular::control::at_command(&modem_path, "AT+CIMI").await;
            if let Some(imsi) = output.ok().and_then(|output| super::cellular_ims::identity::parse_cimi_response(&output)) {
                imsi
            } else {
                let endpoint = endpoint.to_owned();
                tokio::task::spawn_blocking(move || {
                    let aids = super::vowifi::qmi_uim::read_uicc_application_aids_via_proxy_reason("@qmi-proxy", &endpoint, slot, std::time::Duration::from_secs(3))?;
                    let apps = super::cellular_ims::identity::UiccApplications::from_aids(aids)?;
                    let aid = apps.usim_aid.ok_or("ims_uicc_binding_unavailable")?;
                    super::vowifi::qmi_uim::read_usim_identity_via_proxy_reason("@qmi-proxy", &endpoint, slot, &aid, std::time::Duration::from_secs(3)).map(|identity| identity.imsi)
                }).await.map_err(|_| "ims_uicc_binding_unavailable")??
            }
        };
        let after_properties = read_mm_properties(&conn, &before, &modem_path, "org.freedesktop.ModemManager1.Modem").await?;
        let after_sim = read_mm_properties(&conn, &before, sim_path.as_str(), "org.freedesktop.ModemManager1.Sim").await?;
        if mm_card_location(&after_properties)? != (current_slot, ports, sim_path)
            || mm_string(&after_sim, "SimIdentifier")?.as_deref() != Some(iccid.as_str())
            || mm_string(&after_sim, "Imsi")?.filter(|value| !value.is_empty()).is_some_and(|value| value != imsi)
        { return Err("ims_uicc_binding_changed"); }
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

type MmProperties = std::collections::HashMap<String, zbus::zvariant::OwnedValue>;

async fn read_mm_properties(conn: &zbus::Connection, owner: &str, path: &str, interface: &str) -> Result<MmProperties, &'static str> {
    zbus::proxy::Builder::<zbus::Proxy<'_>>::new(conn)
        .destination(owner).map_err(|_| "ims_uicc_binding_unavailable")?
        .path(path).map_err(|_| "ims_uicc_binding_unavailable")?
        .interface("org.freedesktop.DBus.Properties").map_err(|_| "ims_uicc_binding_unavailable")?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build().await.map_err(|_| "ims_uicc_binding_unavailable")?
        .call("GetAll", &(interface,)).await.map_err(|_| "ims_uicc_binding_unavailable")
}

fn mm_string(properties: &MmProperties, key: &str) -> Result<Option<String>, &'static str> {
    properties.get(key).map(|value| <&str>::try_from(value).map(str::to_owned)
        .map_err(|_| "ims_uicc_binding_unavailable")).transpose()
}

fn mm_slot(properties: &MmProperties) -> Result<u8, &'static str> {
    match properties.get("PrimarySimSlot") {
        None => Ok(1),
        Some(value) => match u32::try_from(value).map_err(|_| "ims_uicc_binding_unavailable")? {
            0 => Ok(1),
            slot @ 1..=255 => Ok(slot as u8),
            _ => Err("ims_uicc_binding_unavailable"),
        },
    }
}

fn mm_card_location(properties: &MmProperties) -> Result<(u8, Vec<(String, u32)>, zbus::zvariant::OwnedObjectPath), &'static str> {
    let ports = properties.get("Ports").and_then(|value| value.try_clone().ok())
        .and_then(|value| Vec::<(String, u32)>::try_from(value).ok()).ok_or("ims_uicc_binding_unavailable")?;
    let sim = properties.get("Sim").and_then(|value| value.try_clone().ok())
        .and_then(|value| zbus::zvariant::OwnedObjectPath::try_from(value).ok()).ok_or("ims_uicc_binding_unavailable")?;
    if sim.as_str() == "/" { return Err("ims_uicc_binding_unavailable"); }
    Ok((mm_slot(properties)?, ports, sim))
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
type TestBindings = std::collections::HashMap<(String, u8, String), UiccCardBinding>;
#[cfg(test)]
fn test_bindings() -> &'static std::sync::Mutex<TestBindings> {
    static BINDINGS: std::sync::OnceLock<std::sync::Mutex<TestBindings>> = std::sync::OnceLock::new();
    BINDINGS.get_or_init(Default::default)
}
#[cfg(test)]
pub(crate) fn register_test_binding(modem_path: &str, binding: UiccCardBinding) {
    test_bindings().lock().unwrap().insert((binding.endpoint.clone(), binding.slot, modem_path.to_owned()), binding);
}

#[cfg(test)]
#[path = "uicc_ims_tests.rs"]
mod tests;
