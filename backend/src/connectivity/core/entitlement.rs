//! Shared E911 / TS.43 entitlement domain model.
//!
//! Clean-room from the public AOSP carrier entitlement client and GSMA
//! TS.43 recommendations (see `docs/E911_IMPLEMENTATION_RESEARCH.md`). This
//! module is deliberately transport-free: it defines the state machine, the
//! per-sub-status axis (`ProvStatus` / `TcStatus` / `AddrStatus`), the source
//! taxonomy and the provider kinds that every consumer (API, orchestrator,
//! VoWiFi capability gating) must agree on.
//!
//! The authoritative address copy lives in the operator's E911/provisioning
//! system. A local `provisioned` state never by itself claims the carrier
//! confirmed anything: only a successful entitlement re-query may move a
//! record to `Provisioned` with source `CarrierConfirmed`.

#![allow(dead_code)]

use std::fmt;

/// Coarse lifecycle for the entitlement exchange. See the research doc §9.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum E911State {
    /// No provider/evidence and no endpoint; do not guess.
    Unsupported,
    /// No record yet.
    Unknown,
    /// Provider exists but no user intent / no address captured.
    Unconfigured,
    /// An entitlement query is in flight.
    Querying,
    /// The carrier requires terms acceptance before address capture.
    NeedsTerms,
    /// The carrier requires a (civic) address before entitlement.
    NeedsAddress,
    /// The carrier requires a websheet or account action we cannot automate.
    NeedsUserAction,
    /// Address captured locally; provisioning in progress.
    Provisioning,
    /// Carrier returned a successful entitlement re-query.
    Provisioned,
    /// Carrier rejected the address / subscription.
    Rejected,
    /// Previously provisioned but the SIM context changed (e.g. eSIM profile
    /// switch) so the old confirmation is no longer valid.
    Stale,
    /// The carrier is temporarily unavailable; honour `retry_after`.
    TemporarilyUnavailable,
}

impl E911State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Unknown => "unknown",
            Self::Unconfigured => "unconfigured",
            Self::Querying => "querying",
            Self::NeedsTerms => "needs_terms",
            Self::NeedsAddress => "needs_address",
            Self::NeedsUserAction => "needs_user_action",
            Self::Provisioning => "provisioning",
            Self::Provisioned => "provisioned",
            Self::Rejected => "rejected",
            Self::Stale => "stale",
            Self::TemporarilyUnavailable => "temporarily_unavailable",
        }
    }

    /// Parse a state from the wire vocabulary. Unknown strings yield `None` so
    /// corrupt or forward-incompatible persisted records fail closed.
    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "unsupported" => Self::Unsupported,
            "unknown" => Self::Unknown,
            "unconfigured" => Self::Unconfigured,
            "querying" => Self::Querying,
            "needs_terms" => Self::NeedsTerms,
            "needs_address" => Self::NeedsAddress,
            "needs_user_action" => Self::NeedsUserAction,
            "provisioning" => Self::Provisioning,
            "provisioned" => Self::Provisioned,
            "rejected" => Self::Rejected,
            "stale" => Self::Stale,
            "temporarily_unavailable" => Self::TemporarilyUnavailable,
            _ => return None,
        })
    }

    /// Whether the operator has confirmed enough for VoWiFi to be entitled.
    /// Only `Provisioned` counts; `NeedsAddress`/`Provisioning` mean the local
    /// file holds user intent but the carrier has not confirmed it.
    pub fn is_entitled(self) -> bool {
        self == Self::Provisioned
    }
}

impl fmt::Display for E911State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where the current state came from. Mirrors AOSP's distinction between what
/// the carrier confirmed and what we merely believe locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum E911StateSource {
    /// Carrier read back a positive result during the latest entitlement
    /// re-query.
    CarrierConfirmed,
    /// Carrier declared the address during an earlier exchange (rare; still
    /// re-verified on re-query).
    CarrierDeclared,
    /// User-entered intent that has never been confirmed by the carrier.
    LocalOnly,
    /// No evidence.
    Unknown,
}

impl E911StateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CarrierConfirmed => "carrier_confirmed",
            Self::CarrierDeclared => "carrier_declared",
            Self::LocalOnly => "local_only",
            Self::Unknown => "unknown",
        }
    }
}

/// TS.43 three-axis status kept separate so the API/UI can report "operator
/// requires", "address saved locally", "operator confirmed" independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementStatus {
    /// Provisioning entitlement (subscription-level).
    ProvStatus,
    /// Terms & conditions acceptance.
    TcStatus,
    /// E911 address status.
    AddrStatus,
}

impl EntitlementStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProvStatus => "prov_status",
            Self::TcStatus => "tc_status",
            Self::AddrStatus => "addr_status",
        }
    }
}

/// The value carried on one of the three status axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntitlementStatusValue {
    /// Not required / not applicable for this carrier.
    NotRequired,
    /// Not yet satisfied.
    NotSet,
    /// Satisfied / confirmed by the carrier.
    Set,
    /// Explicitly rejected by the carrier.
    Rejected,
    /// Unknown / no evidence.
    Unknown,
}

