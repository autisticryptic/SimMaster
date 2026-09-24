//! Retained native QMI/MBIM sessions, never MM bearer objects.
//!
//! The selected endpoint and dedicated netdev must be explicitly described.
//! Receipts block blind reuse after a crash; ambiguous firmware state is a
//! maintenance/reconciliation error, not permission to steal an interface.

use serde::{Deserialize, Serialize};
use std::{
    net::IpAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{oneshot, Mutex};

use super::{
    config::{NativeBearerConfig, NativeProtocol},
    native::NativeDevice,
    protocol::{labelled, safe_parameter, CommandRequest},
    NativeError,
};
use crate::{
    connectivity::modems::ims::cellular_ims::{
        bearer::{configure_data_bearer_network_in_worker, BearerConnection},
        native_bearer, CellularImsError,
    },
    hardware::{
        cellular::qmi_wds,
        devices::transport::{
            BearerDomain, BearerInterfaceOwnership, CellularDataTransport, ImsBearerError,
            ImsBearerErrorKind, ImsBearerFailureHint, ImsBearerHandle, ImsBearerInfo,
            ImsBearerTransport, ThreeGppRat, TransportFuture,
        },
    },
    platform::config::ApnConfig,
    services::ue_worker::{worker_for_line, NetConfigOp, UeWorkerBinding},
};

#[derive(Debug, Clone, Copy)]
enum Role {
    Ims,
    Data,
}
impl Role {
    fn label(self) -> &'static str {
        match self {
            Self::Ims => "ims",
            Self::Data => "data",
        }
    }
    fn endpoint(self, device: &NativeDevice) -> Option<NativeBearerConfig> {
        match self {
            Self::Ims => device.spec.ims.clone(),
            Self::Data => device.spec.data.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Client {
    cid: Option<u8>,
    packet_handle: Option<u32>,
    mbim_session: Option<u8>,
}

#[derive(Serialize)]
struct Receipt<'a> {
    schema: u32,
    owner_pid: u32,
    physical_key: &'a str,
    control_device: &'a str,
    interface: &'a str,
    namespace: &'a str,
    clients: &'a [Client],
}

struct Session {
    device: Arc<NativeDevice>,
    endpoint: NativeBearerConfig,
    role: Role,
    clients: Vec<Client>,
    receipt: String,
    namespace: String,
    lost: Arc<AtomicBool>,
    monitor: Option<tokio::task::JoinHandle<()>>,
}

fn receipt_path(device: &NativeDevice, role: Role) -> String {
    format!("session-{}-{}", device.spec.line_id(), role.label())
}

impl Session {
    fn save(&self, create: bool) -> Result<(), NativeError> {
        let bytes = serde_json::to_vec(&Receipt {
            schema: 1,
            owner_pid: std::process::id(),
            physical_key: &self.device.spec.hardware_key,
            control_device: &self.endpoint.control_device,
            interface: &self.endpoint.interface,
            namespace: &self.namespace,
            clients: &self.clients,
        })
        .map_err(|_| NativeError::Protocol("native_session_receipt_encode_failed".into()))?;
        self.device.io.save_receipt(&self.receipt, &bytes, create)
    }

    async fn cleanup_locked(&mut self) -> bool {
        if let Some(monitor) = self.monitor.take() {
            monitor.abort();
        }
        let mut clean = true;
        for index in (0..self.clients.len()).rev() {
            let client = self.clients[index].clone();
            let mut released = true;
            if let Some(cid) = client.cid {
                if let Some(handle) = client.packet_handle {
                    let disconnected = self
                        .device
                        .io
                        .execute(&qmi_request(
                            &self.endpoint,
                            Some(cid),
                            "--wds-get-packet-service-status",
                            true,
                        ))
                        .await
                        .ok()
                        .is_some_and(|text| {
                            labelled(&text, "Connection status") == Some("disconnected")
                        });
                    let request = qmi_request(
                        &self.endpoint,
                        Some(cid),
                        &format!("--wds-stop-network={handle}"),
                        true,
                    );
                    if !disconnected && self.device.io.execute(&request).await.is_err() {
                        clean = false;
                        continue;
                    }
                }
                if self
                    .device
                    .io
                    .execute(&qmi_request(&self.endpoint, Some(cid), "--wds-noop", false))
                    .await
                    .is_err()
                {
                    released = false;
                }
            } else if let Some(id) = client.mbim_session {
                let request = mbim_request(&self.endpoint, &format!("--disconnect={id}"));
                if self.device.io.execute(&request).await.is_err() {
                    released = false;
                }
            } else {
                released = false;
            }
            if released {
                // Retire this numeric ID in memory BEFORE persisting. A failed
                // receipt write must never make a later cleanup reuse a CID
                // which the modem has already returned to its allocator.
                self.clients.remove(index);
                if self.save(false).is_err() { clean = false; break; }
            } else { clean = false; }
        }
        // Stopping WDS/MBIM is not proof that a moved interface came home.
        // Cancellation, a failed move or a stale worker can all leave the
        // network side unresolved. Only explicit, verified restore clears
        // this intent; Drop must not erase it after stopping the session.
        clean &= self.namespace.is_empty();
        if clean {
            clean = self.device.io.clear_receipt(&self.receipt).is_ok();
            if clean {
                self.device
                    .active_interfaces
                    .lock()
                    .unwrap()
                    .remove(self.role.label());
            }
        }
        if !clean {
            tracing::warn!(line_id = %self.device.spec.line_id(), role = self.role.label(), namespace_pending = !self.namespace.is_empty(), "Native cleanup incomplete; retaining ownership receipt");
        }
        clean
    }

    async fn release(mut self) {
        let operation = self.device.operation.clone();
        let _guard = operation.lock().await;
        self.cleanup_locked().await;
    }

    fn start_monitor(&mut self) {
        let device = self.device.clone();
        let endpoint = self.endpoint.clone();
        let clients = self.clients.clone();
        let lost = self.lost.clone();
        self.monitor = Some(tokio::spawn(async move {
            let mut errors = 0_u8;
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                let requests = clients
                    .iter()
                    .map(|client| {
                        if let Some(cid) = client.cid {
                            qmi_request(
                                &endpoint,
                                Some(cid),
                                "--wds-get-packet-service-status",
                                true,
                            )
                        } else {
                            mbim_request(
                                &endpoint,
                                &format!(
                                    "--query-connection-state={}",
                                    client.mbim_session.unwrap_or(0)
                                ),
                            )
                        }
                    })
                    .collect();
                match device.commands(requests).await {
                    Ok(values) => {
                        if values.iter().any(|s| {
                            labelled(s, "Connection status") == Some("disconnected")
                                || labelled(s, "Activation state") == Some("deactivated")
                        }) {
                            lost.store(true, Ordering::Release);
                            break;
                        }
                        if values.iter().all(|s| {
                            labelled(s, "Connection status") == Some("connected")
                                || labelled(s, "Activation state") == Some("activated")
                        }) {
                            errors = 0;
                        } else {
                            errors = errors.saturating_add(1);
                            if errors >= 3 {
                                lost.store(true, Ordering::Release);
                                break;
                            }
                        }
                    }
                    Err(_) => {
                        errors = errors.saturating_add(1);
                        if errors >= 3 {
                            lost.store(true, Ordering::Release);
                            break;
                        }
                    }
                }
            }
        }));
    }
}

fn qmi_request(
    endpoint: &NativeBearerConfig,
    cid: Option<u8>,
    action: &str,
    retain: bool,
) -> CommandRequest {
    let mut request = CommandRequest::query(NativeProtocol::Qmi, &endpoint.control_device, action);
    if let Some(cid) = cid {
        request.arguments.push(format!("--client-cid={cid}"));
    }
    if retain {
        request.arguments.push("--client-no-release-cid".into());
    }
    request.timeout_seconds = 90;
    request
}

fn mbim_request(endpoint: &NativeBearerConfig, action: &str) -> CommandRequest {
    let mut request = CommandRequest::query(NativeProtocol::Mbim, &endpoint.control_device, action);
    request.arguments.push("--no-close".into());
    request.timeout_seconds = 90;
    request
}

fn qmi_start(apn: &ApnConfig, family: u8, profile_id: Option<u32>) -> Result<String, NativeError> {
    let name = safe_parameter(apn.apn.trim())?;
    if name.is_empty() || !matches!(family, 4 | 6) || profile_id.is_some_and(|id| id > 255) {
        return Err(NativeError::Protocol(
            "native_qmi_bearer_parameters_invalid".into(),
        ));
    }
    // qmicli's WDS grammar takes 4/6, unlike MM and MBIM's ipv4/ipv6 names.
    let mut action = format!("--wds-start-network=apn={name},ip-type={family}");
    if let Some(profile) = profile_id {
        action.push_str(&format!(",3gpp-profile={profile}"));
    }
    if !apn.username.is_empty() || !apn.password.is_empty() {
        let auth = match apn.auth_method.to_ascii_lowercase().as_str() {
            "pap" => "PAP",
            "both" | "pap-or-chap" => "BOTH",
            _ => "CHAP",
        };
        action.push_str(&format!(
            ",username={},password={},auth={auth}",
            safe_parameter(&apn.username)?,
            safe_parameter(&apn.password)?
        ));
    }
    Ok(action)
}

fn mbim_start(
    endpoint: &NativeBearerConfig,
    apn: &ApnConfig,
    families: &[u8],
    role: Role,
) -> Result<String, NativeError> {
    let family = match families {
        [4] => "ipv4",
        [6] => "ipv6",
        [4, 6] | [6, 4] => "ipv4v6",
        _ => return Err(NativeError::Protocol("native_ip_families_invalid".into())),
    };
    if apn.apn.trim().is_empty() {
        return Err(NativeError::Protocol("native_apn_required".into()));
    }
    let context = match role {
        Role::Ims => "ims",
        Role::Data => "internet",
    };
    let auth = if apn.username.is_empty() && apn.password.is_empty() {
        "none"
    } else {
        match apn.auth_method.to_ascii_lowercase().as_str() {
            "pap" => "pap",
            _ => "chap",
        }
    };
    Ok(format!("--connect=session-id={},access-string={},ip-type={family},auth={auth},username={},password={},context-type={context}",
        endpoint.session_id,safe_parameter(&apn.apn)?,safe_parameter(&apn.username)?,safe_parameter(&apn.password)?))
}

async fn begin(
    device: Arc<NativeDevice>,
    role: Role,
    apn: ApnConfig,
    families: Vec<u8>,
    profile_id: Option<u32>,
    allow_roaming: bool,
) -> Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), NativeError> {
    // Admission happens before taking the transaction gate; no radio enabling
    // is implicit in packet data setup.
    if device.radio().await? != super::super::radio::RadioState::On {
        return Err(NativeError::Unavailable(
            "native_bearer_radio_not_on".into(),
        ));
    }
    let network = device.network().await?;
    if !network.registration.registered() {
        return Err(NativeError::Unavailable(
            "native_bearer_not_registered".into(),
        ));
    }
    if !allow_roaming && network.registration.roaming()? {
        return Err(NativeError::Unavailable(
            "cellular_data_roaming_forbidden".into(),
        ));
    }
    let endpoint = role.endpoint(&device).ok_or(NativeError::Unsupported(
        "native_bearer_endpoint_not_configured",
    ))?;
    let (sender, receiver) = oneshot::channel();
    let pending = super::PendingSetup::new();
    tokio::spawn(async move {
        let _pending = pending;
        let operation = device.operation.clone();
        let _guard = operation.lock().await;
        let result = begin_locked(
            device,
            endpoint,
            role,
            apn,
            families,
            profile_id,
            network.technology,
        )
        .await;
        if let Err(Ok((_, handle))) = sender.send(result) {
            // Release after dropping the transaction gate to avoid re-entry.
            drop(_guard);
            handle.release().await;
        }
    });
    receiver
        .await
        .map_err(|_| NativeError::CommandFailed("native_bearer_setup_worker_failed"))?
}

