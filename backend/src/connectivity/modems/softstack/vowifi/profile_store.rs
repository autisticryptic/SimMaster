//! Database-backed VoWiFi carrier profile store.
//!
//! Carrier defaults come from the normalized, read-only `carrier_Bundles`
//! catalog. SimAdmin's own SQLite table is retained only for operator-created
//! overrides. Resolution order for a given SIM is:
//!
//! 1. **Local database override** — explicit operator intent wins.
//! 2. **Carrier catalog** — firmware-derived access/IMS/SIP configuration.
//!
//! There is deliberately no Rust built-in or code-derived fallback. Missing
//! carrier data is reported before network registration starts.

use std::{collections::BTreeMap, sync::Arc};

use super::carrier_catalog::{CarrierCatalog, CatalogAccessKind};
use super::profile_record::CarrierProfileRecord;
use super::profiles::{self, CarrierProfile};
use crate::platform::db::Database;

/// Where a resolved profile came from. Surfaced to the UI so an operator can
/// tell a verified profile from a guessed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    Database,
    Catalog,
}

impl ProfileOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileOrigin::Database => "database",
            ProfileOrigin::Catalog => "carrier_catalog",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub profile: &'static CarrierProfile,
    pub origin: ProfileOrigin,
}

#[derive(Clone)]
pub struct ProfileStore {
    database: Arc<Database>,
    catalog: Option<Arc<CarrierCatalog>>,
}

impl ProfileStore {
    pub fn with_catalog(database: Arc<Database>, catalog: Arc<CarrierCatalog>) -> Self {
        Self {
            database,
            catalog: Some(catalog),
        }
    }

