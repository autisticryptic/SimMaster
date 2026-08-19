//! Read-only adapter for the schema-v7 `carrier_Bundles` SQLite catalog.
//!
//! v7 stores one compiled static configuration in
//! `carrier_profiles.config_json`. SimAdmin accepts only that contract: older
//! normalized catalogs must be rebuilt by `carrier_Bundles` before use.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use super::profile_record::CarrierProfileRecord;

#[path = "carrier_catalog_v7.rs"]
mod v7;

const CARRIER_BUNDLES_APPLICATION_ID: i64 = 1_128_419_922;
const SUPPORTED_SCHEMA_VERSION: i64 = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogAccessKind {
    LteEpc,
    WifiEpdg,
}

impl CatalogAccessKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::LteEpc => "lte_epc",
            Self::WifiEpdg => "wifi_epdg",
        }
    }

    const fn v7_status_column(self) -> &'static str {
        match self {
            Self::LteEpc => "lte_ims_status",
            Self::WifiEpdg => "vowifi_status",
        }
    }

    const fn v7_config_key(self) -> &'static str {
        match self {
            Self::LteEpc => "lte",
            Self::WifiEpdg => "vowifi",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CatalogRelease {
    pub release_id: String,
    pub generated_at: String,
    pub sealed: bool,
}

#[derive(Debug, Clone)]
pub struct CatalogProfile {
    pub record: CarrierProfileRecord,
    pub release: CatalogRelease,
}

#[derive(Debug, Clone)]
pub struct CatalogIdentityMatch {
    pub profile: CatalogProfile,
    pub match_prefix: String,
}

#[derive(Debug, Clone, Default)]
pub struct CatalogServiceCapabilities {
    pub volte_ready: bool,
    pub vowifi_ready: bool,
    pub vilte_enabled: bool,
    pub smsoip_enabled: bool,
    pub ut_xcap_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct CatalogProfileIcon {
    pub data: Vec<u8>,
    pub media_type: String,
}

/// The handle stores only a path. Every operation opens and validates the
/// immutable catalog again so an atomically replaced release is visible without
/// restarting SimAdmin.
#[derive(Debug, Clone)]
pub struct CarrierCatalog {
    path: PathBuf,
}

impl CarrierCatalog {
    /// Create a catalog handle without requiring the file to exist yet.
    ///
    /// The web installer uses this during first-run setup. Every actual query
    /// still opens and validates the file, so an absent or invalid catalog can
    /// never be consumed by the IMS runtime.
    pub fn at_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let catalog = Self::at_path(path);
        catalog.validated_connection()?;
        Ok(catalog)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn release(&self) -> Result<CatalogRelease, String> {
        v7::read_release(&self.validated_connection()?)
    }

    pub fn list(&self, access: CatalogAccessKind) -> Result<Vec<CatalogProfile>, String> {
        v7::list(&self.validated_connection()?, access)
    }

    pub fn service_capabilities(
        &self,
    ) -> Result<std::collections::HashMap<String, CatalogServiceCapabilities>, String> {
        v7::service_capabilities(&self.validated_connection()?)
    }

    pub fn profile_icon(&self, profile_id: &str) -> Result<Option<CatalogProfileIcon>, String> {
        v7::profile_icon(&self.validated_connection()?, profile_id)
    }

    /// Return profiles whose rules can be evaluated from PLMN/IMSI alone.
    /// GID, SPN and ICCID constrained rules are not widened into PLMN matches.
    pub fn public_identity_matches(
        &self,
        access: CatalogAccessKind,
    ) -> Result<Vec<CatalogIdentityMatch>, String> {
        v7::public_identity_matches(&self.validated_connection()?, access)
    }

    pub fn unique_for_plmn(
        &self,
        plmn: &str,
        access: CatalogAccessKind,
    ) -> Result<Option<CatalogProfile>, String> {
        let mut matches = self
            .public_identity_matches(access)?
            .into_iter()
            .filter(|matched| matched.profile.record.meta.plmn == plmn)
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            left.profile
                .record
                .meta
                .profile_id
                .cmp(&right.profile.record.meta.profile_id)
        });
        matches.dedup_by(|left, right| {
            left.profile.record.meta.profile_id == right.profile.record.meta.profile_id
        });
        Ok((matches.len() == 1).then(|| matches.remove(0).profile))
    }

    pub fn get(
        &self,
        profile_id: &str,
        access: CatalogAccessKind,
    ) -> Result<Option<CatalogProfile>, String> {
        let profile_id = profile_id.trim();
        if profile_id.is_empty() {
            return Ok(None);
        }
        v7::get(&self.validated_connection()?, profile_id, access)
    }

    pub fn imsi_has_ambiguous_plmn(&self, imsi: &str) -> Result<bool, String> {
        let imsi = imsi.trim();
        if imsi.len() < 6 || !imsi.bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok(false);
        }
        v7::imsi_has_ambiguous_plmn(&self.validated_connection()?, imsi)
    }

    pub fn ambiguous_plmn_prefixes(&self) -> Result<Vec<String>, String> {
        v7::ambiguous_plmn_prefixes(&self.validated_connection()?)
    }

    /// Infer the SIM home PLMN from unconstrained catalog identity rules,
    /// independently of whether the requested IMS access is currently ready.
    /// This preserves the authoritative 2/3-digit MNC boundary when a known
    /// carrier must use the explicitly labelled standard-derived fallback.
    pub fn infer_home_plmn(&self, imsi: &str) -> Result<Option<String>, String> {
        let imsi = imsi.trim();
        if imsi.len() < 5 || !imsi.bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok(None);
        }
        v7::infer_home_plmn(&self.validated_connection()?, imsi)
    }

    pub fn resolve_for_imsi(
        &self,
        imsi: &str,
        home_plmn: Option<&str>,
        access: CatalogAccessKind,
    ) -> Result<Option<CatalogProfile>, String> {
        let imsi = imsi.trim();
        if imsi.len() < 5 || !imsi.bytes().all(|byte| byte.is_ascii_digit()) {
            return Ok(None);
        }
        v7::resolve_for_imsi(&self.validated_connection()?, imsi, home_plmn, access)
    }

    fn validated_connection(&self) -> Result<Connection, String> {
        let conn = self.open_connection()?;
        validate_schema(&conn)?;
        Ok(conn)
    }

    fn open_connection(&self) -> Result<Connection, String> {
        let uri = immutable_sqlite_uri(&self.path);
        let conn = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|error| {
            format!(
                "carrier_catalog_open_failed:{}:{error}",
                self.path.display()
            )
        })?;
        conn.pragma_update(None, "query_only", true)
            .map_err(db_error)?;
        Ok(conn)
    }
}