async fn begin_locked(
    device: Arc<NativeDevice>,
    endpoint: NativeBearerConfig,
    role: Role,
    apn: ApnConfig,
    families: Vec<u8>,
    profile_id: Option<u32>,
    technology: String,
) -> Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), NativeError> {
    if super::is_shutting_down() {
        return Err(NativeError::Unavailable(
            "native_backend_shutting_down".into(),
        ));
    }
    device.io.verify_bearer(&endpoint).await?;
    if device
        .active_interfaces
        .lock()
        .unwrap()
        .contains_key(role.label())
    {
        return Err(NativeError::OwnerConflict(
            "native_bearer_role_already_owned".into(),
        ));
    }
    if families.is_empty() || families.len() > 2 || families.iter().any(|f| !matches!(f, 4 | 6)) {
        return Err(NativeError::Protocol("native_ip_families_invalid".into()));
    }
    if families.len() == 2 && families[0] == families[1] {
        return Err(NativeError::Protocol("native_duplicate_ip_family".into()));
    }
    // Validate CLI grammars before allocating anything.
    match device.spec.protocol {
        NativeProtocol::Qmi => {
            for family in &families {
                qmi_start(&apn, *family, profile_id)?;
            }
        }
        NativeProtocol::Mbim => {
            mbim_start(&endpoint, &apn, &families, role)?;
        }
        NativeProtocol::At => {
            return Err(NativeError::Unsupported(
                "native_at_packet_data_driver_unavailable",
            ))
        }
    }
    let receipt = receipt_path(&device, role);
    let mut session = Session {
        device: device.clone(),
        endpoint: endpoint.clone(),
        role,
        clients: Vec::new(),
        receipt,
        namespace: String::new(),
        lost: Arc::new(AtomicBool::new(false)),
        monitor: None,
    };
    session.save(true)?;
    device
        .active_interfaces
        .lock()
        .unwrap()
        .insert(role.label().into(), endpoint.interface.clone());
    let mut awaiting_resource_identity = false;
    let result = async {
        let mut settings = Vec::new();
        match device.spec.protocol {
            NativeProtocol::Qmi => {
                for family in &families {
                    awaiting_resource_identity = true;
                    let output = device.io.execute(&qmi_request(&endpoint,None,"--wds-noop",true)).await?;
                    let cid = labelled(&output,"CID").and_then(|s| s.parse::<u8>().ok())
                        .filter(|n| *n > 0).ok_or_else(|| NativeError::Protocol("native_qmi_client_id_unconfirmed".into()))?;
                    session.clients.push(Client { cid: Some(cid), packet_handle: None, mbim_session: None });
                    awaiting_resource_identity = false;
                    session.save(false)?;
                    if let Some(port) = &endpoint.qmi_data_port {
                        device.io.execute(&qmi_request(&endpoint,Some(cid),&format!("--wds-bind-data-port={port}"),true)).await?;
                    }
                    if let Some(binding) = &endpoint.qmi_binding {
                        device.io.execute(&qmi_request(&endpoint,Some(cid),
                            &format!("--wds-bind-mux-data-port=mux-id={},ep-type={},ep-iface-number={}", binding.mux_id,binding.endpoint_type,binding.interface_number),true)).await?;
                    }
                    device.io.execute(&qmi_request(&endpoint,Some(cid),&format!("--wds-set-ip-family={family}"),true)).await?;
                    awaiting_resource_identity = true;
                    let start = device.io.execute(&qmi_request(&endpoint,Some(cid),&qmi_start(&apn,*family,profile_id)?,true)).await?;
                    let handle = qmi_wds::parse_packet_data_handle(&start).and_then(|v| v.parse::<u32>().ok())
                        .ok_or_else(|| NativeError::Protocol("native_qmi_packet_handle_unconfirmed".into()))?;
                    session.clients.last_mut().expect("allocated client").packet_handle = Some(handle);
                    awaiting_resource_identity = false;
                    session.save(false)?;
                    let text = device.io.execute(&qmi_request(&endpoint,Some(cid),"--wds-get-current-settings",true)).await?;
                    let settings_for_family = qmi_wds::parse_current_settings(&text);
                    if (*family == 4 && settings_for_family.ipv4_address.is_none()) || (*family == 6 && settings_for_family.ipv6_address.is_none()) {
                        return Err(NativeError::Protocol("native_bearer_family_address_missing".into()));
                    }
                    settings.push(settings_for_family);
                }
            }
            NativeProtocol::Mbim => {
                let current = device.io.execute(&mbim_request(&endpoint,&format!("--query-connection-state={}",endpoint.session_id))).await?;
                if labelled(&current,"Activation state") != Some("deactivated") {
                    return Err(NativeError::OwnerConflict("native_mbim_session_not_confirmed_free".into()));
                }
                session.clients.push(Client { cid: None, packet_handle: None, mbim_session: Some(endpoint.session_id) });
                session.save(false)?;
                awaiting_resource_identity = true;
                device.io.execute(&mbim_request(&endpoint,&mbim_start(&endpoint,&apn,&families,role)?)).await?;
                awaiting_resource_identity = false;
                let text = device.io.execute(&mbim_request(&endpoint,&format!("--query-ip-configuration={}",endpoint.session_id))).await?;
                settings.push(parse_mbim_ip_configuration(&text)?);
            }
            NativeProtocol::At => return Err(NativeError::Unsupported("native_at_packet_data_driver_unavailable")),
        }
        let mut info = ImsBearerInfo {
            interface: endpoint.interface.clone(), netdev_method: "native_explicit_mapping",
            path_device: endpoint.control_device.clone(),
            path_handle: format!("{}-{}",device.spec.line_id(),role.label()),
            ip_type: if families.len() == 2 { "ipv4v6".into() } else { format!("ipv{}",families[0]) },
            interface_ownership: BearerInterfaceOwnership::ApplicationOwnedNative,
            rat: if technology == "lte" { ThreeGppRat::Lte } else { ThreeGppRat::Unknown },
            bearer_domain: if technology == "lte" { BearerDomain::Eps } else { BearerDomain::Unknown },
            ..Default::default()
        };
        for settings in settings {
            if let Some(address) = settings.ipv4_address {
                info.ipv4_address = Some(parse_ip(&address,4)?);
                info.ipv4_prefix = Some(settings.ipv4_prefix.ok_or_else(|| NativeError::Protocol("native_ipv4_prefix_unknown".into()))?);
                info.ipv4_gateway = settings.ipv4_gateway.and_then(|v| v.parse().ok());
                info.ipv4_dns.extend(settings.ipv4_dns.iter().filter_map(|v| v.parse::<IpAddr>().ok()));
            }
            if let Some(address) = settings.ipv6_address {
                info.ipv6_address = Some(parse_ip(&address,6)?);
                info.ipv6_prefix = Some(settings.ipv6_prefix.ok_or_else(|| NativeError::Protocol("native_ipv6_prefix_unknown".into()))?);
                info.ipv6_gateway = settings.ipv6_gateway.and_then(|v| v.parse().ok());
                info.ipv6_dns.extend(settings.ipv6_dns.iter().filter_map(|v| v.parse::<IpAddr>().ok()));
            }
            info.pcscf.extend(settings.pcscf.iter().filter_map(|v| v.parse::<IpAddr>().ok()));
        }
        if families.contains(&4) && info.ipv4_address.is_none() || families.contains(&6) && info.ipv6_address.is_none() {
            return Err(NativeError::Protocol("native_bearer_family_address_missing".into()));
        }
        Ok(info)
    }.await;
    match result {
        Ok(info) => {
            if super::is_shutting_down() {
                session.cleanup_locked().await;
                return Err(NativeError::Unavailable(
                    "native_backend_shutting_down".into(),
                ));
            }
            session.start_monitor();
            Ok((
                info,
                Box::new(NativeHandle {
                    session: Some(session),
                }),
            ))
        }
        Err(error) => {
            // Unknown allocation outcomes are not safe to silently forget.
            let ambiguous =
                awaiting_resource_identity && !matches!(&error, NativeError::ProtocolRejected(_));
            if ambiguous {
                tracing::warn!(line_id = %device.spec.line_id(), "Native allocation outcome ambiguous; receipt retained for reconciliation");
            } else {
                session.cleanup_locked().await;
            }
            Err(error)
        }
    }
}

