//! Device-agnostic contracts for native modem transports.

use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// Radio access technology carrying a 3GPP IMS bearer.
///
/// This describes the bearer that was actually established.  It must not be
/// inferred merely from a modem advertising 5G support or seeing an NR cell.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreeGppRat {
    #[default]
    Unknown,
    Lte,
    NrNsa,
    NrSa,
}

/// Packet-core domain that owns the bearer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BearerDomain {
    #[default]
    Unknown,
    /// EPS bearer in EPC (the existing VoLTE path).
    Eps,
    /// PDU session in the 5G Core.
    #[serde(rename = "5gs")]
    FiveGs,
}

/// Ownership of the network interface exposed by a bearer provider.
///
/// Namespace migration must eventually key off this value instead of an
/// interface name such as `wwan0`: a host-managed primary interface must stay
/// with ModemManager, whereas a SimAdmin-owned secondary interface may be moved
/// into the line worker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BearerInterfaceOwnership {
    #[default]
    Unknown,
    HostManagedPrimary,
    SimAdminOwnedSecondary,
    WorkerNative,
}

/// Optional 5GS PDU-session metadata supplied by a capable bearer provider.
///
/// Existing LTE/QMI providers leave this as `None`.  Keeping the fields
/// optional also lets ModemManager/MBIM implementations expose only the subset
/// reported by their modem without inventing values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PduSessionInfo {
    pub session_id: Option<u8>,
    pub dnn: Option<String>,
    pub s_nssai: Option<String>,
    pub ssc_mode: Option<u8>,
}

/// Optional 5G QoS-flow metadata associated with a PDU session.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QosFlowInfo {
    pub qfi: Option<u8>,
    pub five_qi: Option<u16>,
    pub arp_priority: Option<u8>,
    pub gbr_uplink_bps: Option<u64>,
    pub gbr_downlink_bps: Option<u64>,
    pub mbr_uplink_bps: Option<u64>,
    pub mbr_downlink_bps: Option<u64>,
}

/// Device-agnostic description of an established native IMS bearer.
///
/// This is what an upper protocol layer consumes: enough to build its own
/// connection contract (addresses, DNS, P-CSCF, prefixes, interface) and to log
/// how the interface was decided, plus the two strings the synthetic bearer path
/// is made from. The WDS session handle itself stays opaque behind
/// [`ImsBearerHandle`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImsBearerInfo {
    /// Interface that carries the session, e.g. `wwan3`.
    pub interface: String,
    /// How the interface was decided (`sole_candidate` / `probe_answered` /
    /// `assumed`).
    pub netdev_method: &'static str,
    /// `ipv4`, `ipv6` or `ipv4v6`.
    pub ip_type: String,
    /// Device path the session's QMI endpoint bound (used for the synthetic
    /// bearer path).
    pub path_device: String,
    /// Retained WDS packet-data handles, joined (used for the synthetic bearer
    /// path).
    pub path_handle: String,
    pub ipv4_address: Option<IpAddr>,
    pub ipv4_gateway: Option<IpAddr>,
    pub ipv4_dns: Vec<IpAddr>,
    pub ipv4_prefix: Option<u8>,
    pub ipv6_address: Option<IpAddr>,
    pub ipv6_gateway: Option<IpAddr>,
    pub ipv6_dns: Vec<IpAddr>,
    pub ipv6_prefix: Option<u8>,
    pub pcscf: Vec<IpAddr>,
    /// Observed access technology and packet-core domain. `Unknown` means the
    /// provider did not expose the value; it never means VoNR is ready.
    pub rat: ThreeGppRat,
    pub bearer_domain: BearerDomain,
    /// Whether the interface may be moved into a per-UE worker namespace.
    pub interface_ownership: BearerInterfaceOwnership,
    /// 5GS-only details. LTE/EPS providers normally leave these empty.
    pub pdu_session: Option<PduSessionInfo>,
    pub qos_flows: Vec<QosFlowInfo>,
}

/// Opaque teardown handle for an established IMS bearer.
///
/// Dropping it without calling [`Self::release`] would leak the WDS session and
/// its endpoint, so callers are expected to drive teardown explicitly (the
/// strategy layer owns the handle until the call/session is over).
///
/// The teardown is returned as a boxed future so the trait stays object-safe and
/// can be held as `Box<dyn ImsBearerHandle + Send>` by upper layers.
pub trait ImsBearerHandle: Send {
    /// Stop the WDS session(s) and release the endpoint and netdev addresses.
    fn release(self: Box<Self>) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
}

/// Why establishing an IMS bearer failed, so an upper layer can classify the
/// error without knowing the device's transport details.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImsBearerErrorKind {
    /// The primary device could not be mapped to a baseband.
    BasebandUnresolved,
    /// No secondary endpoint could be obtained (bound) for the device.
    EndpointUnavailable,
    /// The WDS session failed to start. `detail` carries the stable
    /// `secondary_qmi_start_failed:...` string the baseband-wedge classifier
    /// keys off.
    SessionStartFailed,
    /// The IMS context reported no usable IP configuration / P-CSCF.
    SettingsMissing,
    /// The bam-dmux netdev for the session could not be resolved.
    NetdevUnresolved,
}

/// A device IMS bearer failure with a stable `detail` string for
/// classification, mirroring the pre-existing `VolteError` detail vocabulary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsBearerError {
    pub kind: ImsBearerErrorKind,
    pub detail: String,
}

impl fmt::Display for ImsBearerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

/// Native IMS bearer transport: establishes a raw (device-native) IMS bearer.
///
/// One call is one self-contained attempt on `primary_device`: it brings up the
/// WDS session(s), reads the IMS context settings, resolves the netdev and hands
/// back a device-agnostic [`ImsBearerInfo`] plus an opaque [`ImsBearerHandle`]
/// that tears the session down again. On failure the implementation is
/// responsible for releasing anything it bound.
pub trait ImsBearerTransport: Send + Sync {
    type Error: std::fmt::Display + Send + Sync + 'static;

    /// Establish one IMS bearer for the given address families.
    ///
    /// `families` carries one QMI `ip-type` value (`4` or `6`) for a
    /// single-family attempt, or both, in the plan's start order, for a
    /// `ipv4v6` attempt. The driver is free to implement dual-stack as two
    /// independent sessions on its own.
    ///
    /// `modem_id` is the mmcli selector used to read `+CGCONTRDP`;
    /// `profile_id` is the `3gpp-profile` to start the WDS session with; `cid`
    /// is the AT PDP context id whose settings describe the session.
    async fn establish_ims_bearer(
        &self,
        primary_device: &str,
        modem_id: &str,
        apn: &str,
        profile_id: Option<u32>,
        cid: u8,
        families: &[u8],
    ) -> Result<(ImsBearerInfo, Box<dyn ImsBearerHandle + Send>), Self::Error>;
}
