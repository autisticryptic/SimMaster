//! Qualcomm 410 (MSM8916-class) native IMS bearer driver.
//!
//! Implements [`ImsBearerTransport`] over the spare `DATA*_CNTL` rpmsg channels:
//! this is the "DATA6" path. It binds/obtains the secondary QMI endpoint for the
//! device's baseband, starts one retained WDS IMS session per bearer attempt,
//! reads the IMS context's IP configuration and P-CSCF from
//! `AT+CGCONTRDP`, resolves the bam-dmux netdev that carries the session and
//! hands the result back to the strategy layer as a device-agnostic
//! [`ImsBearerInfo`] plus an opaque [`ImsBearerHandle`].
//!
//! Everything in here is specific to this chip; the upper VoLTE stack only sees
//! the trait. `release_native_ims_bearer` on the strategy side is what drives
//! the returned handle.

use std::future::Future;
use std::pin::Pin;

use crate::hardware::cellular::cgcontrdp::{self, CgcontrdpSettings};
use crate::hardware::devices::qcm410::{
    netdev::{self as qmi_netdev, NetdevConfig},
    secondary_qmi::{self, ImsSession, SecondaryQmiEndpoint},
};
use crate::hardware::devices::transport::{
    BearerInterfaceOwnership, ImsBearerError, ImsBearerErrorKind, ImsBearerFailureHint,
    ImsBearerHandle, ImsBearerInfo, ImsBearerTransport, TransportFuture,
};

/// The primary ModemManager netdev must never be adopted by a native IMS
/// session. Native IMS is valid only on a secondary interface that SimAdmin can
/// move into the line's UE namespace.
const IMS_RESERVED_NETDEVS: &[&str] = &["wwan0"];

/// The qcm410 IMS bearer driver. Stateless; one instance serves every line.
pub struct Qcm410ImsBearer;

/// Everything needed to tear one established bearer down again.
pub struct Qcm410ImsBearerHandle {
    /// Secondary QMI endpoint the session(s) run on. Held so teardown can stop
    /// the sessions and release the endpoint.
    endpoint: SecondaryQmiEndpoint,
    /// Retained WDS client to stop and release on teardown.
    sessions: Vec<ImsSession>,
    /// Family-specific addresses and policy routes installed for the retained
    /// WDS session(s). They must be removed without flushing the shared netdev.
    configured_netdevs: Vec<(String, NetdevConfig)>,
}

impl ImsBearerHandle for Qcm410ImsBearerHandle {
    fn release(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            let Qcm410ImsBearerHandle {
                endpoint,
                sessions,
                configured_netdevs,
            } = *self;
            for (interface, config) in &configured_netdevs {
                qmi_netdev::teardown(interface, config).await;
            }
            for session in sessions {
                secondary_qmi::stop_ims_session(session).await;
            }
            secondary_qmi::release_endpoint(&endpoint).await;
        })
    }
}

impl ImsBearerTransport for Qcm410ImsBearer {
    fn establish_ims_bearer<'a>(
        &'a self,
        primary_device: &'a str,
        modem_id: &'a str,
        apn: &'a str,
        profile_id: Option<u32>,
        cid: u8,
        families: &'a [u8],
    ) -> TransportFuture<'a, Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError>>
    {
        Box::pin(async move {
            // `primary_device` is the line's primary QMI control port; it is used
            // only to find the *baseband*, so the secondary endpoint and the netdev
            // are paired to the same modem (multi-line correctness). The IMS session
            // itself never touches the primary port — that stays with ModemManager.
            let baseband =
                secondary_qmi::baseband_key_for_device(primary_device).map_err(|error| {
                    ImsBearerError {
                        kind: ImsBearerErrorKind::BasebandUnresolved,
                        hint: ImsBearerFailureHint::None,
                        detail: format!("native_ims_baseband_unresolved:{error}"),
                    }
                })?;

            let endpoint = secondary_qmi::runtime_endpoint(primary_device)
                .await
                .map_err(|error| ImsBearerError {
                    kind: ImsBearerErrorKind::EndpointUnavailable,
                    hint: ImsBearerFailureHint::None,
                    detail: error.to_string(),
                })?;

            let result = establish_bearer(
                &endpoint, &baseband, modem_id, apn, profile_id, cid, families,
            )
            .await;
            match result {
                Ok(established) => Ok((
                    established.info,
                    Box::new(established.handle) as Box<dyn ImsBearerHandle + Send>,
                )),
                Err(error) => {
                    secondary_qmi::release_endpoint(&endpoint).await;
                    Err(error)
                }
            }
        })
    }
}