fn parse_ip(address: &str, family: u8) -> Result<IpAddr, NativeError> {
    address
        .parse::<IpAddr>()
        .ok()
        .filter(|a| {
            matches!(family, 4 | 6)
                && !a.is_unspecified()
                && !a.is_multicast()
                && a.is_ipv4() == (family == 4)
        })
        .ok_or_else(|| NativeError::Protocol("native_bearer_ip_invalid".into()))
}

fn parse_mbim_ip_configuration(text: &str) -> Result<qmi_wds::CurrentSettings, NativeError> {
    let mut settings = qmi_wds::CurrentSettings::default();
    let mut family = 0_u8;
    let mut addresses = false;
    let mut dns = false;
    for line in text.lines().map(str::trim) {
        // mbimcli prefixes family headers with [device-path]. Normalize only
        // the header candidate, leaving IP/DNS indices and field values intact.
        let header = line
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .map_or(line, |(_, rest)| rest.trim_start());
        if header.starts_with("IPv4 configuration available:") {
            family = 4;
            addresses = false;
            dns = false;
            continue;
        }
        if header.starts_with("IPv6 configuration available:") {
            family = 6;
            addresses = false;
            dns = false;
            continue;
        }
        if line.starts_with("IP addresses") {
            addresses = true;
            dns = false;
            continue;
        }
        if line.starts_with("DNS addresses") {
            addresses = false;
            dns = true;
            continue;
        }
        if let Some(gateway) = labelled(line, "Gateway") {
            addresses = false;
            dns = false;
            if gateway == "0.0.0.0" || gateway == "::" || gateway.eq_ignore_ascii_case("none") {
                continue;
            }
            let gateway = parse_ip(gateway, family)?.to_string();
            if family == 4 {
                settings.ipv4_gateway = Some(gateway);
            } else if family == 6 {
                settings.ipv6_gateway = Some(gateway);
            }
            continue;
        }
        let Some((label, value)) = line.split_once(':') else {
            continue;
        };
        if !matches!(family, 4 | 6) {
            continue;
        }
        let value = value.trim().trim_matches('\'');
        // Explicit labels take precedence over the preceding section header:
        // some mbimcli releases omit "DNS addresses" before a DNS [n] field.
        // Never let the old address-section flag swallow that DNS entry, or
        // mistake an unrelated labelled field for an IP address.
        let index_only = label.starts_with('[') && label.ends_with(']');
        if label.starts_with("IP [") || (addresses && index_only) {
            let (ip, prefix) = value.split_once('/').ok_or_else(|| {
                NativeError::Protocol("native_mbim_address_prefix_missing".into())
            })?;
            let ip = parse_ip(ip, family)?.to_string();
            let prefix = prefix
                .parse::<u8>()
                .ok()
                .filter(|prefix| *prefix <= if family == 4 { 32 } else { 128 })
                .ok_or_else(|| {
                    NativeError::Protocol("native_mbim_address_prefix_invalid".into())
                })?;
            if family == 4 && settings.ipv4_address.is_none() {
                settings.ipv4_address = Some(ip);
                settings.ipv4_prefix = Some(prefix);
            } else if family == 6 && settings.ipv6_address.is_none() {
                settings.ipv6_address = Some(ip);
                settings.ipv6_prefix = Some(prefix);
            }
        } else if label.starts_with("DNS [") || (dns && index_only) {
            if value == "0.0.0.0" || value == "::" {
                continue;
            }
            let value = parse_ip(value, family)?.to_string();
            let servers = if family == 4 {
                &mut settings.ipv4_dns
            } else {
                &mut settings.ipv6_dns
            };
            if !servers.contains(&value) {
                servers.push(value);
            }
        }
    }
    if settings.ipv4_address.is_none() && settings.ipv6_address.is_none() {
        return Err(NativeError::Protocol(
            "native_mbim_ip_configuration_missing".into(),
        ));
    }
    Ok(settings)
}

