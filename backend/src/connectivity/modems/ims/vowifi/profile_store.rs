//! VoWiFi carrier profile store.
//!
//! Carrier defaults come from the normalized, read-only `carrier_Bundles`
//! catalog. User-authored overrides live in SimAdmin's application database so
//! replacing the catalog never deletes local work and normal device backups
//! include it. This store merges both sources and keeps the live matcher fed via
//! [`ProfileStore::publish`].
//!
//! Automatic matching may use a clearly marked, conservative 3GPP-derived
//! fallback when neither source has a usable access profile. Explicit profile
//! pins remain strict and never silently fall back.

use std::{collections::BTreeMap, sync::Arc};

use super::carrier_catalog::{CarrierCatalog, CatalogAccessKind};
use super::profile_record::CarrierProfileRecord;
use super::profiles::{self, CarrierProfile};
use crate::platform::db::{CustomCarrierProfileEntry, Database};

/// Where a resolved profile came from. Surfaced to the UI so an operator can
/// tell a sealed catalog row from an operator-authored override.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    #[serde(rename = "carrier_catalog")]
    Catalog,
    Database,
    Derived,
}

impl ProfileOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileOrigin::Catalog => "carrier_catalog",
            ProfileOrigin::Database => "database",
            ProfileOrigin::Derived => "derived",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedProfile {
    pub profile: &'static CarrierProfile,
    pub origin: ProfileOrigin,
    pub fallback_reason: Option<String>,
}

#[derive(Clone)]
pub struct ProfileStore {
    catalog: Arc<CarrierCatalog>,
    database: Arc<Database>,
}

impl ProfileStore {
    pub fn new(catalog: Arc<CarrierCatalog>, database: Arc<Database>) -> Self {
        Self { catalog, database }
    }

    fn custom_records(
        &self,
    ) -> Result<Vec<(CustomCarrierProfileEntry, CarrierProfileRecord)>, String> {
        self.database
            .list_custom_carrier_profiles()
            .map_err(|error| format!("custom_carrier_profile_list_failed:{error}"))?
            .into_iter()
            .map(|entry| {
                let record = serde_json::from_str::<CarrierProfileRecord>(&entry.record_json)
                    .map_err(|error| {
                        format!(
                            "custom_carrier_profile_json_invalid:{}:{error}",
                            entry.profile_id
                        )
                    })?;
                record.validate().map_err(|error| {
                    format!(
                        "custom_carrier_profile_invalid:{}:{error}",
                        entry.profile_id
                    )
                })?;
                Ok((entry, record))
            })
            .collect()
    }

    /// List the profiles the catalog makes available for VoWiFi.
    pub fn list(&self) -> Result<Vec<StoredProfile>, String> {
        let mut merged = BTreeMap::new();
        match self.catalog.service_capabilities() {
            Ok(capabilities) => {
                // Prefer the WiFi projection because it contains the complete
                // ePDG/IKE policy required when a catalog row is copied into a
                // user-editable profile. LTE-only rows still enter on the
                // second pass.
                for access in [CatalogAccessKind::WifiEpdg, CatalogAccessKind::LteEpc] {
                    for profile in self.catalog.list(access)? {
                        let profile_id = profile.record.meta.profile_id.clone();
                        let capability = capabilities.get(&profile_id).cloned().unwrap_or_default();
                        merged
                            .entry(profile_id.clone())
                            .or_insert_with(|| StoredProfile {
                                profile_id,
                                plmn: profile.record.meta.plmn.clone(),
                                origin: ProfileOrigin::Catalog,
                                source: format!("carrier_catalog:{}", profile.release.release_id),
                                updated_at: profile.release.generated_at,
                                record: profile.record,
                                volte_ready: capability.volte_ready,
                                vowifi_ready: capability.vowifi_ready,
                                vilte_enabled: capability.vilte_enabled,
                                smsoip_enabled: capability.smsoip_enabled,
                                ut_xcap_enabled: capability.ut_xcap_enabled,
                            });
                    }
                }
            }
            Err(error) => {
                tracing::debug!(error = %error, "Carrier catalog unavailable while listing profiles")
            }
        }
        for (entry, record) in self.custom_records()? {
            merged.insert(
                entry.profile_id.clone(),
                StoredProfile {
                    profile_id: entry.profile_id,
                    plmn: entry.plmn,
                    origin: ProfileOrigin::Database,
                    source: "manual".to_string(),
                    updated_at: entry.updated_at,
                    volte_ready: true,
                    vowifi_ready: record.voice.vowifi_enabled,
                    vilte_enabled: false,
                    smsoip_enabled: true,
                    ut_xcap_enabled: record.ut.enabled,
                    record,
                },
            );
        }
        Ok(merged.into_values().collect())
    }

