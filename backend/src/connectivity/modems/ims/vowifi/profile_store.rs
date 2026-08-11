//! VoWiFi carrier profile store.
//!
//! Carrier defaults come from the normalized, read-only `carrier_Bundles`
//! catalog. The SQLite-backed operator override table has been removed: profile
//! persistence is being redesigned and will be reintroduced jointly with the
//! other subsystems that store carrier facts. Until then this type is a
//! read-only facade over the catalog and keeps the live VoWiFi matcher fed via
//! [`ProfileStore::publish`].
//!
//! There is deliberately no Rust built-in or code-derived fallback. Missing
//! carrier data is reported before network registration starts.

use std::{collections::BTreeMap, sync::Arc};

use super::carrier_catalog::{CarrierCatalog, CatalogAccessKind};
use super::profile_record::CarrierProfileRecord;
use super::profiles::{self, CarrierProfile};

/// Where a resolved profile came from. Surfaced to the UI so an operator can
/// tell a verified profile from a guessed one. Today every resolution comes
/// from the carrier catalog; a database origin returns with the redesigned
/// profile storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    Catalog,
}

impl ProfileOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
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
    catalog: Arc<CarrierCatalog>,
}

impl ProfileStore {
    pub fn with_catalog(catalog: Arc<CarrierCatalog>) -> Self {
        Self { catalog }
    }

    /// List the profiles the catalog makes available for VoWiFi.
    pub fn list(&self) -> Result<Vec<StoredProfile>, String> {
        let mut merged = BTreeMap::new();
        for profile in self.catalog.list(CatalogAccessKind::WifiEpdg)? {
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
        Ok(merged.into_values().collect())
    }

    /// Push the catalog rows into the resolver used by the live VoWiFi path.
    ///
    /// The catalog is read-only and immutable for a process, so this only needs
    /// to run once at startup.
    pub fn publish(&self) {
        let published = (|| -> Result<_, String> {
            let mut all_profiles = BTreeMap::new();
            let mut resolver_matches = Vec::new();
            for entry in self.catalog.list(CatalogAccessKind::WifiEpdg)? {
                entry.record.validate()?;
                let profile = entry.record.intern();
                all_profiles.insert(profile.meta.profile_id.to_string(), profile);
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
            Ok((
                all_profiles.values().copied().collect::<Vec<_>>(),
                resolver_matches,
            ))
        })();

        match published {
            Ok((all_profiles, resolver_matches)) => {
                profiles::publish_resolver_profiles(&all_profiles, &resolver_matches);
                match self.catalog.ambiguous_plmn_prefixes() {
                    Ok(prefixes) => profiles::publish_ambiguous_plmn_prefixes(&prefixes),
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

    /// Resolve the VoWiFi profile for a PLMN. The carrier catalog is the only
    /// source; no generated or compiled-in answer is returned.
    pub fn resolve_by_plmn(&self, mcc: &str, mnc: &str) -> Option<ResolvedProfile> {
        let plmn = format!("{mcc}{mnc}");
        self.catalog
            .unique_for_plmn(&plmn, CatalogAccessKind::WifiEpdg)
            .ok()?
            .map(|entry| ResolvedProfile {
                profile: entry.record.intern(),
                origin: ProfileOrigin::Catalog,
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
            let profile = self.catalog.get(profile_id, access)?.ok_or_else(|| {
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

        let digits = imsi.trim();
        let home_plmn = home_plmn.map(str::trim).filter(|plmn| {
            matches!(plmn.len(), 5 | 6)
                && plmn.bytes().all(|byte| byte.is_ascii_digit())
                && digits.starts_with(*plmn)
        });
        if home_plmn.is_none() && self.catalog.imsi_has_ambiguous_plmn(digits)? {
            return Ok(None);
        }
        let profile = self
            .catalog
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
    /// Where the row came from, such as a sealed catalog release.
    pub source: String,
    pub updated_at: String,
    pub record: CarrierProfileRecord,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn store_with_catalog() -> (ProfileStore, PathBuf) {
        let (catalog, path) = super::super::carrier_catalog::test_catalog_fixture();
        (ProfileStore::with_catalog(Arc::new(catalog)), path)
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
}