struct NativeHandle {
    session: Option<Session>,
}
impl Drop for NativeHandle {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(session.release());
            } // A receipt remains if runtime teardown prevents cleanup.
        }
    }
}
impl ImsBearerHandle for NativeHandle {
    fn check_liveness(&mut self) -> Result<(), ImsBearerError> {
        if self
            .session
            .as_ref()
            .is_none_or(|s| s.lost.load(Ordering::Acquire))
        {
            Err(ims_error(
                NativeError::Unavailable("native_bearer_session_lost".into()),
                ImsBearerErrorKind::SessionLost,
            ))
        } else {
            Ok(())
        }
    }
    fn prepare_namespace_move(&mut self, namespace: &str) -> Result<Box<dyn Send>, ImsBearerError> {
        let session = self.session.as_mut().ok_or_else(|| {
            ims_error(
                NativeError::Unavailable("native_bearer_released".into()),
                ImsBearerErrorKind::SessionLost,
            )
        })?;
        session.namespace = namespace.into();
        session
            .save(false)
            .map_err(|e| ims_error(e, ImsBearerErrorKind::SessionLost))?;
        Ok(Box::new(()))
    }
    fn confirm_namespace_restore<'a>(
        &'a mut self,
        namespace: &'a str,
    ) -> TransportFuture<'a, Result<(), ImsBearerError>> {
        Box::pin(async move {
            let session = self.session.as_mut().ok_or_else(|| {
                ims_error(
                    NativeError::Unavailable("native_bearer_released".into()),
                    ImsBearerErrorKind::SessionLost,
                )
            })?;
            if namespace.is_empty() || session.namespace != namespace {
                return Err(ims_error(
                    NativeError::OwnerConflict("native_namespace_restore_mismatch".into()),
                    ImsBearerErrorKind::SessionLost,
                ));
            }
            // move_iface_out is best-effort when an interface is absent.
            // Require the physical owner and an idle host-side netdev, not
            // merely a successful command or a same-name namespace lookup.
            session
                .device
                .io
                .verify_bearer(&session.endpoint)
                .await
                .map_err(|error| ims_error(error, ImsBearerErrorKind::SessionLost))?;
            let previous = std::mem::take(&mut session.namespace);
            if let Err(error) = session.save(false) {
                session.namespace = previous;
                return Err(ims_error(error, ImsBearerErrorKind::SessionLost));
            }
            Ok(())
        })
    }
    fn release(
        mut self: Box<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'static>> {
        let session = self.session.take();
        Box::pin(async move {
            if let Some(session) = session {
                session.release().await;
            }
        })
    }
}

