//! IMS access-network identity shared by VoLTE and VoWiFi REGISTER builders.
//!
//! Carrier policy decides whether a header is sent. This module only formats
//! an identity when the serving PLMN and cell values came from a real runtime
//! snapshot; it never manufactures a PLMN/TAC/cell-id placeholder.

use std::{
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

/// Serving-cell information is refreshed by the line registry every ten
/// seconds. Keep one additional interval of slack for a slow ModemManager pass,
/// but never reuse a cell identity indefinitely after a modem restart or loss
/// of service.
pub const DEFAULT_IMS_ACCESS_NETWORK_MAX_AGE: Duration = Duration::from_secs(30);

/// Profile-controlled source policy for a PANI/CNI identity.
///
/// `dynamic_if_known` prefers a real serving-cell snapshot and falls back to
/// the explicitly configured static value. `required_dynamic` never emits a
/// static or fabricated cell identity when the runtime snapshot is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessIdentityPolicy {
    Omit,
    Static,
    DynamicIfKnown,
    RequiredDynamic,
}

impl AccessIdentityPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Omit => "omit",
            Self::Static => "static",
            Self::DynamicIfKnown => "dynamic_if_known",
            Self::RequiredDynamic => "required_dynamic",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessIdentitySource {
    Dynamic,
    StaticProfile,
    CompatibilityFallback,
    Omitted,
    RequiredDynamicMissing,
}

impl AccessIdentitySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::StaticProfile => "static_profile",
            Self::CompatibilityFallback => "compatibility_fallback",
            Self::Omitted => "omitted",
            Self::RequiredDynamicMissing => "required_dynamic_missing",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessIdentityResolution {
    pub value: Option<String>,
    pub source: AccessIdentitySource,
}

impl AccessIdentityResolution {
    pub fn required_dynamic_missing(&self) -> bool {
        self.source == AccessIdentitySource::RequiredDynamicMissing
    }
}

