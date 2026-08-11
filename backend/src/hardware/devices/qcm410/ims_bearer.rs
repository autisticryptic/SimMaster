//! Qualcomm 410 (MSM8916-class) native IMS bearer driver.
//!
//! Implements [`ImsBearerTransport`] over the spare `DATA*_CNTL` rpmsg channels:
//! this is the "DATA6" path. It binds/obtains the secondary QMI endpoint for the
//! device's baseband, starts a retained WDS IMS session (one per family, or one
//! dual-stack), reads the IMS context's IP configuration and P-CSCF from
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
use crate::hardware::cellular::qmi_netdev::{self, NetdevConfig};
use crate::hardware::devices::qcm410::secondary_qmi::{self, ImsSession, SecondaryQmiEndpoint};
use crate::hardware::devices::transport::{
    ImsBearerError, ImsBearerErrorKind, ImsBearerHandle, ImsBearerInfo, ImsBearerTransport,
};

/// The qcm410 IMS bearer driver. Stateless; one instance serves every line.
pub struct Qcm410ImsBearer;

/// Everything needed to tear one established bearer down again.
pub struct Qcm410ImsBearerHandle {
    /// Secondary QMI endpoint the session(s) run on. Held so teardown can stop
    /// the sessions and release the endpoint.
    endpoint: SecondaryQmiEndpoint,
    /// Retained WDS clients to stop and release on teardown. One per family for
    /// a dual-stack bearer; exactly one for a single-family bearer.
    sessions: Vec<ImsSession>,
    /// Family-specific addresses and policy routes installed for the retained
    /// WDS session(s). They must be removed without flushing the shared netdev.
    configured_netdevs: Vec<(String, NetdevConfig)>,
}

impl ImsBearerHandle for Qcm410ImsBearerHandle {
    fn release(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(async move {
            for (interface, config) in &self.configured_netdevs {
                qmi_netdev::teardown(interface, config).await;
            }
            for session in &self.sessions {
                secondary_qmi::stop_ims_session(&self.endpoint, session).await;
            }
            secondary_qmi::release_endpoint(&self.endpoint).await;
        })
    }
}

impl ImsBearerTransport for Qcm410ImsBearer {
    type Error = ImsBearerError;

    async fn establish_ims_bearer(
        &self,
        primary_device: &str,
        modem_id: &str,
        apn: &str,
        profile_id: Option<u32>,
        cid: u8,
        families: &[u8],
    ) -> Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), ImsBearerError> {
        // `primary_device` is the line's primary QMI control port; it is used
        // only to find the *baseband*, so the secondary endpoint and the netdev
        // are paired to the same modem (multi-line correctness). The IMS session
        // itself never touches the primary port — that stays with ModemManager.
        let baseband = secondary_qmi::baseband_key_for_device(primary_device).map_err(|error| {
            ImsBearerError {
                kind: ImsBearerErrorKind::BasebandUnresolved,
                detail: format!("native_ims_baseband_unresolved:{error}"),
            }
        })?;

        let endpoint = secondary_qmi::runtime_endpoint(primary_device)
            .await
            .map_err(|error| ImsBearerError {
                kind: ImsBearerErrorKind::EndpointUnavailable,
                detail: error.to_string(),
            })?;

        let result = establish_bearer(
            &endpoint, &baseband, modem_id, apn, profile_id, cid, families,
        )
        .await;
        match result {
            Ok(established) => Ok((established.info, Box::new(established.handle))),
            Err(error) => {
                secondary_qmi::release_endpoint(&endpoint).await;
                Err(error)
            }
        }
    }
}

struct Established {
    info: ImsBearerInfo,
    handle: Qcm410ImsBearerHandle,
}

/// Start the retained WDS session(s) for `families`, read the IMS context
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
    let dual = families.len() > 1;
    let mut sessions = Vec::with_capacity(families.len());
    for family in families.iter().copied() {
        match start_session(endpoint, apn, family, profile_id).await {
            Ok(session) => sessions.push(session),
            Err(error) => {
                stop_sessions(endpoint, &sessions).await;
                return Err(error);
            }
        }
    }

    // Both retained sessions are up; the modem now describes the merged context.
    // Read it once from AT, which is beta8's IMS source of truth.
    let settings = match read_settings(modem_id, cid, apn).await {
        Ok(settings) => settings,
        Err(error) => {
            stop_sessions(endpoint, &sessions).await;
            return Err(error);
        }
    };

    let netdev_family = if settings.ipv6_address.is_some() {
        6
    } else {
        4
    };
    let Some(config) = netdev_config_for(&settings, netdev_family) else {
        stop_sessions(endpoint, &sessions).await;
        return Err(settings_missing(
            "native_ims_session_has_no_address".to_string(),
        ));
    };
    let resolution = match qmi_netdev::resolve(baseband, &config).await {
        Ok(resolution) => resolution,
        Err(error) => {
            stop_sessions(endpoint, &sessions).await;
            return Err(ImsBearerError {
                kind: ImsBearerErrorKind::NetdevUnresolved,
                detail: format!("native_ims_netdev_unresolved:{error}"),
            });
        }
    };

    let info = ImsBearerInfo {
        interface: resolution.interface.clone(),
        netdev_method: resolution.method.as_str(),
        ip_type: ip_type_for(dual, families[0]).to_string(),
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

fn ip_type_for(dual: bool, first_family: u8) -> &'static str {
    if dual {
        "ipv4v6"
    } else if first_family == 6 {
        "ipv6"
    } else {
        "ipv4"
    }
}

/// Start one retained IMS WDS session on the secondary endpoint. The driver
/// allocates a CID with `wds-noop`, sets the family, starts the network and
/// keeps that CID for settings and teardown.
async fn start_session(
    endpoint: &SecondaryQmiEndpoint,
    apn: &str,
    family: u8,
    profile_id: Option<u32>,
) -> Result<ImsSession, ImsBearerError> {
    secondary_qmi::start_ims_session(endpoint, apn, family, profile_id)
        .await
        .map_err(|detail| ImsBearerError {
            kind: ImsBearerErrorKind::SessionStartFailed,
            detail,
        })
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

async fn stop_sessions(endpoint: &SecondaryQmiEndpoint, sessions: &[ImsSession]) {
    for session in sessions {
        secondary_qmi::stop_ims_session(endpoint, session).await;
    }
}

fn settings_missing(detail: String) -> ImsBearerError {
    ImsBearerError {
        kind: ImsBearerErrorKind::SettingsMissing,
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
    fn ip_type_reflects_single_vs_dual() {
        assert_eq!(ip_type_for(false, 4), "ipv4");
        assert_eq!(ip_type_for(false, 6), "ipv6");
        assert_eq!(ip_type_for(true, 4), "ipv4v6");
        assert_eq!(ip_type_for(true, 6), "ipv4v6");
    }
}