struct Established {
    info: ImsBearerInfo,
    handle: Qcm410ImsBearerHandle,
}

/// Start one retained WDS session for `families`, read the IMS context
/// settings, resolve the netdev and assemble the device-agnostic result.
async fn establish_bearer(
    endpoint: &SecondaryQmiEndpoint,
    baseband: &str,
    modem_id: &str,
    apn: &str,
    profile_id: Option<u32>,
    cid: u8,
    families: &[u8],
) -> Result<Established, ImsBearerError> {
    let Some(first_family) = families.first().copied() else {
        return Err(ImsBearerError {
            kind: ImsBearerErrorKind::SessionStartFailed,
            hint: ImsBearerFailureHint::None,
            detail: "native_ims_no_address_family".to_string(),
        });
    };
    // qmicli accepts only `ip-type=4` or `ip-type=6`. On this DATA6 transport,
    // omitting the field for an IPv4v6 profile crashes the modem's DHCP manager.
    // A logical dual-stack attempt is therefore a safe probe of its preferred
    // family. The upper plan still handles a network-forced opposite family and
    // the remaining single-stack fallback attempts.
    let requested_family = Some(first_family);
    let session = start_session(endpoint, apn, requested_family, profile_id).await?;
    let sessions = vec![session];

    // Read the active context once from AT, which is beta8's IMS source of truth.
    let settings = match read_settings(modem_id, cid, apn).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_sessions(sessions).await;
            return Err(error);
        }
    };

    let netdev_family = if settings.ipv6_address.is_some() {
        6
    } else {
        4
    };
    let Some(config) = netdev_config_for(&settings, netdev_family) else {
        stop_sessions(sessions).await;
        return Err(settings_missing(
            "native_ims_session_has_no_address".to_string(),
        ));
    };
    let resolution = match qmi_netdev::resolve(baseband, &config, IMS_RESERVED_NETDEVS).await {
        Ok(resolution) => resolution,
        Err(error) => {
            stop_sessions(sessions).await;
            return Err(ImsBearerError {
                kind: ImsBearerErrorKind::NetdevUnresolved,
                hint: if matches!(error, qmi_netdev::NetdevError::LinkUnavailable(_)) {
                    ImsBearerFailureHint::BasebandWedged
                } else {
                    ImsBearerFailureHint::None
                },
                detail: format!("native_ims_netdev_unresolved:{error}"),
            });
        }
    };

    let info = ImsBearerInfo {
        interface: resolution.interface.clone(),
        netdev_method: resolution.method.as_str(),
        ip_type: ip_type_for(first_family).to_string(),
        path_device: endpoint.device_path.clone(),
        path_handle: joined_handles(&sessions),
        ipv4_address: settings.ipv4_address,
        ipv4_gateway: settings.ipv4_gateway,
        ipv4_dns: settings.ipv4_dns,
        ipv4_prefix: settings.ipv4_prefix,
        ipv6_address: settings.ipv6_address,
        ipv6_gateway: settings.ipv6_gateway,
        ipv6_dns: settings.ipv6_dns,
        ipv6_prefix: settings.ipv6_prefix,
        pcscf: settings.pcscf,
        interface_ownership: BearerInterfaceOwnership::ApplicationOwnedNative,
        ..Default::default()
    };
    Ok(Established {
        info,
        handle: Qcm410ImsBearerHandle {
            endpoint: endpoint.clone(),
            sessions,
            configured_netdevs: vec![(resolution.interface, config)],
        },
    })
}

fn ip_type_for(first_family: u8) -> &'static str {
    if first_family == 6 {
        "ipv6"
    } else {
        "ipv4"
    }
}

/// Start one retained IMS WDS session on the secondary endpoint. CID allocation
/// and network activation happen in the same qmicli process because DATA6 cannot
/// carry a retained CID across separate direct-QMI opens.
async fn start_session(
    endpoint: &SecondaryQmiEndpoint,
    apn: &str,
    family: Option<u8>,
    profile_id: Option<u32>,
) -> Result<ImsSession, ImsBearerError> {
    secondary_qmi::start_ims_session(endpoint, apn, family, profile_id)
        .await
        .map_err(|detail| ImsBearerError {
            kind: ImsBearerErrorKind::SessionStartFailed,
            hint: classify_session_failure(&detail),
            detail,
        })
}