fn ims_error(error: NativeError, kind: ImsBearerErrorKind) -> ImsBearerError {
    ImsBearerError {
        kind,
        hint: ImsBearerFailureHint::None,
        detail: error.to_string(),
    }
}

pub struct NativeImsTransport {
    device: Arc<NativeDevice>,
}
impl NativeImsTransport {
    pub fn new(device: Arc<NativeDevice>) -> Arc<Self> {
        Arc::new(Self { device })
    }
}
impl ImsBearerTransport for NativeImsTransport {
    fn endpoint_available(&self, primary: &str) -> bool {
        self.device.spec.control_device == primary && self.device.spec.ims.is_some()
    }
    fn establish_ims_bearer<'a>(
        &'a self,
        primary: &'a str,
        selector: &'a str,
        apn: &'a str,
        profile: Option<u32>,
        _cid: u8,
        families: &'a [u8],
        roaming: bool,
    ) -> TransportFuture<'a, Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError>>
    {
        Box::pin(async move {
            if !self.endpoint_available(primary) || selector != self.device.spec.selector() {
                return Err(ims_error(
                    NativeError::OwnerConflict("native_ims_endpoint_owner_mismatch".into()),
                    ImsBearerErrorKind::EndpointUnavailable,
                ));
            }
            begin(
                self.device.clone(),
                Role::Ims,
                ApnConfig {
                    apn: apn.into(),
                    ..Default::default()
                },
                families.to_vec(),
                profile,
                roaming,
            )
            .await
            .map_err(|e| ims_error(e, ImsBearerErrorKind::SessionStartFailed))
        })
    }
}

pub struct NativeDataTransport {
    device: Arc<NativeDevice>,
    state: Mutex<Option<native_bearer::NativeImsBearer>>,
}

fn data_ip_families(protocol: &str) -> Result<Vec<u8>, String> {
    match protocol.trim().to_ascii_lowercase().as_str() {
        "ipv4" => Ok(vec![4]),
        "ipv6" => Ok(vec![6]),
        // `dual` is the persisted ApnConfig default and the existing UI value.
        "dual" | "ipv4v6" => Ok(vec![4, 6]),
        _ => Err("native_data_ip_protocol_invalid".into()),
    }
}

impl NativeDataTransport {
    pub fn new(device: Arc<NativeDevice>) -> Arc<Self> {
        Arc::new(Self {
            device,
            state: Mutex::new(None),
        })
    }
}

fn data_route_ops(bearer: &BearerConnection) -> Result<Vec<NetConfigOp>, CellularImsError> {
    let mut routes = Vec::new();
    for (address, gateway) in [
        (bearer.settings.ipv4_address, bearer.settings.ipv4_gateway),
        (bearer.settings.ipv6_address, bearer.settings.ipv6_gateway),
    ] {
        let Some(address) = address else { continue };
        if let Some(gateway) = gateway {
            if gateway.is_ipv6() != address.is_ipv6()
                || gateway.is_unspecified()
                || gateway.is_multicast()
            {
                return Err(CellularImsError::new("native_data_gateway_family_invalid"));
            }
            // Raw-IP gateways may be outside the assigned prefix. Declare the
            // direct host route before the family-explicit default route.
            routes.push(NetConfigOp::RouteReplace {
                target: format!("{gateway}/{}", if gateway.is_ipv6() { 128 } else { 32 }),
                via: None,
                dev: Some(bearer.interface.clone()),
                src: None,
                table: None,
                onlink: false,
            });
            routes.push(NetConfigOp::DefaultRouteReplace {
                via: gateway.to_string(),
                dev: bearer.interface.clone(),
            });
        } else {
            routes.push(NetConfigOp::DefaultRouteDeviceReplace {
                dev: bearer.interface.clone(),
                ipv6: address.is_ipv6(),
                metric: 50,
            });
        }
    }
    Ok(routes)
}

async fn apply_data_routes(
    binding: &UeWorkerBinding,
    routes: Vec<NetConfigOp>,
) -> Result<(), CellularImsError> {
    let outcome = binding.apply_net_config(routes).await.map_err(|error| {
        CellularImsError::with_detail("native_data_ue_route_failed", error.to_string())
    })?;
    if !outcome.ok {
        return Err(CellularImsError::with_detail(
            "native_data_ue_route_failed",
            outcome
                .error
                .unwrap_or_else(|| "worker net-config failed".into()),
        ));
    }
    Ok(())
}