    pub fn upsert(&self, mut record: CarrierProfileRecord) -> Result<StoredProfile, String> {
        record.validate()?;
        // SimAdmin does not persist emergency-address configuration in carrier
        // profiles. Keep the runtime field inert when storing a custom row.
        record.e911.enabled = false;
        record.e911.provider = None;
        record.e911.entitlement_url = None;
        record.e911.websheet_host_policy = None;
        let record_json = serde_json::to_string(&record)
            .map_err(|error| format!("custom_carrier_profile_serialize_failed:{error}"))?;
        let entry = self
            .database
            .upsert_custom_carrier_profile(&record.meta.profile_id, &record.meta.plmn, &record_json)
            .map_err(|error| format!("custom_carrier_profile_save_failed:{error}"))?;
        self.publish();
        Ok(StoredProfile {
            profile_id: entry.profile_id,
            plmn: entry.plmn,
            origin: ProfileOrigin::Database,
            source: "manual".to_string(),
            updated_at: entry.updated_at,
            volte_ready: true,
            vowifi_ready: record.voice.vowifi_enabled,
            vilte_enabled: false,
            smsoip_enabled: true,
            ut_xcap_enabled: record.ut.enabled,
            record,
        })
    }

    pub fn delete(&self, profile_id: &str) -> Result<bool, String> {
        let deleted = self
            .database
            .delete_custom_carrier_profile(profile_id)
            .map_err(|error| format!("custom_carrier_profile_delete_failed:{error}"))?;
        if deleted {
            self.publish();
        }
        Ok(deleted)
    }