fn immutable_sqlite_uri(path: &Path) -> String {
    let text = path.to_string_lossy();
    let mut encoded = String::with_capacity(text.len() + 25);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'\\' | b':' | b'.' | b'_' | b'-')
        {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    format!("file:{encoded}?mode=ro&immutable=1")
}

fn validate_schema(conn: &Connection) -> Result<(), String> {
    let application_id = conn
        .query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
        .map_err(db_error)?;
    if application_id != CARRIER_BUNDLES_APPLICATION_ID {
        return Err(format!(
            "carrier_catalog_application_id_mismatch:{application_id}"
        ));
    }
    let version = conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .map_err(db_error)?;
    if version != SUPPORTED_SCHEMA_VERSION {
        return Err(format!(
            "carrier_catalog_schema_unsupported:{version}:expected:{SUPPORTED_SCHEMA_VERSION}"
        ));
    }
    v7::validate_schema(conn)
}

fn normalize_home_plmn<'a>(imsi: &str, home_plmn: Option<&'a str>) -> Option<&'a str> {
    home_plmn.map(str::trim).filter(|plmn| {
        matches!(plmn.len(), 5 | 6)
            && plmn.bytes().all(|byte| byte.is_ascii_digit())
            && imsi.starts_with(*plmn)
    })
}

#[derive(Debug)]
struct ProfileMetaRow {
    profile_name: String,
    brand: String,
    legal_name: String,
    country_iso2: String,
    plmn: String,
    mcc: String,
    mnc: String,
    mnc_length: u8,
}

fn normalize_ip_family(value: &str) -> String {
    match value {
        "ipv4" => "ipv4",
        "ipv6" => "ipv6",
        "ipv4v6" | "ipv4_or_ipv6" => "ipv4v6",
        _ => "ipv4v6",
    }
    .to_string()
}

fn default_access_network_info(access: CatalogAccessKind) -> &'static str {
    match access {
        CatalogAccessKind::LteEpc => "3GPP-E-UTRAN-FDD",
        CatalogAccessKind::WifiEpdg => super::profiles::DEFAULT_ACCESS_NETWORK_INFO,
    }
}

fn expand_static_template(
    template: &str,
    meta: &ProfileMetaRow,
    field: &str,
) -> Result<String, String> {
    let expanded = template
        .replace("{plmn}", &meta.plmn)
        .replace("{mcc}", &meta.mcc)
        .replace("{mnc}", &meta.mnc)
        .replace("{mnc3}", &format!("{:0>3}", meta.mnc));
    reject_unexpanded_template(&expanded, field)?;
    Ok(expanded)
}

fn expand_ims_static_template(
    template: &str,
    meta: &ProfileMetaRow,
    home_domain: &str,
    field: &str,
) -> Result<String, String> {
    expand_static_template(&template.replace("{home_domain}", home_domain), meta, field)
}

fn reject_unexpanded_template(value: &str, field: &str) -> Result<(), String> {
    if value.contains('{') || value.contains('}') {
        return Err(format!(
            "carrier_catalog_runtime_template_unsupported:{field}"
        ));
    }
    Ok(())
}

fn db_error(error: rusqlite::Error) -> String {
    format!("carrier_catalog_query_failed:{error}")
}

#[cfg(test)]
pub(crate) fn test_catalog_fixture() -> (CarrierCatalog, PathBuf) {
    v7::test_catalog_fixture()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_legacy_catalog_before_any_query_runs() {
        let path = std::env::temp_dir().join(format!(
            "simadmin-legacy-catalog-{}-{}.sqlite3",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        {
            let conn = Connection::open(&path).expect("create legacy fixture");
            conn.pragma_update(None, "application_id", CARRIER_BUNDLES_APPLICATION_ID)
                .expect("set application id");
            conn.pragma_update(None, "user_version", 6)
                .expect("set legacy version");
        }
        let error = CarrierCatalog::open(&path).expect_err("v6 must be rejected");
        assert_eq!(error, "carrier_catalog_schema_unsupported:6:expected:7");
        std::fs::remove_file(path).expect("remove fixture");
    }
}