    /// One-time migration of the legacy `vowifi-profiles.conf` file.
    ///
    /// That file held user-created ePDG overrides, which the profile database
    /// now supersedes. A legacy entry can only be migrated when the carrier
    /// catalog already supplies a complete profile for its PLMN; partial facts
    /// are never expanded from guessed IMS/ePDG defaults.
    pub fn migrate_legacy_profiles_file(&self, path: &std::path::Path) -> Result<usize, String> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Ok(0);
        };
        let legacy = crate::platform::config::parse_external_vowifi_profiles(&content);
        let mut migrated = 0;
        for entry in legacy {
            let Some(base) = self.resolve_by_plmn(&entry.mcc, &entry.mnc) else {
                tracing::warn!(profile_id = %entry.profile_id, "Skipping legacy VoWiFi profile because the carrier catalog has no matching baseline");
                continue;
            };
            let base = CarrierProfileRecord::from_profile(base.profile);
            let Some(mut record) = super::profile_import::ImportedCarrierFacts {
                mcc: entry.mcc.clone(),
                mnc: entry.mnc.clone(),
                ims_apn: entry.apn.clone(),
                ..Default::default()
            }
            .to_record(&base) else {
                tracing::warn!(profile_id = %entry.profile_id, "Skipping legacy VoWiFi profile with an invalid PLMN");
                continue;
            };
            // Keep the operator's own identifier so an edit made before the
            // migration is still recognisable afterwards.
            record.meta.profile_id = entry.profile_id.clone();
            record.epdg.host = entry.epdg_host.clone();
            record.epdg.port = entry.epdg_port;
            if matches!(entry.ip_stack.as_str(), "ipv4" | "ipv6" | "ipv4v6") {
                record.epdg.ip_stack = entry.ip_stack.clone();
            }
            if let Some(dns) = entry.dns_server.clone().filter(|v| !v.trim().is_empty()) {
                record.epdg.dns_servers = vec![dns.clone()];
                record.epdg.dns_server = Some(dns);
            }
            record.meta.source_refs = vec!["migrated:vowifi-profiles.conf".to_string()];
            if let Err(error) = self.save(&record, "legacy_file") {
                tracing::warn!(profile_id = %entry.profile_id, error = %error, "Failed to migrate legacy VoWiFi profile");
                continue;
            }
            migrated += 1;
        }
        if migrated > 0 {
            let archived = path.with_extension("conf.migrated");
            if let Err(error) = std::fs::rename(path, &archived) {
                tracing::warn!(error = %error, "Migrated legacy VoWiFi profiles but could not archive the file");
            }
        }
        Ok(migrated)
    }

    /// Project the stored profiles down to the legacy "external profile" shape
    /// the older API and UI still speak.
    pub fn list_as_external(
        &self,
    ) -> Result<Vec<crate::platform::config::ExternalVowifiProfile>, String> {
        let mut out = self
            .list()?
            .into_iter()
            .map(|stored| crate::platform::config::ExternalVowifiProfile {
                profile_id: stored.record.meta.profile_id,
                mcc: stored.record.meta.mcc,
                mnc: stored.record.meta.mnc,
                epdg_host: stored.record.epdg.host,
                epdg_port: stored.record.epdg.port,
                ip_stack: stored.record.epdg.ip_stack,
                apn: stored.record.epdg.apn,
                dns_server: stored
                    .record
                    .epdg
                    .dns_servers
                    .first()
                    .cloned()
                    .or(stored.record.epdg.dns_server),
            })
            .collect::<Vec<_>>();
        out.sort_by(|left, right| left.profile_id.cmp(&right.profile_id));
        Ok(out)
    }

    /// Apply a legacy-shaped ePDG override, expanding it into a full profile.
    /// An existing row for that id is edited in place so the REGISTER policy the
    /// operator already tuned is preserved.
    pub fn save_external(
        &self,
        entry: &crate::platform::config::ExternalVowifiProfile,
    ) -> Result<(), String> {
        let mut record = match self.get(&entry.profile_id)? {
            Some(existing) => existing,
            None => return Err("full_carrier_profile_required".to_string()),
        };
        record.meta.profile_id = entry.profile_id.clone();
        record.epdg.host = entry.epdg_host.clone();
        record.epdg.port = entry.epdg_port;
        if matches!(entry.ip_stack.as_str(), "ipv4" | "ipv6" | "ipv4v6") {
            record.epdg.ip_stack = entry.ip_stack.clone();
        }
        if let Some(apn) = entry.apn.clone().filter(|v| !v.trim().is_empty()) {
            record.epdg.apn = Some(apn);
        }
        match entry.dns_server.clone().filter(|v| !v.trim().is_empty()) {
            Some(dns) => {
                record.epdg.dns_servers = vec![dns.clone()];
                record.epdg.dns_server = Some(dns);
            }
            None => {
                record.epdg.dns_servers.clear();
                record.epdg.dns_server = None;
            }
        }
        self.save(&record, "manual")
    }

    pub fn list(&self) -> Result<Vec<StoredProfile>, String> {
        let mut merged = BTreeMap::new();
        if let Some(catalog) = &self.catalog {
            for profile in catalog.list(CatalogAccessKind::WifiEpdg)? {
                let profile_id = profile.record.meta.profile_id.clone();
                merged.insert(
                    profile_id.clone(),
                    StoredProfile {
                        profile_id,
                        plmn: profile.record.meta.plmn.clone(),
                        source: format!("carrier_catalog:{}", profile.release.release_id),
                        updated_at: profile.release.generated_at,
                        record: profile.record,
                    },
                );
            }
        }
        for profile in self.local_list()? {
            merged.insert(profile.profile_id.clone(), profile);
        }
        Ok(merged.into_values().collect())
    }

    fn local_list(&self) -> Result<Vec<StoredProfile>, String> {
        let rows = self
            .database
            .list_vowifi_carrier_profiles()
            .map_err(|error| error.to_string())?;
        let mut profiles = Vec::with_capacity(rows.len());
        for row in rows {
            match serde_json::from_str::<CarrierProfileRecord>(&row.payload_json) {
                Ok(record) => profiles.push(StoredProfile {
                    profile_id: row.profile_id,
                    plmn: row.plmn,
                    source: row.source,
                    updated_at: row.updated_at,
                    record,
                }),
                Err(error) => {
                    // A corrupt row must not take down the whole list; skip it
                    // and let the operator see the rest.
                    tracing::warn!(
                        profile_id = %row.profile_id,
                        error = %error,
                        "Skipping unreadable VoWiFi carrier profile row"
                    );
                }
            }
        }
        Ok(profiles)
    }

    pub fn get(&self, profile_id: &str) -> Result<Option<CarrierProfileRecord>, String> {
        if let Some(row) = self
            .database
            .get_vowifi_carrier_profile(profile_id)
            .map_err(|error| error.to_string())?
        {
            return serde_json::from_str(&row.payload_json)
                .map(Some)
                .map_err(|error| error.to_string());
        }
        self.catalog
            .as_ref()
            .map(|catalog| catalog.get(profile_id, CatalogAccessKind::WifiEpdg))
            .transpose()
            .map(|profile| profile.flatten().map(|profile| profile.record))
    }

    /// Insert or replace a profile. The record is validated first so a bad edit
    /// is rejected at the API boundary rather than surfacing as a failed IKE
    /// exchange much later.
    pub fn save(&self, record: &CarrierProfileRecord, source: &str) -> Result<(), String> {
        record.validate()?;
        let json = serde_json::to_string(record).map_err(|error| error.to_string())?;
        self.database
            .upsert_vowifi_carrier_profile(
                &record.meta.profile_id,
                &record.meta.plmn,
                source,
                &json,
            )
            .map_err(|error| error.to_string())?;
        self.publish();
        Ok(())
    }

    pub fn delete(&self, profile_id: &str) -> Result<bool, String> {
        let deleted = self
            .database
            .delete_vowifi_carrier_profile(profile_id)
            .map_err(|error| error.to_string())?;
        self.publish();
        Ok(deleted)
    }

    /// Push the current rows into the resolver used by the live VoWiFi path.
    ///
    /// Without this an edit would only change what the API reports; matching at
    /// connect time goes through the pure `profiles::resolve_*` functions, which
    /// have no database handle of their own.
    pub fn publish(&self) {
        let published = (|| -> Result<_, String> {
            let mut catalog_profiles = BTreeMap::new();
            let mut catalog_matches = Vec::new();
            if let Some(catalog) = &self.catalog {
                for entry in catalog.list(CatalogAccessKind::WifiEpdg)? {
                    entry.record.validate()?;
                    let profile = entry.record.intern();
                    catalog_profiles.insert(profile.meta.profile_id.to_string(), profile);
                }
                for matched in catalog.public_identity_matches(CatalogAccessKind::WifiEpdg)? {
                    let profile_id = matched.profile.record.meta.profile_id.clone();
                    let profile = catalog_profiles
                        .get(&profile_id)
                        .copied()
                        .unwrap_or_else(|| matched.profile.record.intern());
                    catalog_matches.push((matched.match_prefix, profile));
                }
            }

            let mut local_profiles = Vec::new();
            for entry in self.local_list()? {
                match entry.record.validate() {
                    Ok(()) => local_profiles.push(entry.record.intern()),
                    Err(error) => tracing::warn!(
                        profile_id = %entry.profile_id,
                        error = %error,
                        "Skipping invalid local VoWiFi profile during resolver publication"
                    ),
                }
            }
            let local_ids = local_profiles
                .iter()
                .map(|profile| profile.meta.profile_id)
                .collect::<std::collections::HashSet<_>>();
            let local_plmns = local_profiles
                .iter()
                .map(|profile| profile.meta.plmn)
                .collect::<std::collections::HashSet<_>>();

            let mut all_profiles = catalog_profiles;
            for profile in &local_profiles {
                all_profiles.insert(profile.meta.profile_id.to_string(), *profile);
            }
            let mut resolver_matches = local_profiles
                .iter()
                .map(|profile| (profile.meta.plmn.to_string(), *profile))
                .collect::<Vec<_>>();
            resolver_matches.extend(catalog_matches.into_iter().filter(|(_, profile)| {
                !local_ids.contains(profile.meta.profile_id)
                    && !local_plmns.contains(profile.meta.plmn)
            }));
            Ok((
                all_profiles.values().copied().collect::<Vec<_>>(),
                resolver_matches,
            ))
        })();

        match published {
            Ok((all_profiles, resolver_matches)) => {
                profiles::publish_resolver_profiles(&all_profiles, &resolver_matches);
                match self
                    .catalog
                    .as_ref()
                    .map(|catalog| catalog.ambiguous_plmn_prefixes())
                    .transpose()
                {
                    Ok(prefixes) => {
                        profiles::publish_ambiguous_plmn_prefixes(&prefixes.unwrap_or_default())
                    }
                    Err(error) => {
                        profiles::publish_ambiguous_plmn_prefixes(&[]);
                        tracing::warn!(
                            error = %error,
                            "Failed to publish ambiguous carrier PLMN prefixes"
                        );
                    }
                }
            }
            Err(error) => {
                tracing::warn!(error = %error, "Failed to publish VoWiFi carrier profiles to the resolver");
            }
        }
    }

    /// Resolve the VoWiFi profile for a PLMN. Both possible sources are
    /// databases; no generated or compiled-in answer is returned.
    pub fn resolve_by_plmn(&self, mcc: &str, mnc: &str) -> Option<ResolvedProfile> {
        let plmn = format!("{mcc}{mnc}");
        if let Ok(Some(row)) = self.database.get_vowifi_carrier_profile_by_plmn(&plmn) {
            if let Ok(record) = serde_json::from_str::<CarrierProfileRecord>(&row.payload_json) {
                if record.validate().is_ok() {
                    return Some(ResolvedProfile {
                        profile: record.intern(),
                        origin: ProfileOrigin::Database,
                    });
                }
            }
        }
        self.catalog
            .as_ref()?
            .unique_for_plmn(&plmn, CatalogAccessKind::WifiEpdg)
            .ok()?
            .map(|entry| ResolvedProfile {
                profile: entry.record.intern(),
                origin: ProfileOrigin::Catalog,
            })
    }

    /// Resolve a profile for one registration access. Local full-profile rows
    /// can override either leg; otherwise the catalog access row (`wifi_epdg`
    /// or `lte_epc`) is loaded for this attempt.
    pub fn resolve_for_imsi_access(
        &self,
        pinned_profile_id: Option<&str>,
        imsi: &str,
        home_plmn: Option<&str>,
        access: CatalogAccessKind,
    ) -> Result<Option<ResolvedProfile>, String> {
        if let Some(profile_id) = pinned_profile_id.map(str::trim).filter(|id| !id.is_empty()) {
            if let Some(row) = self
                .database
                .get_vowifi_carrier_profile(profile_id)
                .map_err(|error| error.to_string())?
            {
                let record = serde_json::from_str::<CarrierProfileRecord>(&row.payload_json)
                    .map_err(|error| {
                        format!("local_carrier_profile_invalid:{profile_id}:{error}")
                    })?;
                match access {
                    CatalogAccessKind::WifiEpdg => record.validate()?,
                    CatalogAccessKind::LteEpc => record.validate_ims_only()?,
                }
                return Ok(Some(ResolvedProfile {
                    profile: record.intern(),
                    origin: ProfileOrigin::Database,
                }));
            }
            if let Some(catalog) = &self.catalog {
                let profile = catalog.get(profile_id, access)?.ok_or_else(|| {
                    format!(
                        "carrier_catalog_profile_not_found:{profile_id}:{}",
                        access.as_str()
                    )
                })?;
                return Ok(Some(ResolvedProfile {
                    profile: profile.record.intern(),
                    origin: ProfileOrigin::Catalog,
                }));
            }
            return Err(format!(
                "carrier_profile_not_found:{profile_id}:{}",
                access.as_str()
            ));
        }

        let digits = imsi.trim();
        let home_plmn = home_plmn.map(str::trim).filter(|plmn| {
            matches!(plmn.len(), 5 | 6)
                && plmn.bytes().all(|byte| byte.is_ascii_digit())
                && digits.starts_with(*plmn)
        });
        if home_plmn.is_none()
            && self
                .catalog
                .as_ref()
                .map(|catalog| catalog.imsi_has_ambiguous_plmn(digits))
                .transpose()?
                .unwrap_or(false)
        {
            return Ok(None);
        }
        let local = self.local_list()?.into_iter().filter(|entry| {
            home_plmn.map_or_else(
                || digits.starts_with(&entry.record.meta.plmn),
                |plmn| entry.record.meta.plmn == plmn,
            ) && match access {
                CatalogAccessKind::WifiEpdg => entry.record.validate().is_ok(),
                CatalogAccessKind::LteEpc => entry.record.validate_ims_only().is_ok(),
            }
        });
        if let Some(entry) = local.max_by_key(|entry| entry.record.meta.plmn.len()) {
            return Ok(Some(ResolvedProfile {
                profile: entry.record.intern(),
                origin: ProfileOrigin::Database,
            }));
        }
        let Some(catalog) = &self.catalog else {
            return Ok(None);
        };
        let profile = catalog
            .resolve_for_imsi(digits, home_plmn, access)?
            .map(|profile| profile.record.intern())
            .map(ResolvedProfile::from);
        Ok(profile)
    }
}