fn classify_session_failure(detail: &str) -> ImsBearerFailureHint {
    let error = detail.to_ascii_lowercase();
    if error.contains("ipv6onlyallowed")
        || error.contains("ipv6-only-allowed")
        || error.contains("only ipv6 allowed")
        || error.contains("pdn-ipv4-call-disallowed")
    {
        ImsBearerFailureHint::NetworkForcedIpv6
    } else if error.contains("ipv4onlyallowed")
        || error.contains("ipv4-only-allowed")
        || error.contains("only ipv4 allowed")
        || error.contains("pdn-ipv6-call-disallowed")
    {
        ImsBearerFailureHint::NetworkForcedIpv4
    } else {
        let call_failed = error.contains("call failed") || error.contains("callfailed");
        let internal_error = error.contains("internal error") || error.contains("[internal] error");
        if error.contains("interface-in-use-config-match")
            || error.contains("endpoint hangup")
            || error.contains("mobileequipment.unknown")
            || (call_failed && internal_error)
        {
            ImsBearerFailureHint::BasebandWedged
        } else {
            ImsBearerFailureHint::None
        }
    }
}

/// Read the IMS context's IP configuration and P-CSCF from `AT+CGCONTRDP`.
///
/// A context that reports neither an address nor a P-CSCF is treated as missing
/// so the caller does not build an unusable bearer.
async fn read_settings(
    modem_id: &str,
    cid: u8,
    apn: &str,
) -> Result<CgcontrdpSettings, ImsBearerError> {
    let settings = cgcontrdp::read_cgcontrdp_settings(modem_id, cid, apn)
        .await
        .map_err(|error| settings_missing(format!("native_ims_cgcontrdp_read_failed:{error}")))?;
    if settings.ipv4_address.is_none() && settings.ipv6_address.is_none() {
        return Err(settings_missing(format!(
            "native_ims_cgcontrdp_no_address:cid={cid}"
        )));
    }
    Ok(settings)
}

async fn stop_sessions(sessions: Vec<ImsSession>) {
    for session in sessions {
        secondary_qmi::stop_ims_session(session).await;
    }
}

fn settings_missing(detail: String) -> ImsBearerError {
    ImsBearerError {
        kind: ImsBearerErrorKind::SettingsMissing,
        hint: ImsBearerFailureHint::None,
        detail,
    }
}

fn joined_handles(sessions: &[ImsSession]) -> String {
    sessions
        .iter()
        .map(|session| session.packet_data_handle.as_str())
        .collect::<Vec<_>>()
        .join("+")
}

/// Build the netdev probe configuration for the family the session came up on.
fn netdev_config_for(settings: &CgcontrdpSettings, family: u8) -> Option<NetdevConfig> {
    let (address, gateway, dns, prefix) = if family == 6 {
        (
            settings.ipv6_address,
            settings.ipv6_gateway,
            &settings.ipv6_dns,
            settings.ipv6_prefix,
        )
    } else {
        (
            settings.ipv4_address,
            settings.ipv4_gateway,
            &settings.ipv4_dns,
            settings.ipv4_prefix,
        )
    };
    let address = address?;
    Some(NetdevConfig::from_session(
        address, prefix, None, dns, gateway,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn reference_settings() -> CgcontrdpSettings {
        CgcontrdpSettings {
            ipv4_address: Some(IpAddr::V4(Ipv4Addr::new(10, 129, 39, 207))),
            ipv4_gateway: Some(IpAddr::V4(Ipv4Addr::new(10, 129, 39, 208))),
            ipv4_dns: vec![
                IpAddr::V4(Ipv4Addr::new(172, 17, 163, 218)),
                IpAddr::V4(Ipv4Addr::new(172, 17, 167, 218)),
            ],
            ipv4_prefix: Some(27),
            pcscf: vec![IpAddr::V4(Ipv4Addr::new(10, 11, 12, 13))],
            ..Default::default()
        }
    }

    #[test]
    fn netdev_config_picks_the_family_specific_address() {
        let settings = reference_settings();
        let config = netdev_config_for(&settings, 4).unwrap();
        assert_eq!(config.address, "10.129.39.207".parse::<IpAddr>().unwrap());
        assert_eq!(config.prefix, 27);
        // No v6 address in the reference settings, so a v6 request has nothing to
        // configure.
        assert!(netdev_config_for(&settings, 6).is_none());
    }

    #[test]
    fn ip_type_reflects_the_actual_safe_data6_family() {
        assert_eq!(ip_type_for(4), "ipv4");
        assert_eq!(ip_type_for(6), "ipv6");
    }
}