/// Resolve a PANI/CNI value without ever manufacturing a cell identity.
///
/// The caller decides which runtime context is relevant. In particular, a
/// VoWiFi PANI must pass no cellular context because the cellular snapshot is
/// only valid for CNI on that access leg.
pub fn resolve_access_identity(
    policy: AccessIdentityPolicy,
    static_value: Option<&str>,
    dynamic_context: Option<&ImsAccessNetworkContext>,
) -> AccessIdentityResolution {
    let dynamic_value = dynamic_context.map(ImsAccessNetworkContext::cellular_access_info);
    let static_value = static_value.and_then(sanitize_header_value);
    match policy {
        AccessIdentityPolicy::Omit => AccessIdentityResolution {
            value: None,
            source: AccessIdentitySource::Omitted,
        },
        AccessIdentityPolicy::Static => AccessIdentityResolution {
            source: if static_value.is_some() {
                AccessIdentitySource::StaticProfile
            } else {
                AccessIdentitySource::Omitted
            },
            value: static_value,
        },
        AccessIdentityPolicy::DynamicIfKnown => {
            if let Some(value) = dynamic_value {
                AccessIdentityResolution {
                    value: Some(value),
                    source: AccessIdentitySource::Dynamic,
                }
            } else {
                AccessIdentityResolution {
                    source: if static_value.is_some() {
                        AccessIdentitySource::CompatibilityFallback
                    } else {
                        AccessIdentitySource::Omitted
                    },
                    value: static_value,
                }
            }
        }
        AccessIdentityPolicy::RequiredDynamic => AccessIdentityResolution {
            source: if dynamic_value.is_some() {
                AccessIdentitySource::Dynamic
            } else {
                AccessIdentitySource::RequiredDynamicMissing
            },
            value: dynamic_value,
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImsAccessType {
    EutranFdd,
    EutranTdd,
    Iwlan,
    NrFdd,
    NrTdd,
}

impl ImsAccessType {
    pub const fn as_3gpp_token(self) -> &'static str {
        match self {
            Self::EutranFdd => "3GPP-E-UTRAN-FDD",
            Self::EutranTdd => "3GPP-E-UTRAN-TDD",
            Self::Iwlan => "3GPP-IWLAN",
            Self::NrFdd => "3GPP-NR-FDD",
            Self::NrTdd => "3GPP-NR-TDD",
        }
    }

    fn cellular_identity_widths(self) -> Option<(u32, usize, u64, usize)> {
        match self {
            // TS 24.229 table 7.2A.4-1: 16-bit TAC + 28-bit E-UTRAN cell id.
            Self::EutranFdd | Self::EutranTdd => Some((0xffff, 4, 0x0fff_ffff, 7)),
            // TS 24.229 table 7.2A.4-1: 24-bit TAC + 36-bit NR cell identity.
            Self::NrFdd | Self::NrTdd => Some((0x00ff_ffff, 6, 0x0f_ffff_ffff, 9)),
            Self::Iwlan => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessNetworkSource {
    ModemManager,
    TestFixture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImsAccessNetworkContext {
    pub access_type: ImsAccessType,
    pub serving_plmn: String,
    pub cell_id: u64,
    pub tac: u32,
    pub cell_info_age_seconds: Option<u32>,
    captured_at: Option<Instant>,
    pub source: AccessNetworkSource,
}

impl ImsAccessNetworkContext {
    pub fn new(
        access_type: ImsAccessType,
        serving_plmn: impl Into<String>,
        cell_id: u64,
        tac: u32,
        cell_info_age_seconds: Option<u32>,
        source: AccessNetworkSource,
    ) -> Option<Self> {
        let serving_plmn = serving_plmn.into();
        if !valid_plmn(&serving_plmn) || cell_id == 0 || tac == 0 {
            return None;
        }
        let (max_tac, _, max_cell_id, _) = access_type.cellular_identity_widths()?;
        if tac > max_tac || cell_id > max_cell_id {
            return None;
        }
        Some(Self {
            access_type,
            serving_plmn,
            cell_id,
            tac,
            cell_info_age_seconds,
            captured_at: None,
            source,
        })
    }

    /// Compatibility constructor for callers that already hold a single modem
    /// observation. New live code should publish a [`ServingAccessSnapshot`] to
    /// [`ImsAccessNetworkRuntime`] and resolve it at message-build time.
    pub fn from_modem_snapshot(
        profile_access_info: &str,
        mcc: Option<&str>,
        mnc: Option<&str>,
        technology: &str,
        cell_id: u64,
        tac: u32,
        serving_band: Option<&str>,
    ) -> Option<Self> {
        ServingAccessSnapshot::new(
            mcc?,
            mnc?,
            technology,
            cell_id,
            tac,
            serving_band.map(str::to_string),
            AccessNetworkSource::ModemManager,
        )?
        .resolve(profile_access_info)
    }

    /// A standards-shaped cellular access value suitable for PANI or CNI.
    pub fn cellular_access_info(&self) -> String {
        let (_, tac_width, _, cell_width) = self
            .access_type
            .cellular_identity_widths()
            .expect("validated cellular access type");
        let mut value = format!(
            "{};utran-cell-id-3gpp={}{:0tac_width$X}{:0cell_width$X}",
            self.access_type.as_3gpp_token(),
            self.serving_plmn,
            self.tac,
            self.cell_id,
        );
        if let Some(base_age) = self.cell_info_age_seconds {
            let elapsed = self
                .captured_at
                .map(|captured| u32::try_from(captured.elapsed().as_secs()).unwrap_or(u32::MAX))
                .unwrap_or(0);
            value.push_str(&format!(
                ";cell-info-age={}",
                base_age.saturating_add(elapsed)
            ));
        }
        value
    }
}

/// Raw, profile-independent serving information captured for one physical
/// line. The profile hint is consulted only when the modem did not expose a
/// band from which FDD/TDD can be determined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServingAccessSnapshot {
    pub serving_plmn: String,
    pub technology: String,
    pub cell_id: u64,
    pub tac: u32,
    pub serving_band: Option<String>,
    pub source: AccessNetworkSource,
    captured_at: Instant,
}

impl ServingAccessSnapshot {
    pub fn new(
        mcc: &str,
        mnc: &str,
        technology: &str,
        cell_id: u64,
        tac: u32,
        serving_band: Option<String>,
        source: AccessNetworkSource,
    ) -> Option<Self> {
        let serving_plmn = serving_plmn(mcc, mnc)?;
        let technology = technology.trim().to_ascii_lowercase();
        if !matches!(technology.as_str(), "lte" | "nr") || cell_id == 0 || tac == 0 {
            return None;
        }
        let serving_band = serving_band
            .and_then(|value| sanitize_header_value(&value))
            .filter(|value| !value.is_empty());
        Some(Self {
            serving_plmn,
            technology,
            cell_id,
            tac,
            serving_band,
            source,
            captured_at: Instant::now(),
        })
    }

    pub fn age(&self) -> Duration {
        self.captured_at.elapsed()
    }

    pub fn resolve(&self, profile_access_info: &str) -> Option<ImsAccessNetworkContext> {
        let access_type = match self.technology.as_str() {
            "lte" => infer_lte_access_type(profile_access_info, self.serving_band.as_deref())?,
            "nr" => infer_nr_access_type(profile_access_info, self.serving_band.as_deref())?,
            _ => return None,
        };
        let mut context = ImsAccessNetworkContext::new(
            access_type,
            self.serving_plmn.clone(),
            self.cell_id,
            self.tac,
            Some(0),
            self.source,
        )?;
        context.captured_at = Some(self.captured_at);
        Some(context)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AccessNetworkRuntimeStatus {
    pub available: bool,
    pub stale: bool,
    pub technology: Option<String>,
    pub serving_plmn: Option<String>,
    pub age_seconds: Option<u64>,
    pub last_error: Option<String>,
}

/// Fresh serving-area facts that may be used for standards-based ePDG
/// selection. This deliberately excludes cell-id and profile-derived values:
/// only a real per-line LTE/NR snapshot can produce it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpdgLocationSnapshot {
    pub serving_plmn: String,
    pub technology: String,
    pub tac: u32,
}

#[derive(Debug, Default)]
struct AccessNetworkRuntimeState {
    snapshot: Option<ServingAccessSnapshot>,
    last_error: Option<String>,
}

/// Per-line mutable serving-cell context. Clones share only the state owned by
/// the same [`LineRuntime`](crate::services::line_registry::LineRuntime); there
/// is deliberately no process-wide map that could mix two modems' identities.
#[derive(Clone, Default)]
pub struct ImsAccessNetworkRuntime {
    inner: Arc<RwLock<AccessNetworkRuntimeState>>,
}

impl std::fmt::Debug for ImsAccessNetworkRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ImsAccessNetworkRuntime")
            .field("status", &self.status(DEFAULT_IMS_ACCESS_NETWORK_MAX_AGE))
            .finish()
    }
}

impl PartialEq for ImsAccessNetworkRuntime {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for ImsAccessNetworkRuntime {}

impl ImsAccessNetworkRuntime {
    pub fn publish(&self, snapshot: ServingAccessSnapshot) {
        let mut state = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.snapshot = Some(snapshot);
        state.last_error = None;
    }

    /// Record a refresh failure while retaining the last snapshot until its TTL
    /// expires. This tolerates one transient D-Bus failure without allowing an
    /// old cell identity to survive indefinitely.
    pub fn record_refresh_error(&self, reason: impl Into<String>) {
        self.inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last_error = Some(reason.into());
    }

    /// Clear the serving identity immediately for authoritative state changes
    /// such as line removal, modem restart, or a confirmed no-service result.
    pub fn clear(&self, reason: impl Into<String>) {
        let mut state = self
            .inner
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.snapshot = None;
        state.last_error = Some(reason.into());
    }

    pub fn context(&self, profile_access_info: &str) -> Option<ImsAccessNetworkContext> {
        self.context_with_max_age(profile_access_info, DEFAULT_IMS_ACCESS_NETWORK_MAX_AGE)
    }

    pub fn context_with_max_age(
        &self,
        profile_access_info: &str,
        max_age: Duration,
    ) -> Option<ImsAccessNetworkContext> {
        let snapshot = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot
            .clone()?;
        if snapshot.age() > max_age {
            return None;
        }
        snapshot.resolve(profile_access_info)
    }

    pub fn epdg_location(&self) -> Option<EpdgLocationSnapshot> {
        self.epdg_location_with_max_age(DEFAULT_IMS_ACCESS_NETWORK_MAX_AGE)
    }

    pub fn epdg_location_with_max_age(&self, max_age: Duration) -> Option<EpdgLocationSnapshot> {
        let snapshot = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot
            .clone()?;
        if snapshot.age() > max_age || !matches!(snapshot.technology.as_str(), "lte" | "nr") {
            return None;
        }
        Some(EpdgLocationSnapshot {
            serving_plmn: snapshot.serving_plmn,
            technology: snapshot.technology,
            tac: snapshot.tac,
        })
    }

    pub fn status(&self, max_age: Duration) -> AccessNetworkRuntimeStatus {
        let state = self
            .inner
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let age = state.snapshot.as_ref().map(ServingAccessSnapshot::age);
        AccessNetworkRuntimeStatus {
            available: state.snapshot.is_some() && age.is_some_and(|age| age <= max_age),
            stale: age.is_some_and(|age| age > max_age),
            technology: state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.technology.clone()),
            serving_plmn: state
                .snapshot
                .as_ref()
                .map(|snapshot| snapshot.serving_plmn.clone()),
            age_seconds: age.map(|age| age.as_secs()),
            last_error: state.last_error.clone(),
        }
    }
}

/// Reject control-character/header injection and return a trimmed value.
pub fn sanitize_header_value(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty()
        || value
            .chars()
            .any(|ch| ch == '\r' || ch == '\n' || ch == '\0' || ch.is_control())
    {
        return None;
    }
    Some(value.to_string())
}

/// Extract the access-type token from a PANI-style profile value for Contact
/// `+g.3gpp.accesstype`. Parameters belong in PANI/CNI, not in that feature tag.
pub fn access_type_token(value: &str) -> Option<String> {
    let value = sanitize_header_value(value)?;
    let token = value.split(';').next()?.trim();
    if token.is_empty()
        || !token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '.' | '_'))
    {
        return None;
    }
    Some(token.to_string())
}

fn serving_plmn(mcc: &str, mnc: &str) -> Option<String> {
    let mcc = mcc.trim();
    let mnc = mnc.trim();
    if mcc.len() != 3
        || !(mnc.len() == 2 || mnc.len() == 3)
        || !mcc.bytes().all(|byte| byte.is_ascii_digit())
        || !mnc.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Some(format!("{mcc}{mnc}"))
}

fn valid_plmn(plmn: &str) -> bool {
    matches!(plmn.len(), 5 | 6) && plmn.bytes().all(|byte| byte.is_ascii_digit())
}

fn infer_lte_access_type(
    profile_access_info: &str,
    serving_band: Option<&str>,
) -> Option<ImsAccessType> {
    if let Some(band) = serving_band.and_then(|value| parse_band_number(value, &["LTE B", "B"])) {
        if (33..=53).contains(&band) || band == 103 {
            return Some(ImsAccessType::EutranTdd);
        }
        if matches!(band, 1..=32 | 65 | 66 | 68 | 70..=74 | 85 | 87 | 88) {
            return Some(ImsAccessType::EutranFdd);
        }
    }
    infer_access_type_from_profile(
        profile_access_info,
        "3GPP-E-UTRAN-FDD",
        "3GPP-E-UTRAN-TDD",
        ImsAccessType::EutranFdd,
        ImsAccessType::EutranTdd,
    )
}

fn infer_nr_access_type(
    profile_access_info: &str,
    serving_band: Option<&str>,
) -> Option<ImsAccessType> {
    if let Some(band) =
        serving_band.and_then(|value| parse_band_number(value, &["NR N", "NR BAND ", "N"]))
    {
        if matches!(band, 34 | 38..=41 | 46..=48 | 53 | 77..=79 | 90 | 96 | 102 | 104) {
            return Some(ImsAccessType::NrTdd);
        }
        if matches!(
            band,
            1..=3 | 5 | 7 | 8 | 12..=14 | 18 | 20 | 25 | 26 | 28..=30 | 65 | 66 | 70 | 71 | 74..=76 | 85 | 91..=94
        ) {
            return Some(ImsAccessType::NrFdd);
        }
    }
    infer_access_type_from_profile(
        profile_access_info,
        "3GPP-NR-FDD",
        "3GPP-NR-TDD",
        ImsAccessType::NrFdd,
        ImsAccessType::NrTdd,
    )
}

fn infer_access_type_from_profile(
    profile_access_info: &str,
    fdd_token: &str,
    tdd_token: &str,
    fdd: ImsAccessType,
    tdd: ImsAccessType,
) -> Option<ImsAccessType> {
    let access_type = access_type_token(profile_access_info)?;
    if access_type.eq_ignore_ascii_case(tdd_token) {
        Some(tdd)
    } else if access_type.eq_ignore_ascii_case(fdd_token) {
        Some(fdd)
    } else {
        None
    }
}

fn parse_band_number(value: &str, prefixes: &[&str]) -> Option<u32> {
    let upper = value.trim().to_ascii_uppercase();
    let rest = prefixes
        .iter()
        .find_map(|prefix| upper.strip_prefix(prefix))?;
    rest.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lte_context_formats_real_tac_and_eci_at_fixed_widths() {
        let context = ImsAccessNetworkContext::new(
            ImsAccessType::EutranFdd,
            "50212",
            0x1234567,
            0x00ab,
            Some(0),
            AccessNetworkSource::TestFixture,
        )
        .expect("valid LTE context");

        assert_eq!(
            context.cellular_access_info(),
            "3GPP-E-UTRAN-FDD;utran-cell-id-3gpp=5021200AB1234567;cell-info-age=0"
        );
    }

    #[test]
    fn modem_snapshot_uses_serving_band_before_profile_hint() {
        let context = ImsAccessNetworkContext::from_modem_snapshot(
            "IEEE-802.11",
            Some("460"),
            Some("01"),
            "lte",
            0x12345,
            0x12,
            Some("LTE B41"),
        )
        .expect("LTE TDD context");

        assert_eq!(context.access_type, ImsAccessType::EutranTdd);
        assert!(context
            .cellular_access_info()
            .starts_with("3GPP-E-UTRAN-TDD;"));
    }

    #[test]
    fn nr_snapshot_preserves_complete_36_bit_nci() {
        let context = ImsAccessNetworkContext::from_modem_snapshot(
            "3GPP-NR-FDD",
            Some("310"),
            Some("260"),
            "nr",
            0x0f_1234_5678,
            0x12_3456,
            Some("NR n78"),
        )
        .expect("NR TDD context");

        assert_eq!(context.access_type, ImsAccessType::NrTdd);
        assert_eq!(
            context.cellular_access_info(),
            "3GPP-NR-TDD;utran-cell-id-3gpp=310260123456F12345678;cell-info-age=0"
        );
    }

    #[test]
    fn per_line_runtime_does_not_share_snapshots() {
        let first = ImsAccessNetworkRuntime::default();
        let second = ImsAccessNetworkRuntime::default();
        first.publish(
            ServingAccessSnapshot::new(
                "502",
                "12",
                "lte",
                0x12345,
                0x12,
                Some("LTE B3".to_string()),
                AccessNetworkSource::TestFixture,
            )
            .unwrap(),
        );

        assert!(first.context("3GPP-E-UTRAN-FDD").is_some());
        assert!(second.context("3GPP-E-UTRAN-FDD").is_none());
        assert_ne!(first, second);
        assert_eq!(first, first.clone());
    }

    #[test]
    fn epdg_location_exposes_only_fresh_per_line_serving_facts() {
        let runtime = ImsAccessNetworkRuntime::default();
        runtime.publish(
            ServingAccessSnapshot::new(
                "502",
                "12",
                "lte",
                0x12345,
                0x0b21,
                Some("LTE B3".to_string()),
                AccessNetworkSource::TestFixture,
            )
            .unwrap(),
        );

        assert_eq!(
            runtime.epdg_location(),
            Some(EpdgLocationSnapshot {
                serving_plmn: "50212".to_string(),
                technology: "lte".to_string(),
                tac: 0x0b21,
            })
        );
        assert!(runtime.epdg_location_with_max_age(Duration::ZERO).is_none());
    }

    #[test]
    fn runtime_clear_removes_identity_and_reports_reason() {
        let runtime = ImsAccessNetworkRuntime::default();
        runtime.publish(
            ServingAccessSnapshot::new(
                "460",
                "01",
                "lte",
                1,
                1,
                Some("LTE B41".to_string()),
                AccessNetworkSource::TestFixture,
            )
            .unwrap(),
        );
        runtime.clear("line_absent");

        assert!(runtime.context("3GPP-E-UTRAN-TDD").is_none());
        assert_eq!(
            runtime
                .status(DEFAULT_IMS_ACCESS_NETWORK_MAX_AGE)
                .last_error
                .as_deref(),
            Some("line_absent")
        );
    }

    #[test]
    fn invalid_or_missing_identity_is_omitted() {
        for (plmn, cell_id, tac) in [
            ("50\r\n212", 1, 1),
            ("5021", 1, 1),
            ("50212", 0, 1),
            ("50212", 1, 0),
        ] {
            assert!(ImsAccessNetworkContext::new(
                ImsAccessType::EutranFdd,
                plmn,
                cell_id,
                tac,
                Some(0),
                AccessNetworkSource::TestFixture,
            )
            .is_none());
        }
    }

    #[test]
    fn access_identity_policy_distinguishes_dynamic_static_and_missing_required() {
        let context = ImsAccessNetworkContext::new(
            ImsAccessType::EutranFdd,
            "50212",
            0x1234567,
            0x00ab,
            Some(0),
            AccessNetworkSource::TestFixture,
        )
        .expect("valid LTE context");

        let dynamic = resolve_access_identity(
            AccessIdentityPolicy::DynamicIfKnown,
            Some("3GPP-E-UTRAN-FDD"),
            Some(&context),
        );
        assert_eq!(dynamic.source, AccessIdentitySource::Dynamic);
        assert!(dynamic.value.unwrap().contains("utran-cell-id-3gpp="));

        let fallback = resolve_access_identity(
            AccessIdentityPolicy::DynamicIfKnown,
            Some("3GPP-E-UTRAN-FDD"),
            None,
        );
        assert_eq!(fallback.source, AccessIdentitySource::CompatibilityFallback);
        assert_eq!(fallback.value.as_deref(), Some("3GPP-E-UTRAN-FDD"));

        let required = resolve_access_identity(
            AccessIdentityPolicy::RequiredDynamic,
            Some("3GPP-E-UTRAN-FDD"),
            None,
        );
        assert!(required.required_dynamic_missing());
        assert_eq!(required.value, None);

        let omitted = resolve_access_identity(
            AccessIdentityPolicy::Omit,
            Some("3GPP-E-UTRAN-FDD"),
            Some(&context),
        );
        assert_eq!(omitted.source, AccessIdentitySource::Omitted);
        assert_eq!(omitted.value, None);
    }

    #[test]
    fn header_value_and_contact_token_reject_injection() {
        assert_eq!(
            access_type_token("IEEE-802.11;i-wlan-node-id=example"),
            Some("IEEE-802.11".to_string())
        );
        assert_eq!(sanitize_header_value("IEEE-802.11\r\nRoute: evil"), None);
        assert_eq!(access_type_token("IEEE-802.11\nvideo"), None);
    }
}