impl EntitlementStatusValue {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotRequired => "not_required",
            Self::NotSet => "not_set",
            Self::Set => "set",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }
}

/// Provider taxonomy from the research doc §9.4. Unknown operators default to
/// [`ProviderKind::MetadataOnly`] and never auto-hit other endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Standard TS.43 query + EAP-AKA + websheet.
    Ts43,
    /// Guide the user to the operator's official account page; only re-query
    /// afterwards.
    ExternalPortal,
    /// Only for a confirmed native operator API.
    NativeVerified,
    /// Only hint that E911 is required; no network request.
    MetadataOnly,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ts43 => "ts43",
            Self::ExternalPortal => "external_portal",
            Self::NativeVerified => "native_verified",
            Self::MetadataOnly => "metadata_only",
        }
    }

    /// Parse from the wire vocabulary. Unknown strings fail closed to
    /// `MetadataOnly` so a bad persisted value can never auto-trigger requests.
    pub fn from_str(value: &str) -> Option<Self> {
        Some(match value {
            "ts43" => Self::Ts43,
            "external_portal" => Self::ExternalPortal,
            "native_verified" => Self::NativeVerified,
            "metadata_only" => Self::MetadataOnly,
            _ => return None,
        })
    }

    /// Whether any automated entitlement network request may be made.
    pub fn may_query(self) -> bool {
        matches!(self, Self::Ts43 | Self::NativeVerified)
    }
}

/// The stored per-SIM entitlement record. Everything here is non-secret; the
/// token / cookie / `ServerFlow_User_Data` live in the E911 secret store.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct E911EntitlementRecord {
    pub state: E911State,
    pub source: E911StateSource,
    pub prov_status: EntitlementStatusValue,
    pub tc_status: EntitlementStatusValue,
    pub addr_status: EntitlementStatusValue,
    /// Unix timestamp (seconds) of the last carrier confirmation read-back.
    pub confirmed_at_epoch: Option<i64>,
    /// Unix timestamp (seconds) after which a retry is allowed.
    pub retry_after_epoch: Option<i64>,
    /// Opaque carrier reference; never the address itself.
    pub provider_reference: Option<String>,
    /// True when the carrier signalled the stored address needs re-confirmation.
    pub needs_reconfirm: bool,
}

impl Default for E911EntitlementRecord {
    fn default() -> Self {
        Self {
            state: E911State::Unknown,
            source: E911StateSource::Unknown,
            prov_status: EntitlementStatusValue::Unknown,
            tc_status: EntitlementStatusValue::Unknown,
            addr_status: EntitlementStatusValue::Unknown,
            confirmed_at_epoch: None,
            retry_after_epoch: None,
            provider_reference: None,
            needs_reconfirm: false,
        }
    }
}

impl E911EntitlementRecord {
    /// Whether the record currently satisfies the carrier's entitlement. This
    /// is the only value VoWiFi capability gating should consume.
    pub fn is_provisioned(&self) -> bool {
        self.state == E911State::Provisioned
            && self.source == E911StateSource::CarrierConfirmed
            && self.addr_status == EntitlementStatusValue::Set
    }

    /// Mark the record stale (e.g. after an eSIM profile switch / SIM swap).
    /// Carrier confirmation is never carried across a changed binding.
    pub fn invalidate(&mut self) {
        if self.state == E911State::Provisioned {
            self.state = E911State::Stale;
        }
        self.needs_reconfirm = true;
    }
}

/// Result of a parsed TS.43 entitlement query response (server-facing facts).
/// Secrets (`ServiceFlow_UserData`, also called `ServerFlow_User_Data` by older
/// carriers) are deliberately kept out of this struct so
/// a debug print can never leak them; the orchestrator routes them to the
/// secret store.
#[derive(Clone, PartialEq, Eq)]
pub struct EntitlementQueryOutcome {
    pub state: E911State,
    /// Top-level TS.43 `EntitlementStatus` for the VoWiFi application.
    pub entitlement_status: EntitlementStatusValue,
    pub prov_status: EntitlementStatusValue,
    pub tc_status: EntitlementStatusValue,
    pub addr_status: EntitlementStatusValue,
    /// Opaque provider reference (e.g. `<ref>` element). Not the address.
    pub provider_reference: Option<String>,
    /// When non-empty, the carrier wants a websheet at this URL (SSRF-checked
    /// by the caller before any network I/O).
    pub server_flow_url: Option<String>,
    /// When the websheet needs CSRF/state data the server hands back. This is a
    /// secret: the caller must not log it and must store it encrypted.
    pub server_flow_user_data: Option<String>,
    /// Seconds the carrier asked us to wait before the next query.
    pub retry_after_seconds: Option<u64>,
}