    /// Push the catalog rows into the resolver used by the live VoWiFi path.
    ///
    /// The catalog is read-only and immutable for a process, so this only needs
    /// to run once at startup.
    pub fn publish(&self) {
        let published = (|| -> Result<_, String> {
            let mut all_profiles = BTreeMap::new();
            let mut resolver_matches = Vec::new();
            let custom_records = self.custom_records()?;
            for (_, record) in &custom_records {
                let profile = record.intern();
                all_profiles.insert(record.meta.profile_id.clone(), profile);
                resolver_matches.push((record.meta.plmn.clone(), profile));
            }
            match self.catalog.list(CatalogAccessKind::WifiEpdg) {
                Ok(entries) => {
                    for entry in entries {
                        entry.record.validate()?;
                        let profile = entry.record.intern();
                        all_profiles
                            .entry(profile.meta.profile_id.to_string())
                            .or_insert(profile);
                    }
                    for matched in self
                        .catalog
                        .public_identity_matches(CatalogAccessKind::WifiEpdg)?
                    {
                        let profile_id = matched.profile.record.meta.profile_id.clone();
                        let profile = all_profiles
                            .get(&profile_id)
                            .copied()
                            .unwrap_or_else(|| matched.profile.record.intern());
                        resolver_matches.push((matched.match_prefix, profile));
                    }
                }
                Err(error) => {
                    tracing::debug!(error = %error, "Carrier catalog unavailable while publishing profiles")
                }
            }
            Ok((
                all_profiles.values().copied().collect::<Vec<_>>(),
                resolver_matches,
                custom_records
                    .into_iter()
                    .map(|(_, record)| record.meta.plmn)
                    .collect::<std::collections::HashSet<_>>(),
            ))
        })();

        match published {
            Ok((all_profiles, resolver_matches, custom_plmns)) => {
                profiles::publish_resolver_profiles(&all_profiles, &resolver_matches);
                match self.catalog.ambiguous_plmn_prefixes() {
                    Ok(prefixes) => profiles::publish_ambiguous_plmn_prefixes(
                        &prefixes
                            .into_iter()
                            .filter(|prefix| !custom_plmns.contains(prefix))
                            .collect::<Vec<_>>(),
                    ),
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

    /// Resolve the VoWiFi profile for a PLMN. Custom/catalog rows win; when no
    /// usable row exists, return the standard 3GPP-derived fallback so profile
    /// discovery and live registration have the same no-catalog behaviour.
    pub fn resolve_by_plmn(&self, mcc: &str, mnc: &str) -> Option<ResolvedProfile> {
        let plmn = format!("{mcc}{mnc}");
        if let Some((_, record)) = self
            .custom_records()
            .ok()?
            .into_iter()
            .find(|(_, record)| record.meta.plmn == plmn)
        {
            return Some(ResolvedProfile {
                profile: record.intern(),
                origin: ProfileOrigin::Database,
                fallback_reason: None,
            });
        }
        if let Ok(Some(entry)) = self
            .catalog
            .unique_for_plmn(&plmn, CatalogAccessKind::WifiEpdg)
        {
            return Some(ResolvedProfile {
                profile: entry.record.intern(),
                origin: ProfileOrigin::Catalog,
                fallback_reason: None,
            });
        }
        let profile = profiles::derive_standard_3gpp_profile(
            mcc,
            mnc,
            profiles::Standard3gppAccess::WifiEpdg,
        )?;
        Some(ResolvedProfile {
            profile,
            origin: ProfileOrigin::Derived,
            fallback_reason: Some(format!(
                "carrier_catalog_no_usable_profile:home_plmn:{plmn}:access:wifi_epdg"
            )),
        })
    }

    /// Resolve a profile for one registration access. A pinned profile id is
    /// looked up directly in the catalog; otherwise the catalog access row
    /// (`wifi_epdg` or `lte_epc`) is loaded for this attempt.
    pub fn resolve_for_imsi_access(
        &self,
        pinned_profile_id: Option<&str>,
        imsi: &str,
        home_plmn: Option<&str>,
        access: CatalogAccessKind,
    ) -> Result<Option<ResolvedProfile>, String> {
        if let Some(profile_id) = pinned_profile_id.map(str::trim).filter(|id| !id.is_empty()) {
            if let Some((_, record)) = self
                .custom_records()?
                .into_iter()
                .find(|(_, record)| record.meta.profile_id == profile_id)
            {
                return Ok(Some(ResolvedProfile {
                    profile: record.intern(),
                    origin: ProfileOrigin::Database,
                    fallback_reason: None,
                }));
            }
            let profile = self.catalog.get(profile_id, access)?.ok_or_else(|| {
                format!(
                    "carrier_catalog_profile_not_found:{profile_id}:{}",
                    access.as_str()
                )
            })?;
            return Ok(Some(ResolvedProfile {
                profile: profile.record.intern(),
                origin: ProfileOrigin::Catalog,
                fallback_reason: None,
            }));
        }

        let digits = imsi.trim();
        let explicit_home_plmn = home_plmn
            .map(str::trim)
            .filter(|plmn| {
                matches!(plmn.len(), 5 | 6)
                    && plmn.bytes().all(|byte| byte.is_ascii_digit())
                    && digits.starts_with(*plmn)
            })
            .map(str::to_string);
        let (custom_records, custom_lookup_error) = match self.custom_records() {
            Ok(records) => (records, None),
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    access = access.as_str(),
                    "Custom carrier profile lookup failed; continuing with catalog and standard fallback"
                );
                (Vec::new(), Some(error))
            }
        };
        let mut custom_matches = custom_records
            .into_iter()
            .filter(|(_, record)| {
                explicit_home_plmn.as_deref().map_or_else(
                    || digits.starts_with(&record.meta.plmn),
                    |plmn| record.meta.plmn == plmn,
                )
            })
            .collect::<Vec<_>>();
        custom_matches.sort_by(|left, right| {
            right
                .1
                .meta
                .plmn
                .len()
                .cmp(&left.1.meta.plmn.len())
                .then(left.1.meta.profile_id.cmp(&right.1.meta.profile_id))
        });
        if let Some((_, record)) = custom_matches.into_iter().next() {
            return Ok(Some(ResolvedProfile {
                profile: record.intern(),
                origin: ProfileOrigin::Database,
                fallback_reason: None,
            }));
        }
        let home_plmn = explicit_home_plmn.or_else(|| {
            self.catalog
                .infer_home_plmn(digits)
                .ok()
                .flatten()
                .or_else(|| inferred_home_plmn(digits).map(str::to_string))
        });
        let home_plmn = home_plmn.as_deref();
        let catalog_result = match self.catalog.imsi_has_ambiguous_plmn(digits) {
            Ok(true) if home_plmn.is_none() => Ok(None),
            Ok(_) => self.catalog.resolve_for_imsi(digits, home_plmn, access),
            Err(error) => Err(error),
        };
        let catalog_fallback_reason = match catalog_result {
            Ok(Some(profile)) => {
                return Ok(Some(ResolvedProfile::from(profile.record.intern())));
            }
            Ok(None) => {
                let identity_hint = home_plmn
                    .map(|plmn| format!("home_plmn:{plmn}"))
                    .unwrap_or_else(|| {
                        let prefix = digits.get(..digits.len().min(6)).unwrap_or("unknown");
                        format!("imsi_prefix:{prefix}")
                    });
                format!(
                    "carrier_catalog_no_usable_profile:{identity_hint}:access:{}",
                    access.as_str()
                )
            }
            Err(error) => error,
        };
        let fallback_reason = custom_lookup_error
            .map_or(catalog_fallback_reason.clone(), |error| {
                format!("custom_profile_lookup_failed:{error};{catalog_fallback_reason}")
            });

        Ok(derive_standard_fallback(
            digits,
            home_plmn,
            access,
            fallback_reason,
        ))
    }
}

fn derive_standard_fallback(
    imsi: &str,
    home_plmn: Option<&str>,
    access: CatalogAccessKind,
    fallback_reason: String,
) -> Option<ResolvedProfile> {
    let inferred_length = if imsi.starts_with("460") { 5 } else { 6 };
    let plmn = home_plmn
        .filter(|plmn| {
            matches!(plmn.len(), 5 | 6)
                && plmn.bytes().all(|byte| byte.is_ascii_digit())
                && imsi.starts_with(*plmn)
        })
        .or_else(|| imsi.get(..inferred_length))?;
    let standard_access = match access {
        CatalogAccessKind::LteEpc => profiles::Standard3gppAccess::LteEpc,
        CatalogAccessKind::WifiEpdg => profiles::Standard3gppAccess::WifiEpdg,
    };
    let profile = profiles::derive_standard_3gpp_profile(&plmn[..3], &plmn[3..], standard_access)?;
    Some(ResolvedProfile {
        profile,
        origin: ProfileOrigin::Derived,
        fallback_reason: Some(fallback_reason),
    })
}

fn inferred_home_plmn(imsi: &str) -> Option<&str> {
    let inferred_length = if imsi.starts_with("460") { 5 } else { 6 };
    imsi.get(..inferred_length)
}

impl From<&'static CarrierProfile> for ResolvedProfile {
    fn from(profile: &'static CarrierProfile) -> Self {
        Self {
            profile,
            origin: ProfileOrigin::Catalog,
            fallback_reason: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredProfile {
    pub profile_id: String,
    pub plmn: String,
    pub origin: ProfileOrigin,
    /// Where the row came from, such as a sealed catalog release.
    pub source: String,
    pub updated_at: String,
    pub record: CarrierProfileRecord,
    pub volte_ready: bool,
    pub vowifi_ready: bool,
    pub vilte_enabled: bool,
    pub smsoip_enabled: bool,
    pub ut_xcap_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn store_with_catalog() -> (ProfileStore, PathBuf) {
        let (catalog, path) = super::super::carrier_catalog::test_catalog_fixture();
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        (ProfileStore::new(Arc::new(catalog), database), path)
    }

    #[test]
    fn catalog_is_listed_and_missing_carriers_use_derived_fallback() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();

        let listed = store.list().expect("list");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].profile_id, "test-v7-23433");
        assert_eq!(listed[1].profile_id, "test-v7-23434");
        assert!(listed[0].source.starts_with("carrier_catalog:"));
        assert!(store.resolve_by_plmn("460", "01").is_none());
        let fallback = store
            .resolve_for_imsi_access(None, "460011234567890", None, CatalogAccessKind::LteEpc)
            .expect("unknown profile query")
            .expect("standard fallback");
        assert_eq!(fallback.origin, ProfileOrigin::Derived);
        assert_eq!(fallback.profile.meta.profile_id, "derived_3gpp_lte_46001");
        assert!(fallback
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("carrier_catalog_no_usable_profile")));
        store.publish();
        let published = profiles::resolve_for_line(None, "234330123456789", Some("23433"))
            .expect("published catalog match");
        assert_eq!(published.profile.meta.profile_id, "test-v7-23433");
        assert_eq!(published.matched_prefix, "234330");

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn absent_catalog_uses_derived_fallback_without_a_runtime_switch() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog = CarrierCatalog::at_path(PathBuf::from(
            "/definitely-missing/carrier-bundles.sqlite3",
        ));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database);

