//! VoWiFi carrier profile store.
//!
//! Carrier defaults come from the normalized, read-only `carrier_Bundles`
//! catalog. User-authored overrides live in SimAdmin's application database so
//! replacing the catalog never deletes local work and normal device backups
//! include it. This store merges both sources and keeps the live matcher fed via
//! [`ProfileStore::publish`].
//!
//! Automatic matching may use a clearly marked, conservative 3GPP-derived
//! fallback when neither source has a usable access profile. Legacy generic
//! profile-resolution APIs keep explicit pins strict; the per-line VoLTE
//! candidate API deliberately falls back inside the same logical slot when a
//! previously selected source row disappears or loses its LTE projection.

use std::{collections::BTreeMap, sync::Arc};

use super::carrier_catalog::{CarrierCatalog, CatalogAccessKind};
use super::profile_record::{CarrierProfileRecord, CURRENT_SCHEMA_VERSION};
use super::profiles::{self, CarrierProfile};
use crate::platform::{
    config::{VolteProfileCandidate, VolteProfileSource},
    db::{CustomCarrierProfileEntry, Database},
};

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

/// Validation state for one explicit, source-bound VoLTE profile reference.
/// Automatic candidates do not use this: they are allowed to resolve to the
/// derived fallback when their source is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolteProfileReferenceState {
    Ready,
    NotLteReady,
    Missing,
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

#[derive(Debug)]
struct InvalidCustomProfile {
    entry: CustomCarrierProfileEntry,
    error: String,
}

#[derive(Debug, Default)]
struct LoadedCustomProfiles {
    valid: Vec<(CustomCarrierProfileEntry, CarrierProfileRecord)>,
    invalid: Vec<InvalidCustomProfile>,
}

impl ProfileStore {
    pub fn new(catalog: Arc<CarrierCatalog>, database: Arc<Database>) -> Self {
        Self { catalog, database }
    }

    fn custom_records(&self) -> Result<LoadedCustomProfiles, String> {
        let entries = self
            .database
            .list_custom_carrier_profiles()
            .map_err(|error| format!("custom_carrier_profile_list_failed:{error}"))?;
        let mut loaded = LoadedCustomProfiles::default();
        for entry in entries {
            match CarrierProfileRecord::from_database_json(&entry.record_json) {
                Ok(record) => loaded.valid.push((entry, record)),
                Err(error) => {
                    let error = format!(
                        "custom_carrier_profile_invalid:{}:{error}",
                        entry.profile_id
                    );
                    tracing::warn!(
                        profile_id = %entry.profile_id,
                        plmn = %entry.plmn,
                        error = %error,
                        "Ignoring invalid custom carrier profile while keeping other database profiles usable"
                    );
                    loaded.invalid.push(InvalidCustomProfile { entry, error });
                }
            }
        }
        Ok(loaded)
    }

    /// List every selectable row for one access without merging identical ids
    /// across origins. The VoLTE line editor needs the `(origin, profile_id)`
    /// pair to remain unambiguous when a custom row deliberately shadows a
    /// downloaded catalog row.
    ///
    /// Catalog rows that only have the other access projection are included as
    /// disabled choices. This lets the API/UI distinguish "profile exists but
    /// has no LTE projection" from "profile does not exist".
    pub fn list_for_access(&self, access: CatalogAccessKind) -> Result<Vec<StoredProfile>, String> {
        let capabilities = self.catalog.service_capabilities().unwrap_or_default();
        let alternate_access = match access {
            CatalogAccessKind::LteEpc => CatalogAccessKind::WifiEpdg,
            CatalogAccessKind::WifiEpdg => CatalogAccessKind::LteEpc,
        };
        let mut catalog_profiles = BTreeMap::new();
        for projection in [access, alternate_access] {
            match self.catalog.list(projection) {
                Ok(entries) => {
                    for entry in entries {
                        catalog_profiles
                            .entry(entry.record.meta.profile_id.clone())
                            .or_insert((entry, projection));
                    }
                }
                Err(error) => tracing::debug!(
                    error = %error,
                    access = projection.as_str(),
                    "Carrier catalog unavailable while listing source-specific profiles"
                ),
            }
        }

        let mut profiles = Vec::new();
        for (_, (entry, loaded_projection)) in catalog_profiles {
            let capability = capabilities
                .get(&entry.record.meta.profile_id)
                .cloned()
                .unwrap_or_default();
            profiles.push(StoredProfile {
                profile_id: entry.record.meta.profile_id.clone(),
                plmn: entry.record.meta.plmn.clone(),
                origin: ProfileOrigin::Catalog,
                source: format!("carrier_catalog:{}", entry.release.release_id),
                updated_at: entry.release.generated_at,
                volte_ready: capability.volte_ready
                    || loaded_projection == CatalogAccessKind::LteEpc,
                vowifi_ready: capability.vowifi_ready
                    || loaded_projection == CatalogAccessKind::WifiEpdg,
                vilte_enabled: capability.vilte_enabled,
                smsoip_enabled: capability.smsoip_enabled,
                ut_xcap_enabled: capability.ut_xcap_enabled,
                record: entry.record,
            });
        }
        for (entry, record) in self.custom_records()?.valid {
            // A custom profile is stored as one complete IMS record rather than
            // separate LTE/Wi-Fi projections. `from_database_json` already
            // validates the shared IMS portion, so every valid database row has
            // an LTE projection by construction.
            let volte_ready = record.validate_ims_only().is_ok();
            profiles.push(StoredProfile {
                profile_id: entry.profile_id,
                plmn: entry.plmn,
                origin: ProfileOrigin::Database,
                source: "manual".to_string(),
                updated_at: entry.updated_at,
                volte_ready,
                vowifi_ready: record.voice.vowifi_enabled,
                vilte_enabled: false,
                smsoip_enabled: true,
                ut_xcap_enabled: record.ut.enabled,
                record,
            });
        }
        profiles.sort_by(|left, right| {
            profile_origin_rank(left.origin)
                .cmp(&profile_origin_rank(right.origin))
                .then(left.plmn.cmp(&right.plmn))
                .then(left.profile_id.cmp(&right.profile_id))
        });
        Ok(profiles)
    }