impl From<&'static CarrierProfile> for ResolvedProfile {
    fn from(profile: &'static CarrierProfile) -> Self {
        Self {
            profile,
            origin: ProfileOrigin::Catalog,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredProfile {
    pub profile_id: String,
    pub plmn: String,
    /// Where the row came from, such as a sealed catalog release or a local
    /// operator override source.
    pub source: String,
    pub updated_at: String,
    pub record: CarrierProfileRecord,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn store_with_catalog() -> (ProfileStore, PathBuf) {
        let database = Arc::new(Database::new(PathBuf::from(":memory:")).expect("db"));
        let (catalog, path) = super::super::carrier_catalog::test_catalog_fixture();
        (
            ProfileStore::with_catalog(database, Arc::new(catalog)),
            path,
        )
    }

    #[test]
    fn catalog_is_listed_and_unknown_carriers_are_not_derived() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();

        let listed = store.list().expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].profile_id, "test-v7-23433");
        assert_eq!(listed[1].profile_id, "test-v7-23434");
        assert!(listed[0].source.starts_with("carrier_catalog:"));
        assert!(store.resolve_by_plmn("460", "01").is_none());
        assert!(store
            .resolve_for_imsi_access(None, "460011234567890", None, CatalogAccessKind::LteEpc)
            .expect("unknown profile query")
            .is_none());
        store.publish();
        let published = profiles::resolve_for_line(None, "234330123456789", Some("23433"))
            .expect("published catalog match");
        assert_eq!(published.profile.meta.profile_id, "test-v7-23433");
        assert_eq!(published.matched_prefix, "234330");

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn local_override_wins_and_delete_restores_catalog() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        let mut record = store
            .get("test-v7-23433")
            .expect("read")
            .expect("catalog profile");
        record.epdg.host = "epdg.override.test".to_string();
        store.save(&record, "manual").expect("save override");

        let resolved = store
            .resolve_by_plmn("234", "33")
            .expect("resolve override");
        assert_eq!(resolved.origin, ProfileOrigin::Database);
        assert_eq!(resolved.profile.epdg.host, "epdg.override.test");

        assert!(store.delete("test-v7-23433").expect("delete override"));
        let restored = store.resolve_by_plmn("234", "33").expect("resolve catalog");
        assert_eq!(restored.origin, ProfileOrigin::Catalog);
        assert_eq!(restored.profile.epdg.host, "epdg.mnc033.mcc234.example");

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn access_specific_resolution_keeps_lte_and_wifi_apns_separate() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();

        let wifi = store
            .resolve_for_imsi_access(None, "234330123456789", None, CatalogAccessKind::WifiEpdg)
            .expect("wifi query")
            .expect("wifi profile");
        let lte = store
            .resolve_for_imsi_access(None, "234330123456789", None, CatalogAccessKind::LteEpc)
            .expect("lte query")
            .expect("lte profile");
        assert_eq!(wifi.profile.epdg.apn, Some("wifi-ims"));
        assert_eq!(lte.profile.epdg.apn, Some("lte-ims"));
        assert_eq!(lte.profile.ims.register.expires_seconds, 1800);

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn pinned_non_ready_catalog_profile_returns_its_configuration_error() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        {
            let conn = rusqlite::Connection::open(&path).expect("open catalog fixture");
            conn.execute(
                "UPDATE carrier_profiles SET lte_ims_status = 'partial'
                 WHERE profile_id = 'test-v7-23433'",
                [],
            )
            .expect("mark pinned profile partial");
        }

        let error = store
            .resolve_for_imsi_access(
                Some("test-v7-23433"),
                "234330123456789",
                Some("23433"),
                CatalogAccessKind::LteEpc,
            )
            .expect_err("pinned partial profile must not fall back to auto matching");
        assert_eq!(
            error,
            "carrier_catalog_profile_not_ready:test-v7-23433:lte_epc:partial"
        );

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn legacy_override_requires_and_preserves_a_catalog_baseline() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, catalog_path) = store_with_catalog();
        let path = std::env::temp_dir().join(format!(
            "simadmin-legacy-vowifi-{}-{}.conf",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::write(
            &path,
            r#"# SimAdmin custom VoWiFi/ePDG profiles
{
  "schema_version": 1,
  "profiles": [
    {
      "profile_id": "my_test_override",
      "mcc": "234",
      "mnc": "33",
      "epdg_host": "epdg.custom.test",
      "epdg_port": 4500,
      "ip_stack": "ipv4",
      "apn": "cmims",
      "dns_server": "8.8.8.8"
    }
  ]
}

"#,
        )
        .expect("write legacy file");

        assert_eq!(
            store.migrate_legacy_profiles_file(&path).expect("migrate"),
            1
        );

        // The override survived, and fields absent from the legacy file still
        // come from the catalog rather than from generated defaults.
        let resolved = store.resolve_by_plmn("234", "33").expect("resolve");
        assert_eq!(resolved.origin, ProfileOrigin::Database);
        assert_eq!(resolved.profile.epdg.host, "epdg.custom.test");
        assert_eq!(resolved.profile.epdg.port, 4500);
        assert_eq!(resolved.profile.epdg.ip_stack, "ipv4");
        assert_eq!(resolved.profile.epdg.apn, Some("cmims"));
        assert_eq!(resolved.profile.epdg.dns_servers, &["8.8.8.8"]);
        let published = profiles::resolve_for_line(None, "234330123456789", Some("23433"))
            .expect("published local override");
        assert_eq!(published.profile.meta.profile_id, "my_test_override");
        assert_eq!(
            resolved.profile.ims.domain, "ims.mnc033.mcc234.example",
            "fields the file never carried still come from the catalog"
        );

        // The file is archived, so a restart does not migrate it twice.
        assert!(!path.exists());
        let archived = path.with_extension("conf.migrated");
        assert!(archived.exists());
        assert_eq!(store.migrate_legacy_profiles_file(&path).expect("rerun"), 0);

        let _ = std::fs::remove_file(archived);
        std::fs::remove_file(catalog_path).expect("remove fixture");
    }
}