impl CellularDataTransport for NativeDataTransport {
    fn interface(&self) -> TransportFuture<'_, Option<String>> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if let Some(session) = state.as_mut() {
                if session.check_liveness().is_ok() && session.worker_binding_is_current() {
                    return Some(session.interface.clone());
                }
            }
            None
        })
    }
    fn start<'a>(
        &'a self,
        line: &'a str,
        primary: &'a str,
        apn: &'a ApnConfig,
    ) -> TransportFuture<'a, Result<String, String>> {
        Box::pin(async move {
            if line != self.device.spec.line_id() || !self.endpoint_available(primary) {
                return Err("native_data_endpoint_owner_mismatch".into());
            }
            let mut state = self.state.lock().await;
            if let Some(session) = state.as_mut() {
                if session.check_liveness().is_ok() && session.worker_binding_is_current() {
                    return Ok(session.interface.clone());
                }
            }
            if let Some(old) = state.take() {
                native_bearer::release_native_ims_bearer(old).await;
            }
            let worker = worker_for_line(line)
                .ok_or_else(|| "native_data_ue_worker_unavailable".to_string())?;
            if !worker.status().await.ready {
                return Err("native_data_ue_worker_not_ready".into());
            }
            if apn.apn.is_empty() || apn.apn.eq_ignore_ascii_case("ims") {
                return Err("native_data_apn_required".into());
            }
            let families = data_ip_families(&apn.protocol)?;
            let (info, handle) = begin(
                self.device.clone(),
                Role::Data,
                apn.clone(),
                families,
                None,
                true,
            )
            .await
            .map_err(|e| e.to_string())?;
            let mut session = native_bearer::adopt_bearer(info, handle)
                .await
                .map_err(|e| e.to_string())?;
            let result = async {
                session.move_into_worker(worker.clone()).await?;
                let binding = session.worker_binding().ok_or_else(|| {
                    CellularImsError::new("native_data_ue_worker_binding_missing")
                })?;
                configure_data_bearer_network_in_worker(&session.connection, binding).await?;
                apply_data_routes(binding, data_route_ops(&session.connection)?).await
            }
            .await;
            if let Err(error) = result {
                native_bearer::release_native_ims_bearer(session).await;
                return Err(error.to_string());
            }
            let interface = session.interface.clone();
            *state = Some(session);
            Ok(interface)
        })
    }
    fn stop(&self) -> TransportFuture<'_, ()> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if let Some(session) = state.take() {
                native_bearer::release_native_ims_bearer(session).await;
            }
        })
    }
    fn endpoint_available(&self, primary: &str) -> bool {
        self.device.spec.control_device == primary && self.device.spec.data.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mbim_parser_accepts_real_device_prefixed_dual_stack_output() {
        // mbimcli-helpers.c prints no IP/DNS section headings; only the
        // per-family summary has a device path in brackets.
        let settings = parse_mbim_ip_configuration(concat!(
            "\n[/dev/cdc-wdm0] IPv4 configuration available: 'address, gateway, dns, mtu'\n",
            "     IP [0]: '192.0.2.9/30'\n",
            "    Gateway: '192.0.2.10'\n",
            "    DNS [0]: '192.0.2.53'\n",
            "        MTU: '1500'\n",
            "\n[/dev/cdc-wdm0] IPv6 configuration available: 'address, gateway, dns, mtu'\n",
            "     IP [0]: '2001:db8::9/64'\n",
            "    Gateway: 'fe80::1'\n",
            "    DNS [0]: '2001:db8::53'\n",
            "        MTU: '1500'\n",
        ))
        .unwrap();
        assert_eq!(settings.ipv4_address.as_deref(), Some("192.0.2.9"));
        assert_eq!(settings.ipv4_prefix, Some(30));
        assert_eq!(settings.ipv4_gateway.as_deref(), Some("192.0.2.10"));
        assert_eq!(settings.ipv4_dns, vec!["192.0.2.53"]);
        assert_eq!(settings.ipv6_address.as_deref(), Some("2001:db8::9"));
        assert_eq!(settings.ipv6_prefix, Some(64));
        assert_eq!(settings.ipv6_gateway.as_deref(), Some("fe80::1"));
        assert_eq!(settings.ipv6_dns, vec!["2001:db8::53"]);
    }

    #[test]
    fn mbim_prefixed_headers_preserve_single_family_and_validation() {
        let settings = parse_mbim_ip_configuration(concat!(
            "[/dev/cdc-wdm0] IPv4 configuration available: 'none'\n",
            "[/dev/cdc-wdm0] IPv6 configuration available: 'address'\n",
            "     IP [0]: '2001:db8::9/64'\n",
        ))
        .unwrap();
        assert!(settings.ipv4_address.is_none());
        assert_eq!(settings.ipv6_address.as_deref(), Some("2001:db8::9"));
        assert!(parse_mbim_ip_configuration(concat!(
            "[/dev/cdc-wdm0] IPv6 configuration available: 'address'\n",
            "     IP [0]: '192.0.2.9/30'\n",
        ))
        .is_err());
        assert!(parse_mbim_ip_configuration(concat!(
            "unrelated IPv4 configuration available: 'address'\n",
            "     IP [0]: '192.0.2.9/30'\n",
        ))
        .is_err());
    }

    #[test]
    fn mbim_parser_accepts_explicit_dns_labels_without_section_headers() {
        let settings = parse_mbim_ip_configuration("IPv4 configuration available: 'address, dns'\nIP addresses (1)\nIP [0]: '192.0.2.9/30'\nDNS [0]: '192.0.2.53'\nDNS [1]: '192.0.2.53'\nIPv6 configuration available: 'address, gateway, dns'\nIP addresses (1)\nIP [0]: '2001:db8::9/64'\nDNS [0]: '2001:db8::53'\nGateway: '8000::500'").unwrap();
        assert_eq!(settings.ipv4_dns, vec!["192.0.2.53"]);
        assert_eq!(settings.ipv6_dns, vec!["2001:db8::53"]);
        assert_eq!(settings.ipv6_gateway.as_deref(), Some("8000::500"));
    }

    #[test]
    fn mbim_parser_rejects_cross_family_and_bad_prefix_settings() {
        for field in [
            "IP [0]: '192.0.2.1/129'",
            "IP [0]: '2001:db8::1/129'",
            "IP [0]: '2001:db8::1'",
            "IP [0]: '::/64'",
            "IP [0]: '2001:db8::1/64'\nGateway: '192.0.2.1'",
            "IP [0]: '2001:db8::1/64'\nDNS [0]: '192.0.2.53'",
        ] {
            assert!(parse_mbim_ip_configuration(&format!(
                "IPv6 configuration available: 'address'\n{field}"
            ))
            .is_err());
        }
        let settings = parse_mbim_ip_configuration(
            "IPv6 configuration available: 'address'\nIP [0]: '2001:db8::1/64'\nGateway: '::'",
        )
        .unwrap();
        assert_eq!(settings.ipv6_gateway, None);
    }

    #[test]
    fn native_data_routes_require_matching_gateway_family() {
        let mut info = ImsBearerInfo {
            interface: "wwan0".into(),
            ipv6_address: Some("2001:db8::1".parse().unwrap()),
            ipv6_prefix: Some(64),
            ..Default::default()
        };
        let direct = native_bearer::to_bearer_connection(&info).unwrap();
        assert!(matches!(
            data_route_ops(&direct).unwrap().as_slice(),
            [NetConfigOp::DefaultRouteDeviceReplace { ipv6: true, .. }]
        ));
        info.ipv6_gateway = Some("8000::500".parse().unwrap());
        let bearer = native_bearer::to_bearer_connection(&info).unwrap();
        let ops = data_route_ops(&bearer).unwrap();
        assert!(
            matches!(&ops[0], NetConfigOp::RouteReplace { target, via: None, .. } if target == "8000::500/128")
        );
        assert!(
            matches!(&ops[1], NetConfigOp::DefaultRouteReplace { via, .. } if via == "8000::500")
        );
        info.ipv6_gateway = Some("192.0.2.1".parse().unwrap());
        assert!(data_route_ops(&native_bearer::to_bearer_connection(&info).unwrap()).is_err());
    }

    #[tokio::test]
    async fn native_data_does_not_report_success_when_worker_rejects_a_route() {
        let worker = crate::services::ue_worker::UeWorkerHandle::for_line(
            "fixture",
            crate::platform::netns::NetnsName::for_line("sa-ue", "fixture"),
        );
        worker
            .enable_test_net_config_outcome(Some("fixture route failure".into()))
            .await;
        let error = apply_data_routes(
            &worker.bind(),
            vec![NetConfigOp::DefaultRouteDeviceReplace {
                dev: "wwan0".into(),
                ipv6: true,
                metric: 50,
            }],
        )
        .await
        .unwrap_err();
        assert_eq!(error.code(), "native_data_ue_route_failed");
        assert!(error.to_string().contains("fixture route failure"));
    }

    #[test]
    fn native_data_accepts_existing_dual_apn_default_without_guessing_unknown_values() {
        assert_eq!(
            data_ip_families(&ApnConfig::default().protocol).unwrap(),
            vec![4, 6]
        );
        assert_eq!(data_ip_families("dual").unwrap(), vec![4, 6]);
        assert_eq!(data_ip_families("IPV4V6").unwrap(), vec![4, 6]);
        assert_eq!(data_ip_families("ipv4").unwrap(), vec![4]);
        assert_eq!(data_ip_families("ipv6").unwrap(), vec![6]);
        for invalid in ["", "automatic", "dual,autoconnect=yes"] {
            assert!(data_ip_families(invalid).is_err());
        }
    }

    fn endpoint() -> NativeBearerConfig {
        NativeBearerConfig {
            control_device: "/dev/fixture".into(),
            interface: "wwan9".into(),
            session_id: 2,
            qmi_data_port: None,
            qmi_binding: None,
        }
    }

    #[test]
    fn qmi_session_commands_keep_client_ownership_and_never_enable_autoconnect() {
        let endpoint = endpoint();
        let request = qmi_request(&endpoint, Some(17), "--wds-get-current-settings", true);
        assert!(request.arguments.contains(&"--client-cid=17".into()));
        assert!(request
            .arguments
            .contains(&"--client-no-release-cid".into()));
        let start = qmi_start(
            &ApnConfig {
                apn: "ims".into(),
                ..Default::default()
            },
            6,
            Some(2),
        )
        .unwrap();
        assert!(start.contains("ip-type=6"));
        assert!(!start.contains("ip-type=ipv6"));
        assert!(!start.contains("autoconnect"));
        assert!(qmi_start(
            &ApnConfig {
                apn: "ims,autoconnect=yes".into(),
                ..Default::default()
            },
            4,
            None
        )
        .is_err());
    }

    #[test]
    fn mbim_connection_is_explicitly_scoped_to_a_session_and_context() {
        let start = mbim_start(
            &endpoint(),
            &ApnConfig {
                apn: "ims".into(),
                ..Default::default()
            },
            &[4, 6],
            Role::Ims,
        )
        .unwrap();
        assert!(start.contains("session-id=2"));
        assert!(start.contains("context-type=ims"));
        let settings = parse_mbim_ip_configuration("IPv4 configuration available: 'address'\nIP addresses (1)\n IP [0]: '192.0.2.9/30'\nGateway: '192.0.2.10'\nDNS addresses (1)\n DNS [0]: '192.0.2.1'").unwrap();
        assert_eq!(settings.ipv4_address.as_deref(), Some("192.0.2.9"));
        assert_eq!(settings.ipv4_prefix, Some(30));
        let settings = parse_mbim_ip_configuration("IPv6 configuration available: 'address, gateway, dns'\nIP [0]: '2001:db8::10/64'\nGateway: '2001:db8::1'\nDNS [0]: '2001:db8::53'").unwrap();
        assert_eq!(settings.ipv6_prefix, Some(64));
    }

    struct MemoryIo {
        requests: std::sync::Mutex<Vec<CommandRequest>>,
        receipts: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
        interface_available: AtomicBool,
        reject_receipt_update: AtomicBool,
        failure: Option<&'static str>,
    }
    impl super::super::io::NativeIo for MemoryIo {
        fn execute<'a>(
            &'a self,
            request: &'a CommandRequest,
        ) -> TransportFuture<'a, Result<String, NativeError>> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(request.clone());
                let action = &request.arguments[3];
                if action == "--dms-get-operating-mode" {
                    return Ok("Mode: 'online'".into());
                }
                if action == "--nas-get-serving-system" {
                    return Ok("Registration state: 'registered'\nRoaming status: 'off'\nCurrent PLMN:\nMCC: '001'\nMNC: '01'\nRadio interfaces: '1'\n[0]: 'lte'".into());
                }
                if action == "--uim-get-card-status" {
                    return Ok(
                        "Provisioning applications:\nPrimary GW: slot '1', application '1'".into(),
                    );
                }
                if action == "--wds-noop" {
                    return Ok("Client ID not released:\nService: 'wds'\nCID: '17'".into());
                }
                if action.starts_with("--wds-start-network=") {
                    assert!(
                        action.contains("ip-type=4") || action.contains("ip-type=6"),
                        "qmicli does not accept the MM/MBIM ipv4/ipv6 spelling"
                    );
                    assert!(
                        !self.receipts.lock().unwrap().is_empty(),
                        "ownership must be recorded before activation"
                    );
                    if let Some(reason) = self.failure {
                        if reason == "fixture_rejected" {
                            return Err(NativeError::ProtocolRejected(14));
                        }
                        return Err(NativeError::CommandFailed(reason));
                    }
                    return Ok("Packet data handle: '42'".into());
                }
                if action == "--wds-get-current-settings" {
                    return Ok("IPv4 address: 192.0.2.2\nIPv4 subnet mask: 255.255.255.252\nIPv4 gateway address: 192.0.2.1\nP-CSCF address: 192.0.2.3".into());
                }
                Ok(String::new())
            })
        }
        fn verify_bearer<'a>(
            &'a self,
            _endpoint: &'a NativeBearerConfig,
        ) -> TransportFuture<'a, Result<(), NativeError>> {
            Box::pin(async move {
                if self.interface_available.load(Ordering::Acquire) {
                    Ok(())
                } else {
                    Err(NativeError::OwnerConflict(
                        "fixture_interface_not_on_host".into(),
                    ))
                }
            })
        }
        fn save_receipt(&self, key: &str, bytes: &[u8], create: bool) -> Result<(), NativeError> {
            if !create && self.reject_receipt_update.load(Ordering::Acquire) {
                return Err(NativeError::OwnerConflict(
                    "fixture_receipt_update_failed".into(),
                ));
            }
            let mut receipts = self.receipts.lock().unwrap();
            if create && receipts.contains_key(key) {
                return Err(NativeError::OwnerConflict("fixture_receipt_exists".into()));
            }
            receipts.insert(key.into(), bytes.into());
            Ok(())
        }
        fn clear_receipt(&self, key: &str) -> Result<(), NativeError> {
            self.receipts.lock().unwrap().remove(key);
            Ok(())
        }
    }
    fn memory_device(failure: Option<&'static str>) -> (Arc<NativeDevice>, Arc<MemoryIo>) {
        let io = Arc::new(MemoryIo {
            requests: Default::default(),
            receipts: Default::default(),
            interface_available: AtomicBool::new(true),
            reject_receipt_update: AtomicBool::new(false),
            failure,
        });
        let device = NativeDevice::new(
            super::super::config::NativeDeviceConfig {
                hardware_key: "native-session-test".into(),
                sysfs_anchor: "/sys/devices/fixture".into(),
                protocol: NativeProtocol::Qmi,
                control_device: "/dev/fixture".into(),
                at_device: None,
                sms_reception_enabled: false,
                uim_slot: 1,
                ims: Some(endpoint()),
                data: None,
            },
            io.clone(),
        );
        (device, io)
    }

    #[tokio::test]
    async fn native_ims_setup_and_teardown_have_no_mm_or_hardware_dependency_in_logic_tests() {
        let (device, io) = memory_device(None);
        let (info, handle) = begin(
            device.clone(),
            Role::Ims,
            ApnConfig {
                apn: "ims".into(),
                ..Default::default()
            },
            vec![4],
            None,
            false,
        )
        .await
        .unwrap();
        assert_eq!(info.ipv4_address.unwrap().to_string(), "192.0.2.2");
        assert_eq!(
            info.interface_ownership,
            BearerInterfaceOwnership::ApplicationOwnedNative
        );
        assert_eq!(device.active_interfaces.lock().unwrap().len(), 1);
        handle.release().await;
        assert!(io.receipts.lock().unwrap().is_empty());
        assert!(device.active_interfaces.lock().unwrap().is_empty());
        let requests = io.requests.lock().unwrap();
        assert!(requests
            .iter()
            .any(|r| r.arguments.iter().any(|s| s == "--wds-stop-network=42")));
        assert!(requests
            .iter()
            .any(|r| r.arguments.iter().any(|s| s == "--wds-set-ip-family=4")));
        assert!(requests
            .iter()
            .all(|r| r.tool == super::super::protocol::Tool::Qmi));
    }

    #[tokio::test]
    async fn namespace_receipt_is_cleared_only_after_verified_restore() {
        for scenario in [
            "unconfirmed",
            "missing_interface",
            "wrong_namespace",
            "write_failed",
            "confirmed",
        ] {
            let (device, io) = memory_device(None);
            let (_, mut handle) = begin(
                device.clone(),
                Role::Ims,
                ApnConfig {
                    apn: "ims".into(),
                    ..Default::default()
                },
                vec![4],
                None,
                false,
            )
            .await
            .unwrap();
            let namespace = "sa-ue0123456789ab";
            drop(handle.prepare_namespace_move(namespace).unwrap());
            if scenario != "unconfirmed" {
                io.interface_available
                    .store(scenario != "missing_interface", Ordering::Release);
                io.reject_receipt_update
                    .store(scenario == "write_failed", Ordering::Release);
                let target = if scenario == "wrong_namespace" {
                    "sa-ueabcdef012345"
                } else {
                    namespace
                };
                let restored = handle.confirm_namespace_restore(target).await;
                assert_eq!(restored.is_ok(), scenario == "confirmed", "{scenario}");
            }
            let key = receipt_path(&device, Role::Ims);
            let saved: serde_json::Value =
                serde_json::from_slice(io.receipts.lock().unwrap().get(&key).unwrap()).unwrap();
            assert_eq!(
                saved["namespace"],
                if scenario == "confirmed" {
                    ""
                } else {
                    namespace
                },
                "{scenario}"
            );
            handle.release().await;
            assert_eq!(
                io.receipts.lock().unwrap().is_empty(),
                scenario == "confirmed",
                "{scenario}"
            );
            assert_eq!(
                device.active_interfaces.lock().unwrap().is_empty(),
                scenario == "confirmed",
                "{scenario}"
            );
            // Pending network ownership does not suppress safe modem cleanup.
            assert!(io
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.arguments.iter().any(|arg| arg == "--wds-stop-network=42")));
        }
    }

    #[tokio::test]
    async fn dropped_handle_retains_unconfirmed_namespace_ownership() {
        let (device, io) = memory_device(None);
        let (_, mut handle) = begin(
            device.clone(),
            Role::Ims,
            ApnConfig {
                apn: "ims".into(),
                ..Default::default()
            },
            vec![4],
            None,
            false,
        )
        .await
        .unwrap();
        drop(handle.prepare_namespace_move("sa-ue0123456789ab").unwrap());
        drop(handle);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let released = io.requests.lock().unwrap().iter().any(|request| {
                    request.arguments.iter().any(|arg| arg == "--wds-noop")
                        && request.arguments.iter().any(|arg| arg == "--client-cid=17")
                        && !request
                            .arguments
                            .iter()
                            .any(|arg| arg == "--client-no-release-cid")
                });
                if released {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropped native handle must finish bounded modem cleanup");
        // Acquire the same operation gate to wait for the entire cleanup,
        // not just the final command captured by the fake IO boundary.
        let _guard = device.operation.lock().await;
        assert!(!io.receipts.lock().unwrap().is_empty());
        assert!(!device.active_interfaces.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn repeated_cleanup_does_not_replay_released_cids_while_namespace_is_pending() {
        let (device, io) = memory_device(None);
        let mut session = Session {
            device: device.clone(), endpoint: endpoint(), role: Role::Ims,
            clients: vec![Client { cid: Some(17), packet_handle: Some(42), mbim_session: None }],
            receipt: receipt_path(&device, Role::Ims), namespace: "sa-ue0123456789ab".into(),
            lost: Arc::new(AtomicBool::new(false)), monitor: None,
        };
        session.save(true).unwrap();
        assert!(!session.cleanup_locked().await);
        assert!(session.clients.is_empty());
        assert!(!session.cleanup_locked().await);
        let requests = io.requests.lock().unwrap();
        assert_eq!(requests.iter().filter(|r| r.arguments.iter().any(|a| a == "--wds-noop")).count(), 1);
        assert!(!io.receipts.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn native_start_failure_rolls_back_known_client_but_timeout_retains_receipt() {
        for (failure, ambiguous) in [
            ("fixture_rejected", false),
            ("native_protocol_command_timeout", true),
            ("native_helper_output_encoding", true),
            ("native_command_outcome_unconfirmed", true),
        ] {
            let (device, io) = memory_device(Some(failure));
            assert!(begin(
                device.clone(),
                Role::Ims,
                ApnConfig {
                    apn: "ims".into(),
                    ..Default::default()
                },
                vec![4],
                None,
                true
            )
            .await
            .is_err());
            assert_eq!(!io.receipts.lock().unwrap().is_empty(), ambiguous);
            assert_eq!(
                !device.active_interfaces.lock().unwrap().is_empty(),
                ambiguous
            );
        }
    }
}