    /// Check an explicit profile reference without allowing source crossover or
    /// derived fallback. Runtime resolution remains tolerant after a saved row
    /// is later removed; this stricter check is only for accepting a new PUT.
    pub fn volte_reference_state(
        &self,
        source: VolteProfileSource,
        profile_id: &str,
    ) -> Result<VolteProfileReferenceState, String> {
        let profile_id = profile_id.trim();
        if profile_id.is_empty() {
            return Ok(VolteProfileReferenceState::Missing);
        }
        match source {
            VolteProfileSource::Database => {
                let records = self.custom_records()?;
                if records
                    .valid
                    .iter()
                    .any(|(_, record)| record.meta.profile_id == profile_id)
                {
                    return Ok(VolteProfileReferenceState::Ready);
                }
                if records
                    .invalid
                    .iter()
                    .any(|invalid| invalid.entry.profile_id == profile_id)
                {
                    return Ok(VolteProfileReferenceState::NotLteReady);
                }
                Ok(VolteProfileReferenceState::Missing)
            }
            VolteProfileSource::CarrierCatalog => {
                let capabilities = self.catalog.service_capabilities()?;
                let Some(capability) = capabilities.get(profile_id) else {
                    return Ok(VolteProfileReferenceState::Missing);
                };
                if !capability.volte_ready {
                    return Ok(VolteProfileReferenceState::NotLteReady);
                }
                match self.catalog.get(profile_id, CatalogAccessKind::LteEpc) {
                    Ok(Some(_)) => Ok(VolteProfileReferenceState::Ready),
                    Ok(None) => Ok(VolteProfileReferenceState::Missing),
                    Err(error) => {
                        tracing::warn!(
                            profile_id,
                            error = %error,
                            "Catalog marks a profile LTE-ready but its LTE projection is unusable"
                        );
                        Ok(VolteProfileReferenceState::NotLteReady)
                    }
                }
            }
            VolteProfileSource::Derived => Ok(VolteProfileReferenceState::Missing),
        }
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
        for (entry, record) in self.custom_records()?.valid {
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

    /// List database-browser rows without merging identical profile ids across
    /// the user database and downloaded catalog.
    pub fn list_stored_profiles(&self) -> Result<Vec<StoredProfile>, String> {
        self.list_for_access(CatalogAccessKind::WifiEpdg)
    }

    pub fn list_stored_profile_summaries(&self) -> Result<Vec<StoredProfileSummary>, String> {
        let mut summaries = Vec::new();
        match self.catalog.list_summaries() {
            Ok(catalog) => {
                summaries.extend(catalog.into_iter().map(|profile| StoredProfileSummary {
                    profile_id: profile.profile_id,
                    plmn: profile.plmn,
                    mcc: profile.mcc,
                    brand: profile.brand,
                    operator_legal_name: profile.operator_legal_name,
                    aliases: profile.aliases,
                    origin: ProfileOrigin::Catalog,
                    source: format!("carrier_catalog:{}", profile.release.release_id),
                    updated_at: profile.release.generated_at,
                    volte_ready: profile.volte_ready,
                    vowifi_ready: profile.vowifi_ready,
                    vilte_enabled: profile.vilte_enabled,
                    smsoip_enabled: profile.smsoip_enabled,
                    ut_xcap_enabled: profile.ut_xcap_enabled,
                }))
            }
            Err(error) => tracing::debug!(
                error = %error,
                "Carrier catalog unavailable while listing profile summaries"
            ),
        }
        for (entry, record) in self.custom_records()?.valid {
            summaries.push(StoredProfileSummary {
                profile_id: entry.profile_id,
                plmn: entry.plmn,
                mcc: record.meta.mcc.clone(),
                brand: record.meta.brand.clone(),
                operator_legal_name: record.meta.operator_legal_name.clone(),
                aliases: record.meta.aliases.clone(),
                origin: ProfileOrigin::Database,
                source: "manual".to_string(),
                updated_at: entry.updated_at,
                volte_ready: record.validate_ims_only().is_ok(),
                vowifi_ready: record.voice.vowifi_enabled,
                vilte_enabled: false,
                smsoip_enabled: true,
                ut_xcap_enabled: record.ut.enabled,
            });
        }
        summaries.sort_by(|left, right| {
            profile_origin_rank(left.origin)
                .cmp(&profile_origin_rank(right.origin))
                .then(left.plmn.cmp(&right.plmn))
                .then(left.brand.cmp(&right.brand))
                .then(left.profile_id.cmp(&right.profile_id))
        });
        Ok(summaries)
    }

    pub fn get_stored_profile(
        &self,
        origin: ProfileOrigin,
        profile_id: &str,
    ) -> Result<Option<StoredProfile>, String> {
        let profile_id = profile_id.trim();
        if profile_id.is_empty() {
            return Ok(None);
        }
        match origin {
            ProfileOrigin::Database => Ok(self
                .custom_records()?
                .valid
                .into_iter()
                .find(|(entry, _)| entry.profile_id == profile_id)
                .map(|(entry, record)| StoredProfile {
                    profile_id: entry.profile_id,
                    plmn: entry.plmn,
                    origin: ProfileOrigin::Database,
                    source: "manual".to_string(),
                    updated_at: entry.updated_at,
                    volte_ready: record.validate_ims_only().is_ok(),
                    vowifi_ready: record.voice.vowifi_enabled,
                    vilte_enabled: false,
                    smsoip_enabled: true,
                    ut_xcap_enabled: record.ut.enabled,
                    record,
                })),
            ProfileOrigin::Catalog => {
                let Some(summary) = self
                    .catalog
                    .list_summaries()?
                    .into_iter()
                    .find(|profile| profile.profile_id == profile_id)
                else {
                    return Ok(None);
                };
                let access = if summary.vowifi_ready {
                    CatalogAccessKind::WifiEpdg
                } else {
                    CatalogAccessKind::LteEpc
                };
                let Some(profile) = self.catalog.get(profile_id, access)? else {
                    return Ok(None);
                };
                Ok(Some(StoredProfile {
                    profile_id: summary.profile_id,
                    plmn: summary.plmn,
                    origin: ProfileOrigin::Catalog,
                    source: format!("carrier_catalog:{}", summary.release.release_id),
                    updated_at: summary.release.generated_at,
                    record: profile.record,
                    volte_ready: summary.volte_ready,
                    vowifi_ready: summary.vowifi_ready,
                    vilte_enabled: summary.vilte_enabled,
                    smsoip_enabled: summary.smsoip_enabled,
                    ut_xcap_enabled: summary.ut_xcap_enabled,
                }))
            }
            ProfileOrigin::Derived => Ok(None),
        }
    }

    /// Search only operator-authored rows and downloaded catalog rows.
    ///
    /// This is intentionally separate from the runtime resolvers below: the
    /// database browser must never manufacture a derived 3GPP profile when a
    /// stored carrier does not match.
    pub fn search_stored_profiles(
        &self,
        plmn: Option<&str>,
        mcc: Option<&str>,
        name: Option<&str>,
    ) -> Result<Vec<StoredProfile>, String> {
        let plmn = plmn.map(str::trim).filter(|value| !value.is_empty());
        let mcc = mcc.map(str::trim).filter(|value| !value.is_empty());
        let name = name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_lowercase);

        let mut profiles = self.list_stored_profiles()?;
        profiles.retain(|profile| {
            if plmn.is_some_and(|value| profile.plmn != value) {
                return false;
            }
            if mcc.is_some_and(|value| profile.record.meta.mcc != value) {
                return false;
            }
            if let Some(query) = name.as_deref() {
                let meta = &profile.record.meta;
                let matches_name = [
                    meta.brand.as_str(),
                    meta.operator_legal_name.as_str(),
                    profile.profile_id.as_str(),
                ]
                .into_iter()
                .chain(meta.aliases.iter().map(String::as_str))
                .any(|candidate| candidate.to_lowercase().contains(query));
                if !matches_name {
                    return false;
                }
            }
            matches!(
                profile.origin,
                ProfileOrigin::Database | ProfileOrigin::Catalog
            )
        });
        profiles.sort_by(|left, right| {
            profile_origin_rank(left.origin)
                .cmp(&profile_origin_rank(right.origin))
                .then(left.plmn.cmp(&right.plmn))
                .then(left.record.meta.brand.cmp(&right.record.meta.brand))
                .then(left.profile_id.cmp(&right.profile_id))
        });
        Ok(profiles)
    }

    pub fn upsert(&self, mut record: CarrierProfileRecord) -> Result<StoredProfile, String> {
        record.schema_version = CURRENT_SCHEMA_VERSION;
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
            let custom_records = self.custom_records()?.valid;
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
        match self.custom_records() {
            Ok(records) => {
                if let Some((_, record)) = records
                    .valid
                    .into_iter()
                    .find(|(_, record)| record.meta.plmn == plmn)
                {
                    return Some(ResolvedProfile {
                        profile: record.intern(),
                        origin: ProfileOrigin::Database,
                        fallback_reason: None,
                    });
                }
            }
            Err(error) => tracing::warn!(
                error = %error,
                plmn = %plmn,
                "Custom carrier profile lookup failed; continuing with catalog and standard fallback"
            ),
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

    /// Resolve one source-constrained VoLTE candidate. Missing or unusable
    /// database/catalog rows intentionally fall back to a standards-derived
    /// profile for the same logical slot. A legacy SIM pin is consulted only
    /// inside the source where that id actually exists; an explicit line id is
    /// strict and always wins over it.
    pub fn resolve_volte_candidate(
        &self,
        candidate: &VolteProfileCandidate,
        legacy_pinned_profile_id: Option<&str>,
        imsi: &str,
        home_plmn: Option<&str>,
    ) -> Result<Option<ResolvedProfile>, String> {
        let digits = imsi.trim();
        let explicit_home_plmn = normalized_home_plmn(digits, home_plmn);
        let inferred_home_plmn = explicit_home_plmn
            .clone()
            .or_else(|| self.catalog.infer_home_plmn(digits).ok().flatten());
        let source = candidate.source;
        let explicit_profile_id = candidate
            .profile_id
            .as_deref()
            .map(str::trim)
            .filter(|profile_id| !profile_id.is_empty());
        let legacy_profile_id = legacy_pinned_profile_id
            .map(str::trim)
            .filter(|profile_id| !profile_id.is_empty());

        let requested = match source {
            VolteProfileSource::Database => {
                let records = match self.custom_records() {
                    Ok(records) => records,
                    Err(error) => {
                        return Ok(derive_standard_fallback(
                            digits,
                            inferred_home_plmn.as_deref(),
                            CatalogAccessKind::LteEpc,
                            format!("volte_profile_database_lookup_failed:{error}"),
                        ));
                    }
                };
                if let Some(profile_id) = explicit_profile_id {
                    if let Some((_, record)) = records
                        .valid
                        .iter()
                        .find(|(_, record)| record.meta.profile_id == profile_id)
                    {
                        return Ok(Some(ResolvedProfile {
                            profile: record.clone().intern(),
                            origin: ProfileOrigin::Database,
                            fallback_reason: None,
                        }));
                    }
                    let reason = records
                        .invalid
                        .iter()
                        .find(|invalid| invalid.entry.profile_id == profile_id)
                        .map(|invalid| invalid.error.clone())
                        .unwrap_or_else(|| {
                            format!("volte_profile_database_profile_not_found:{profile_id}")
                        });
                    return Ok(derive_standard_fallback(
                        digits,
                        inferred_home_plmn.as_deref(),
                        CatalogAccessKind::LteEpc,
                        reason,
                    ));
                }
                if let Some(profile_id) = legacy_profile_id {
                    if let Some((_, record)) = records
                        .valid
                        .iter()
                        .find(|(_, record)| record.meta.profile_id == profile_id)
                    {
                        return Ok(Some(ResolvedProfile {
                            profile: record.clone().intern(),
                            origin: ProfileOrigin::Database,
                            fallback_reason: None,
                        }));
                    }
                }
                let mut matches = records
                    .valid
                    .into_iter()
                    .filter(|(_, record)| {
                        explicit_home_plmn.as_deref().map_or_else(
                            || digits.starts_with(&record.meta.plmn),
                            |plmn| record.meta.plmn == plmn,
                        )
                    })
                    .collect::<Vec<_>>();
                matches.sort_by(|left, right| {
                    right
                        .1
                        .meta
                        .plmn
                        .len()
                        .cmp(&left.1.meta.plmn.len())
                        .then(left.1.meta.profile_id.cmp(&right.1.meta.profile_id))
                });
                matches
                    .into_iter()
                    .next()
                    .map(|(_, record)| ResolvedProfile {
                        profile: record.intern(),
                        origin: ProfileOrigin::Database,
                        fallback_reason: None,
                    })
            }
            VolteProfileSource::CarrierCatalog => {
                if let Some(profile_id) = explicit_profile_id {
                    match self.catalog.get(profile_id, CatalogAccessKind::LteEpc) {
                        Ok(Some(profile)) => {
                            return Ok(Some(ResolvedProfile {
                                profile: profile.record.intern(),
                                origin: ProfileOrigin::Catalog,
                                fallback_reason: None,
                            }));
                        }
                        Ok(None) => {
                            return Ok(derive_standard_fallback(
                                digits,
                                inferred_home_plmn.as_deref(),
                                CatalogAccessKind::LteEpc,
                                format!(
                                    "volte_profile_carrier_catalog_profile_not_found:{profile_id}"
                                ),
                            ));
                        }
                        Err(error) => {
                            return Ok(derive_standard_fallback(
                                digits,
                                inferred_home_plmn.as_deref(),
                                CatalogAccessKind::LteEpc,
                                format!(
                                    "volte_profile_carrier_catalog_profile_lookup_failed:{profile_id}:{error}"
                                ),
                            ));
                        }
                    }
                }
                if let Some(profile_id) = legacy_profile_id {
                    if let Ok(Some(profile)) =
                        self.catalog.get(profile_id, CatalogAccessKind::LteEpc)
                    {
                        return Ok(Some(ResolvedProfile {
                            profile: profile.record.intern(),
                            origin: ProfileOrigin::Catalog,
                            fallback_reason: None,
                        }));
                    }
                }
                match self.catalog.imsi_has_ambiguous_plmn(digits) {
                    Ok(true) if inferred_home_plmn.is_none() => None,
                    Ok(_) => match self.catalog.resolve_for_imsi(
                        digits,
                        inferred_home_plmn.as_deref(),
                        CatalogAccessKind::LteEpc,
                    ) {
                        Ok(profile) => profile.map(|profile| ResolvedProfile {
                            profile: profile.record.intern(),
                            origin: ProfileOrigin::Catalog,
                            fallback_reason: None,
                        }),
                        Err(error) => {
                            return Ok(derive_standard_fallback(
                                digits,
                                inferred_home_plmn.as_deref(),
                                CatalogAccessKind::LteEpc,
                                error,
                            ));
                        }
                    },
                    Err(error) => {
                        return Ok(derive_standard_fallback(
                            digits,
                            inferred_home_plmn.as_deref(),
                            CatalogAccessKind::LteEpc,
                            error,
                        ));
                    }
                }
            }
            VolteProfileSource::Derived => {
                return Ok(derive_standard_fallback(
                    digits,
                    inferred_home_plmn.as_deref(),
                    CatalogAccessKind::LteEpc,
                    "volte_profile_derived_requested".to_string(),
                )
                .map(|mut resolved| {
                    resolved.fallback_reason = None;
                    resolved
                }));
            }
        };

        if let Some(resolved) = requested {
            return Ok(Some(resolved));
        }
        Ok(derive_standard_fallback(
            digits,
            inferred_home_plmn.as_deref(),
            CatalogAccessKind::LteEpc,
            format!("volte_profile_source_unavailable:{}", source.as_str()),
        ))
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
            let custom_records = self.custom_records()?;
            if let Some((_, record)) = custom_records
                .valid
                .into_iter()
                .find(|(_, record)| record.meta.profile_id == profile_id)
            {
                return Ok(Some(ResolvedProfile {
                    profile: record.intern(),
                    origin: ProfileOrigin::Database,
                    fallback_reason: None,
                }));
            }
            if let Some(invalid) = custom_records
                .invalid
                .into_iter()
                .find(|invalid| invalid.entry.profile_id == profile_id)
            {
                return Err(invalid.error);
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
            Ok(records) => {
                let relevant_errors = records
                    .invalid
                    .into_iter()
                    .filter(|invalid| {
                        explicit_home_plmn.as_deref().map_or_else(
                            || digits.starts_with(&invalid.entry.plmn),
                            |plmn| invalid.entry.plmn == plmn,
                        )
                    })
                    .map(|invalid| invalid.error)
                    .collect::<Vec<_>>();
                (
                    records.valid,
                    (!relevant_errors.is_empty()).then(|| relevant_errors.join("|")),
                )
            }
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
        let home_plmn =
            explicit_home_plmn.or_else(|| self.catalog.infer_home_plmn(digits).ok().flatten());
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

fn profile_origin_rank(origin: ProfileOrigin) -> u8 {
    match origin {
        ProfileOrigin::Database => 0,
        ProfileOrigin::Catalog => 1,
        ProfileOrigin::Derived => 2,
    }
}

fn normalized_home_plmn(imsi: &str, home_plmn: Option<&str>) -> Option<String> {
    home_plmn
        .map(str::trim)
        .filter(|plmn| {
            matches!(plmn.len(), 5 | 6)
                && plmn.bytes().all(|byte| byte.is_ascii_digit())
                && imsi.starts_with(*plmn)
        })
        .map(str::to_string)
}

fn derive_standard_fallback(
    imsi: &str,
    home_plmn: Option<&str>,
    access: CatalogAccessKind,
    fallback_reason: String,
) -> Option<ResolvedProfile> {
    let plmn = home_plmn.filter(|plmn| {
        matches!(plmn.len(), 5 | 6)
            && plmn.bytes().all(|byte| byte.is_ascii_digit())
            && imsi.starts_with(*plmn)
    })?;
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredProfileSummary {
    pub profile_id: String,
    pub plmn: String,
    pub mcc: String,
    pub brand: String,
    pub operator_legal_name: String,
    pub aliases: Vec<String>,
    pub origin: ProfileOrigin,
    pub source: String,
    pub updated_at: String,
    pub volte_ready: bool,
    pub vowifi_ready: bool,
    pub vilte_enabled: bool,
    pub smsoip_enabled: bool,
    pub ut_xcap_enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::config::VolteProfileSelectionConfig;
    use std::path::PathBuf;

    fn store_with_catalog() -> (ProfileStore, PathBuf) {
        let (catalog, path) = super::super::carrier_catalog::test_catalog_fixture();
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        (ProfileStore::new(Arc::new(catalog), database), path)
    }

    fn explicit_private_record(profile_id: &str) -> CarrierProfileRecord {
        let mut record = CarrierProfileRecord::from_profile(&profiles::GB_EE_23433);
        record.meta.profile_id = profile_id.to_string();
        record.meta.mcc = "999".to_string();
        record.meta.mnc = "99".to_string();
        record.meta.mnc_len = 2;
        record.meta.plmn = "99999".to_string();
        record.meta.brand = "Private network".to_string();
        record.ims.domain = "ims.private.example".to_string();
        record.ims.realm = "ims.private.example".to_string();
        record.ims.registrar = Some("sip:ims.private.example".to_string());
        record.epdg.host = "epdg.private.example".to_string();
        record.ikev2.identity_template = Some("private-{imsi}@{ims_realm}".to_string());
        record
    }

    fn rewrite_catalog_fixture_as_private(path: &std::path::Path) {
        let conn = rusqlite::Connection::open(path).expect("open catalog fixture");
        conn.execute(
            "UPDATE profile_match_rules
                SET plmn = '99999', imsi_prefix = '999990'
              WHERE profile_id = 'test-v7-23433'",
            [],
        )
        .expect("rewrite catalog PLMN");
        let config_json = conn
            .query_row(
                "SELECT config_json FROM carrier_profiles WHERE profile_id = 'test-v7-23433'",
                [],
                |row| row.get::<_, String>(0),
            )
            .expect("read catalog config");
        let mut config: serde_json::Value =
            serde_json::from_str(&config_json).expect("parse catalog config");
        config["ims"]["home_domain"] = serde_json::json!("ims.private.example");
        config["ims"]["realm"] = serde_json::json!("ims.private.example");
        config["access"]["vowifi"]["epdg"] =
            serde_json::json!([{ "address": "epdg.private.example", "port": 500 }]);
        config["access"]["vowifi"]["ike"]["identities"]["idi"] = serde_json::json!([
            {
                "identity_type": "id_rfc822_addr",
                "value_template": "private-{imsi}@{ims_realm}"
            }
        ]);
        conn.execute(
            "UPDATE carrier_profiles SET config_json = ?1 WHERE profile_id = 'test-v7-23433'",
            [serde_json::to_string(&config).expect("serialize catalog config")],
        )
        .expect("write catalog config");
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
        let direct_fallback = store
            .resolve_by_plmn("460", "01")
            .expect("standard PLMN fallback");
        assert_eq!(direct_fallback.origin, ProfileOrigin::Derived);
        assert_eq!(
            direct_fallback.profile.meta.profile_id,
            "derived_3gpp_vowifi_46001"
        );
        let fallback = store
            .resolve_for_imsi_access(
                None,
                "460011234567890",
                Some("46001"),
                CatalogAccessKind::LteEpc,
            )
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
    fn stored_profile_search_never_returns_a_derived_fallback() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();

        let missing = store
            .search_stored_profiles(Some("46001"), None, None)
            .expect("search missing PLMN");
        assert!(missing.is_empty());

        let runtime = store
            .resolve_by_plmn("460", "01")
            .expect("runtime keeps its standard fallback");
        assert_eq!(runtime.origin, ProfileOrigin::Derived);

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn stored_profile_search_supports_mcc_and_operator_name() {
        let (store, path) = store_with_catalog();

        let by_mcc = store
            .search_stored_profiles(None, Some("234"), None)
            .expect("search MCC");
        assert_eq!(by_mcc.len(), 2);
        assert!(by_mcc
            .iter()
            .all(|profile| profile.record.meta.mcc == "234"));

        let by_alias = store
            .search_stored_profiles(None, None, Some("tElEcOm"))
            .expect("search alias");
        assert_eq!(by_alias.len(), 2);
        assert!(by_alias
            .iter()
            .all(|profile| profile.origin == ProfileOrigin::Catalog));

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn stored_profile_search_includes_custom_rows_by_mcc_and_name() {
        let (store, path) = store_with_catalog();
        let mut custom = explicit_private_record("private-db-99999");
        custom.meta.brand = "Acme Private Wireless".to_string();
        custom.meta.operator_legal_name = "Acme Network Limited".to_string();
        custom.meta.aliases = vec!["APW".to_string()];
        store.upsert(custom).expect("save custom profile");

        let by_mcc = store
            .search_stored_profiles(None, Some("999"), None)
            .expect("search custom MCC");
        assert_eq!(by_mcc.len(), 1);
        assert_eq!(by_mcc[0].origin, ProfileOrigin::Database);
        assert_eq!(by_mcc[0].profile_id, "private-db-99999");

        for query in ["acme private", "NETWORK LIMITED", "apw"] {
            let by_name = store
                .search_stored_profiles(None, None, Some(query))
                .expect("search custom operator name");
            assert_eq!(by_name.len(), 1, "query: {query}");
            assert_eq!(by_name[0].origin, ProfileOrigin::Database);
            assert_eq!(by_name[0].profile_id, "private-db-99999");
        }

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn stored_profile_search_places_custom_rows_before_catalog_rows() {
        let (store, path) = store_with_catalog();
        let mut custom = store
            .list()
            .expect("list catalog profiles")
            .into_iter()
            .find(|profile| profile.profile_id == "test-v7-23433")
            .expect("catalog fixture profile")
            .record;
        custom.meta.brand = "Local Test Mobile".to_string();
        store.upsert(custom).expect("save custom profile");

        let matches = store
            .search_stored_profiles(Some("23433"), None, None)
            .expect("search PLMN");
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].origin, ProfileOrigin::Database);
        assert_eq!(matches[0].profile_id, "test-v7-23433");
        assert_eq!(matches[1].origin, ProfileOrigin::Catalog);
        assert_eq!(matches[1].profile_id, "test-v7-23433");

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn profile_summaries_keep_sources_distinct_and_details_source_bound() {
        let (store, path) = store_with_catalog();
        let mut custom = store
            .list()
            .expect("list catalog profiles")
            .into_iter()
            .find(|profile| profile.profile_id == "test-v7-23433")
            .expect("catalog fixture profile")
            .record;
        custom.meta.brand = "User Summary Override".to_string();
        store.upsert(custom).expect("save same-id custom profile");

        let summaries = store
            .list_stored_profile_summaries()
            .expect("list lightweight summaries");
        let same_id = summaries
            .iter()
            .filter(|profile| profile.profile_id == "test-v7-23433")
            .collect::<Vec<_>>();
        assert_eq!(same_id.len(), 2);
        assert_eq!(same_id[0].origin, ProfileOrigin::Database);
        assert_eq!(same_id[0].brand, "User Summary Override");
        assert_eq!(same_id[1].origin, ProfileOrigin::Catalog);
        assert_eq!(same_id[1].mcc, "234");
        assert!(serde_json::to_value(same_id[0])
            .expect("serialize summary")
            .get("record")
            .is_none());

        let database = store
            .get_stored_profile(ProfileOrigin::Database, "test-v7-23433")
            .expect("load database detail")
            .expect("database detail exists");
        let catalog = store
            .get_stored_profile(ProfileOrigin::Catalog, "test-v7-23433")
            .expect("load catalog detail")
            .expect("catalog detail exists");
        assert_eq!(database.record.meta.brand, "User Summary Override");
        assert_ne!(catalog.record.meta.brand, "User Summary Override");

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn private_plmn_never_receives_a_public_standard_fallback() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database);

        assert!(store.resolve_by_plmn("999", "99").is_none());
        assert!(store
            .resolve_for_imsi_access(
                None,
                "999990123456789",
                Some("99999"),
                CatalogAccessKind::WifiEpdg,
            )
            .expect("private automatic lookup")
            .is_none());

        for candidate in VolteProfileSelectionConfig::default().attempts {
            assert!(store
                .resolve_volte_candidate(&candidate, None, "999990123456789", Some("99999"),)
                .expect("private VoLTE candidate lookup")
                .is_none());
        }
    }

    #[test]
    fn explicit_database_profile_remains_available_for_private_plmn() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database);
        store
            .upsert(explicit_private_record("private-db-99999"))
            .expect("save explicit private database profile");

        let by_plmn = store
            .resolve_by_plmn("999", "99")
            .expect("private database PLMN match");
        assert_eq!(by_plmn.origin, ProfileOrigin::Database);
        assert_eq!(by_plmn.profile.ims.domain, "ims.private.example");
        assert_eq!(by_plmn.profile.epdg.host, "epdg.private.example");

        let resolved = store
            .resolve_volte_candidate(
                &VolteProfileCandidate {
                    source: VolteProfileSource::Database,
                    profile_id: Some("private-db-99999".to_string()),
                },
                None,
                "999990123456789",
                Some("99999"),
            )
            .expect("private database candidate lookup")
            .expect("private database profile");
        assert_eq!(resolved.origin, ProfileOrigin::Database);
    }

    #[test]
    fn explicit_catalog_profile_remains_available_for_private_plmn() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        rewrite_catalog_fixture_as_private(&path);

        let by_plmn = store
            .resolve_by_plmn("999", "99")
            .expect("private catalog PLMN match");
        assert_eq!(by_plmn.origin, ProfileOrigin::Catalog);
        assert_eq!(by_plmn.profile.ims.domain, "ims.private.example");
        assert_eq!(by_plmn.profile.epdg.host, "epdg.private.example");

        let resolved = store
            .resolve_volte_candidate(
                &VolteProfileCandidate {
                    source: VolteProfileSource::CarrierCatalog,
                    profile_id: Some("test-v7-23433".to_string()),
                },
                None,
                "999990123456789",
                Some("99999"),
            )
            .expect("private catalog candidate lookup")
            .expect("private catalog profile");
        assert_eq!(resolved.origin, ProfileOrigin::Catalog);

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn unknown_imsi_does_not_guess_a_two_or_three_digit_mnc() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database);

        let resolved = store
            .resolve_for_imsi_access(None, "310260123456789", None, CatalogAccessKind::LteEpc)
            .expect("ambiguous query should remain non-fatal");

        assert!(resolved.is_none());
    }

    #[test]
    fn source_constrained_ids_keep_database_and_catalog_rows_distinct() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        let mut custom = store
            .list()
            .expect("catalog list")
            .into_iter()
            .find(|profile| {
                profile.origin == ProfileOrigin::Catalog && profile.profile_id == "test-v7-23433"
            })
            .expect("catalog fixture")
            .record;
        custom.meta.brand = "User override".to_string();
        store.upsert(custom).expect("save same-id custom row");

        let database = store
            .resolve_volte_candidate(
                &VolteProfileCandidate {
                    source: VolteProfileSource::Database,
                    profile_id: Some("test-v7-23433".to_string()),
                },
                None,
                "234330123456789",
                Some("23433"),
            )
            .expect("database resolution")
            .expect("database profile");
        let catalog = store
            .resolve_volte_candidate(
                &VolteProfileCandidate {
                    source: VolteProfileSource::CarrierCatalog,
                    profile_id: Some("test-v7-23433".to_string()),
                },
                None,
                "234330123456789",
                Some("23433"),
            )
            .expect("catalog resolution")
            .expect("catalog profile");

        assert_eq!(database.origin, ProfileOrigin::Database);
        assert_eq!(database.profile.meta.brand, "User override");
        assert_eq!(catalog.origin, ProfileOrigin::Catalog);
        assert_ne!(catalog.profile.meta.brand, "User override");
        let selectable = store
            .list_for_access(CatalogAccessKind::LteEpc)
            .expect("selectable profiles")
            .into_iter()
            .filter(|profile| profile.profile_id == "test-v7-23433")
            .collect::<Vec<_>>();
        assert_eq!(selectable.len(), 2);
        assert_ne!(selectable[0].origin, selectable[1].origin);

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn every_missing_source_slot_falls_back_to_derived_without_deduplication() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database);
        let candidates = VolteProfileSelectionConfig::default().attempts;
        let resolved = candidates
            .iter()
            .map(|candidate| {
                store
                    .resolve_volte_candidate(candidate, None, "502121234567890", Some("50212"))
                    .expect("candidate resolution")
                    .expect("derived fallback")
            })
            .collect::<Vec<_>>();

        assert_eq!(resolved.len(), 3, "logical slots must not be deduplicated");
        assert!(resolved
            .iter()
            .all(|profile| profile.origin == ProfileOrigin::Derived));
        assert!(resolved
            .iter()
            .all(|profile| { profile.profile.meta.profile_id == "derived_3gpp_lte_50212" }));
        assert!(resolved[0].fallback_reason.is_some());
        assert!(resolved[1].fallback_reason.is_some());
        assert!(resolved[2].fallback_reason.is_none());
    }

    #[test]
    fn volte_database_candidate_auto_matches_home_plmn() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database);
        let mut record = CarrierProfileRecord::from_profile(&profiles::GB_EE_23433);
        record.meta.profile_id = "custom-50212".to_string();
        record.meta.mcc = "502".to_string();
        record.meta.mnc = "12".to_string();
        record.meta.mnc_len = 2;
        record.meta.plmn = "50212".to_string();
        store.upsert(record).expect("save database profile");

        let resolved = store
            .resolve_volte_candidate(
                &VolteProfileCandidate::automatic(VolteProfileSource::Database),
                None,
                "502121234567890",
                Some("50212"),
            )
            .expect("database automatic resolution")
            .expect("database profile");

        assert_eq!(resolved.origin, ProfileOrigin::Database);
        assert_eq!(resolved.profile.meta.profile_id, "custom-50212");
        assert!(resolved.fallback_reason.is_none());
    }