impl EntitlementQueryOutcome {
    /// True only when this response, on its own, confirms provisioning. A
    /// websheet response (`server_flow_url` set) never confirms anything.
    pub fn is_carrier_confirmed(&self) -> bool {
        self.server_flow_url.is_none()
            && self.entitlement_status == EntitlementStatusValue::Set
            && self.prov_status == EntitlementStatusValue::Set
            && self.tc_status != EntitlementStatusValue::Rejected
            && self.addr_status == EntitlementStatusValue::Set
    }
}

impl std::fmt::Debug for EntitlementQueryOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntitlementQueryOutcome")
            .field("state", &self.state)
            .field("entitlement_status", &self.entitlement_status)
            .field("prov_status", &self.prov_status)
            .field("tc_status", &self.tc_status)
            .field("addr_status", &self.addr_status)
            .field("provider_reference", &self.provider_reference)
            .field("server_flow_url_present", &self.server_flow_url.is_some())
            .field(
                "server_flow_user_data_present",
                &self.server_flow_user_data.is_some(),
            )
            .field("retry_after_seconds", &self.retry_after_seconds)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_vocabulary_round_trips() {
        for state in [
            E911State::Unsupported,
            E911State::Unknown,
            E911State::Unconfigured,
            E911State::Querying,
            E911State::NeedsTerms,
            E911State::NeedsAddress,
            E911State::NeedsUserAction,
            E911State::Provisioning,
            E911State::Provisioned,
            E911State::Rejected,
            E911State::Stale,
            E911State::TemporarilyUnavailable,
        ] {
            assert_eq!(E911State::from_str(state.as_str()), Some(state));
            assert_eq!(state.to_string(), state.as_str());
        }
        assert_eq!(E911State::from_str("not_a_state"), None);
    }

    #[test]
    fn only_provisioned_is_entitled() {
        assert!(E911State::Provisioned.is_entitled());
        for state in [
            E911State::Unsupported,
            E911State::Unknown,
            E911State::Unconfigured,
            E911State::NeedsAddress,
            E911State::Provisioning,
            E911State::Rejected,
            E911State::Stale,
            E911State::TemporarilyUnavailable,
        ] {
            assert!(!state.is_entitled());
        }
    }

    #[test]
    fn is_provisioned_requires_carrier_confirmed_address() {
        let mut record = E911EntitlementRecord {
            state: E911State::Provisioned,
            source: E911StateSource::CarrierConfirmed,
            addr_status: EntitlementStatusValue::Set,
            ..Default::default()
        };
        assert!(record.is_provisioned());

        record.source = E911StateSource::LocalOnly;
        assert!(!record.is_provisioned());
        record.source = E911StateSource::CarrierConfirmed;

        record.state = E911State::NeedsAddress;
        assert!(!record.is_provisioned());
        record.state = E911State::Provisioned;

        record.addr_status = EntitlementStatusValue::NotSet;
        assert!(!record.is_provisioned());
    }

    #[test]
    fn invalidate_never_pretends_confirmation_survives_binding_change() {
        let mut record = E911EntitlementRecord {
            state: E911State::Provisioned,
            source: E911StateSource::CarrierConfirmed,
            addr_status: EntitlementStatusValue::Set,
            needs_reconfirm: false,
            ..Default::default()
        };
        record.invalidate();
        assert_eq!(record.state, E911State::Stale);
        assert!(record.needs_reconfirm);
        assert!(!record.is_provisioned());
    }

    #[test]
    fn outcome_with_server_flow_never_confirms() {
        let confirmed = EntitlementQueryOutcome {
            state: E911State::Provisioned,
            entitlement_status: EntitlementStatusValue::Set,
            prov_status: EntitlementStatusValue::Set,
            tc_status: EntitlementStatusValue::Set,
            addr_status: EntitlementStatusValue::Set,
            provider_reference: Some("ref-1".to_string()),
            server_flow_url: None,
            server_flow_user_data: None,
            retry_after_seconds: None,
        };
        assert!(confirmed.is_carrier_confirmed());

        let websheet = EntitlementQueryOutcome {
            server_flow_url: Some("https://carrier.example/websheet".to_string()),
            ..confirmed.clone()
        };
        assert!(!websheet.is_carrier_confirmed());

        let rejected = EntitlementQueryOutcome {
            addr_status: EntitlementStatusValue::Rejected,
            ..confirmed
        };
        assert!(!rejected.is_carrier_confirmed());
    }

    #[test]
    fn provider_kinds_parse_and_gate_queries() {
        assert_eq!(ProviderKind::Ts43.as_str(), "ts43");
        assert_eq!(
            ProviderKind::from_str(ProviderKind::NativeVerified.as_str()),
            Some(ProviderKind::NativeVerified)
        );
        assert_eq!(ProviderKind::from_str("bogus"), None);
        assert!(ProviderKind::Ts43.may_query());
        assert!(ProviderKind::NativeVerified.may_query());
        assert!(!ProviderKind::ExternalPortal.may_query());
        assert!(!ProviderKind::MetadataOnly.may_query());
    }
}