        let resolved = store
            .resolve_for_imsi_access(
                None,
                "502121234567890",
                Some("50212"),
                CatalogAccessKind::LteEpc,
            )
            .expect("missing catalog query")
            .expect("derived profile");

        assert_eq!(resolved.origin, ProfileOrigin::Derived);
        assert_eq!(resolved.profile.meta.profile_id, "derived_3gpp_lte_50212");
        assert!(resolved
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("carrier_catalog_open_failed")));
    }

    #[test]
    fn access_specific_resolution_keeps_lte_and_wifi_apns_separate() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();

        assert_eq!(
            store.catalog.infer_home_plmn("234330123456789").unwrap(),
            Some("23433".to_string())
        );
        let wifi = store
            .resolve_for_imsi_access(
                None,
                "234330123456789",
                Some("46000"),
                CatalogAccessKind::WifiEpdg,
            )
            .expect("wifi query")
            .expect("wifi profile");
        let lte = store
            .resolve_for_imsi_access(
                None,
                "234330123456789",
                Some("46000"),
                CatalogAccessKind::LteEpc,
            )
            .expect("lte query")
            .expect("lte profile");
        assert_eq!(wifi.profile.epdg.apn, Some("wifi-ims"));
        assert_eq!(lte.profile.epdg.apn, Some("lte-ims"));
        assert_eq!(lte.profile.ims.register.expires_seconds, 1800);
        assert_eq!(wifi.origin, ProfileOrigin::Catalog);
        assert_eq!(lte.origin, ProfileOrigin::Catalog);
        assert!(wifi.fallback_reason.is_none());
        assert!(lte.fallback_reason.is_none());

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn custom_profile_overrides_catalog_and_delete_restores_it() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        let mut record = store
            .list()
            .expect("catalog list")
            .into_iter()
            .find(|profile| profile.profile_id == "test-v7-23433")
            .expect("fixture profile")
            .record;
        record.meta.brand = "Custom Test Mobile".to_string();

        let saved = store.upsert(record).expect("save custom profile");
        assert_eq!(saved.origin, ProfileOrigin::Database);
        let resolved = store
            .resolve_by_plmn("234", "33")
            .expect("resolve custom profile");
        assert_eq!(resolved.origin, ProfileOrigin::Database);
        assert_eq!(resolved.profile.meta.brand, "Custom Test Mobile");
        assert_eq!(
            store
                .list()
                .expect("merged list")
                .into_iter()
                .find(|profile| profile.profile_id == "test-v7-23433")
                .expect("merged profile")
                .origin,
            ProfileOrigin::Database
        );

        assert!(store
            .delete("test-v7-23433")
            .expect("delete custom profile"));
        assert_eq!(
            store
                .resolve_by_plmn("234", "33")
                .expect("catalog restored")
                .origin,
            ProfileOrigin::Catalog
        );
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
    fn automatic_match_derives_when_known_lte_profile_is_not_ready() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        {
            let conn = rusqlite::Connection::open(&path).expect("open catalog fixture");
            conn.execute(
                "UPDATE carrier_profiles SET lte_ims_status = 'unknown'
                 WHERE profile_id = 'test-v7-23433'",
                [],
            )
            .expect("mark automatic profile unknown");
        }

        let resolved = store
            .resolve_for_imsi_access(
                None,
                "234330123456789",
                Some("46000"),
                CatalogAccessKind::LteEpc,
            )
            .expect("automatic match should derive")
            .expect("derived profile");
        assert_eq!(resolved.origin, ProfileOrigin::Derived);
        assert_eq!(resolved.profile.meta.profile_id, "derived_3gpp_lte_23433");
        assert_eq!(
            resolved.fallback_reason.as_deref(),
            Some("carrier_catalog_profile_not_ready:test-v7-23433:lte_epc:unknown")
        );
        assert_eq!(
            resolved.profile.ims.register.access_network_info,
            "3GPP-E-UTRAN-FDD"
        );
        assert!(!resolved.profile.ims.register.include_visited_network);

        std::fs::remove_file(path).expect("remove fixture");
    }
}