    #[test]
    fn volte_catalog_candidate_auto_matches_lte_catalog() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();

        let resolved = store
            .resolve_volte_candidate(
                &VolteProfileCandidate::automatic(VolteProfileSource::CarrierCatalog),
                None,
                "234330123456789",
                Some("23433"),
            )
            .expect("catalog automatic resolution")
            .expect("catalog profile");

        assert_eq!(resolved.origin, ProfileOrigin::Catalog);
        assert_eq!(resolved.profile.meta.profile_id, "test-v7-23433");
        assert!(resolved.fallback_reason.is_none());
        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn deleted_explicit_source_profile_falls_back_to_derived() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database);

        for source in [
            VolteProfileSource::Database,
            VolteProfileSource::CarrierCatalog,
        ] {
            let resolved = store
                .resolve_volte_candidate(
                    &VolteProfileCandidate {
                        source,
                        profile_id: Some("deleted-profile".to_string()),
                    },
                    None,
                    "502121234567890",
                    Some("50212"),
                )
                .expect("missing explicit profile resolution")
                .expect("derived fallback");
            assert_eq!(resolved.origin, ProfileOrigin::Derived);
            assert!(resolved
                .fallback_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("deleted-profile")));
        }
    }

    #[test]
    fn legacy_pin_only_applies_inside_the_matching_source() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        let legacy_pin = Some("test-v7-23433");

        let database = store
            .resolve_volte_candidate(
                &VolteProfileCandidate::automatic(VolteProfileSource::Database),
                legacy_pin,
                "234330123456789",
                Some("23433"),
            )
            .expect("database resolution")
            .expect("derived database fallback");
        let catalog = store
            .resolve_volte_candidate(
                &VolteProfileCandidate::automatic(VolteProfileSource::CarrierCatalog),
                legacy_pin,
                "234330123456789",
                Some("23433"),
            )
            .expect("catalog resolution")
            .expect("catalog profile");

        assert_eq!(database.origin, ProfileOrigin::Derived);
        assert_eq!(catalog.origin, ProfileOrigin::Catalog);
        assert_eq!(catalog.profile.meta.profile_id, "test-v7-23433");
        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn derived_candidate_never_reads_database_rows() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database);
        let mut record = CarrierProfileRecord::from_profile(&profiles::GB_EE_23433);
        record.meta.profile_id = "custom-50212".to_string();
        record.meta.mcc = "502".to_string();
        record.meta.mnc = "12".to_string();
        record.meta.mnc_len = 2;
        record.meta.plmn = "50212".to_string();
        store.upsert(record).expect("save database profile");

        let resolved = store
            .resolve_volte_candidate(
                &VolteProfileCandidate::automatic(VolteProfileSource::Derived),
                Some("custom-50212"),
                "502121234567890",
                Some("50212"),
            )
            .expect("derived resolution")
            .expect("derived profile");

        assert_eq!(resolved.origin, ProfileOrigin::Derived);
        assert_eq!(resolved.profile.meta.profile_id, "derived_3gpp_lte_50212");
        assert!(resolved.fallback_reason.is_none());
    }
    #[test]
    fn absent_catalog_uses_derived_fallback_without_a_runtime_switch() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
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
    fn malformed_database_row_does_not_poison_valid_profiles_or_fallbacks() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        store
            .database
            .upsert_custom_carrier_profile("broken-db-00101", "00101", "{")
            .expect("insert malformed database profile");

        let mut record = CarrierProfileRecord::from_profile(&profiles::GB_EE_23433);
        record.meta.profile_id = "valid-db-23433".to_string();
        record.meta.brand = "Valid DB Override".to_string();
        let json = serde_json::to_string(&record).expect("serialize valid database profile");
        store
            .database
            .upsert_custom_carrier_profile(&record.meta.profile_id, &record.meta.plmn, &json)
            .expect("insert valid database profile");

        let resolved = store
            .resolve_for_imsi_access(
                None,
                "234330123456789",
                Some("23433"),
                CatalogAccessKind::WifiEpdg,
            )
            .expect("automatic resolution must ignore unrelated malformed rows")
            .expect("valid database profile");
        assert_eq!(resolved.origin, ProfileOrigin::Database);
        assert_eq!(resolved.profile.meta.profile_id, "valid-db-23433");

        let listed = store.list().expect("list must retain valid rows");
        assert!(listed
            .iter()
            .any(|profile| profile.profile_id == "valid-db-23433"));
        assert!(!listed
            .iter()
            .any(|profile| profile.profile_id == "broken-db-00101"));

        store
            .database
            .delete_custom_carrier_profile("valid-db-23433")
            .expect("delete valid database profile");
        let catalog = store
            .resolve_by_plmn("234", "33")
            .expect("malformed row must not hide catalog fallback");
        assert_eq!(catalog.origin, ProfileOrigin::Catalog);

        let error = store
            .resolve_for_imsi_access(
                Some("broken-db-00101"),
                "001010123456789",
                Some("00101"),
                CatalogAccessKind::WifiEpdg,
            )
            .expect_err("an explicitly pinned malformed row must remain a hard error");
        assert!(error.starts_with("custom_carrier_profile_invalid:broken-db-00101:"));

        let derived = store
            .resolve_for_imsi_access(
                None,
                "001010123456789",
                Some("00101"),
                CatalogAccessKind::LteEpc,
            )
            .expect("automatic matching must fall back past a malformed target row")
            .expect("standard-derived fallback");
        assert_eq!(derived.origin, ProfileOrigin::Derived);
        assert!(derived.fallback_reason.as_deref().is_some_and(|reason| {
            reason.contains("custom_carrier_profile_invalid:broken-db-00101:")
        }));

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn legacy_database_profile_wins_and_preserves_explicit_disables() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let catalog =
            CarrierCatalog::at_path(PathBuf::from("/definitely-missing/carrier-bundles.sqlite3"));
        let database = Arc::new(
            Database::new(PathBuf::from(":memory:")).expect("create profile store database"),
        );
        let store = ProfileStore::new(Arc::new(catalog), database.clone());

        let mut record = CarrierProfileRecord::from_profile(&profiles::GB_EE_23433);
        record.schema_version = 0;
        record.meta.profile_id = "legacy-db-50212".to_string();
        record.meta.mcc = "502".to_string();
        record.meta.mnc = "12".to_string();
        record.meta.mnc_len = 2;
        record.meta.plmn = "50212".to_string();
        record.ims.register.include_mmtel_features = false;
        record.ims.register.enable_cellular_network_info = false;
        record.ims.register.always_add_sip_instance = false;
        record.ims.register.sec_agree_mode = "disabled".to_string();
        record.ims.register.require_sec_agree_headers = false;
        record.ims.register.proxy_require_sec_agree_headers = false;
        let json = serde_json::to_string(&record).expect("serialize legacy database profile");
        database
            .upsert_custom_carrier_profile(&record.meta.profile_id, &record.meta.plmn, &json)
            .expect("insert legacy database profile");

        let resolved = store
            .resolve_for_imsi_access(
                None,
                "502121234567890",
                Some("50212"),
                CatalogAccessKind::WifiEpdg,
            )
            .expect("resolve database profile")
            .expect("database profile");
        assert_eq!(resolved.origin, ProfileOrigin::Database);
        assert_eq!(resolved.profile.meta.profile_id, "legacy-db-50212");
        assert!(!resolved.profile.ims.register.include_mmtel_features);
        assert!(!resolved.profile.ims.register.enable_cellular_network_info);
        assert!(!resolved.profile.ims.register.always_add_sip_instance);
        assert_eq!(resolved.profile.ims.register.sec_agree_mode, "disabled");

        let listed = store.list().expect("list normalized database profile");
        let stored = listed
            .iter()
            .find(|profile| profile.profile_id == "legacy-db-50212")
            .expect("listed database profile");
        assert_eq!(stored.record.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn upsert_persists_current_profile_schema_version() {
        let (store, path) = store_with_catalog();
        let mut record = CarrierProfileRecord::from_profile(&profiles::GB_EE_23433);
        record.schema_version = 0;
        record.meta.profile_id = "schema-version-test".to_string();
        record.meta.mcc = "001".to_string();
        record.meta.mnc = "01".to_string();
        record.meta.mnc_len = 2;
        record.meta.plmn = "00101".to_string();

        let saved = store.upsert(record).expect("save database profile");
        assert_eq!(saved.record.schema_version, CURRENT_SCHEMA_VERSION);
        let rows = store
            .database
            .list_custom_carrier_profiles()
            .expect("read database row");
        let value: serde_json::Value =
            serde_json::from_str(&rows[0].record_json).expect("parse persisted database row");
        assert_eq!(
            value["schema_version"].as_u64(),
            Some(CURRENT_SCHEMA_VERSION as u64)
        );

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn volte_catalog_reference_reports_missing_lte_projection_and_runtime_falls_back() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let (store, path) = store_with_catalog();
        {
            let conn = rusqlite::Connection::open(&path).expect("open catalog fixture");
            conn.execute(
                "UPDATE carrier_profiles SET lte_ims_status = 'partial'
                 WHERE profile_id = 'test-v7-23433'",
                [],
            )
            .expect("remove LTE-ready projection");
        }

        let listed = store
            .list_for_access(CatalogAccessKind::LteEpc)
            .expect("list selectable VoLTE profiles");
        let catalog = listed
            .iter()
            .find(|profile| {
                profile.origin == ProfileOrigin::Catalog && profile.profile_id == "test-v7-23433"
            })
            .expect("Wi-Fi-only catalog row remains visible as a disabled VoLTE choice");
        assert!(!catalog.volte_ready);
        assert!(catalog.vowifi_ready);
        assert_eq!(
            store
                .volte_reference_state(VolteProfileSource::CarrierCatalog, "test-v7-23433",)
                .expect("explicit reference state"),
            VolteProfileReferenceState::NotLteReady
        );
        assert_eq!(
            store
                .volte_reference_state(VolteProfileSource::CarrierCatalog, "missing-profile",)
                .expect("missing explicit reference state"),
            VolteProfileReferenceState::Missing
        );

        let resolved = store
            .resolve_volte_candidate(
                &VolteProfileCandidate {
                    source: VolteProfileSource::CarrierCatalog,
                    profile_id: Some("test-v7-23433".to_string()),
                },
                None,
                "234330123456789",
                Some("23433"),
            )
            .expect("runtime candidate resolution")
            .expect("derived fallback");
        assert_eq!(resolved.origin, ProfileOrigin::Derived);
        assert!(resolved
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("carrier_catalog_profile_not_ready")));

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
