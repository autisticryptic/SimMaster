//! Database-backed VoWiFi carrier profile store.
//!
//! Profiles used to be `static` constants, so adding a carrier meant editing
//! Rust and rebuilding. They now live in SQLite and can be edited, imported and
//! extended at runtime. Resolution order for a given PLMN:
//!
//! 1. **Database** — operator-edited or imported profiles win.
//! 2. **Built-ins** — the shipped, hand-verified profiles.
//! 3. **Derivation** — ePDG/IMS names computed from MCC/MNC per 3GPP TS 23.003.
//!
//! The derivation step is what lets a SIM from a carrier nobody has ever tested
//! still come up: the ePDG FQDN and IMS domain are not carrier secrets, they are
//! a documented function of the IMSI.

use std::sync::Arc;

use super::profile_record::CarrierProfileRecord;
use super::profiles::{self, CarrierProfile, BUILTIN_PROFILES};
use crate::platform::db::Database;

/// Where a resolved profile came from. Surfaced to the UI so an operator can
/// tell a verified profile from a guessed one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileOrigin {
    Database,
    Builtin,
    Derived,
}

impl ProfileOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            ProfileOrigin::Database => "database",
            ProfileOrigin::Builtin => "builtin",
            ProfileOrigin::Derived => "derived",
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
}

impl ProfileStore {
    pub fn new(database: Arc<Database>) -> Self {
        Self { database }
    }

    /// Copy the compiled-in profiles into the database the first time the store
    /// is used. Existing rows are never overwritten, so operator edits survive
    /// upgrades; a built-in that gains a new carrier still gets inserted.
    pub fn seed_builtins(&self) -> Result<usize, String> {
        let mut inserted = 0;
        for profile in BUILTIN_PROFILES {
            let record = CarrierProfileRecord::from_profile(profile);
            let exists = self
                .database
                .get_vowifi_carrier_profile(&record.meta.profile_id)
                .map_err(|error| error.to_string())?
                .is_some();
            if exists {
                continue;
            }
            let json = serde_json::to_string(&record).map_err(|error| error.to_string())?;
            self.database
                .upsert_vowifi_carrier_profile(
                    &record.meta.profile_id,
                    &record.meta.plmn,
                    ProfileOrigin::Builtin.as_str(),
                    &json,
                )
                .map_err(|error| error.to_string())?;
            inserted += 1;
        }
        Ok(inserted)
    }

    /// One-time migration of the legacy `vowifi-profiles.conf` file.
    ///
    /// That file held user-created ePDG overrides, which the profile database
    /// now supersedes. Each entry is expanded into a full profile: start from
    /// the 3GPP-derived defaults for its PLMN, then overlay the fields the file
    /// actually carried. The file is renamed rather than deleted so an operator
    /// can still inspect it if a migration looks wrong.
    pub fn migrate_legacy_profiles_file(&self, path: &std::path::Path) -> Result<usize, String> {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Ok(0);
        };
        let legacy = crate::platform::config::parse_external_vowifi_profiles(&content);
        let mut migrated = 0;
        for entry in legacy {
            let Some(mut record) = super::profile_import::ImportedCarrierFacts {
                mcc: entry.mcc.clone(),
                mnc: entry.mnc.clone(),
                ims_apn: entry.apn.clone(),
                ..Default::default()
            }
            .to_record() else {
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
            None => super::profile_import::ImportedCarrierFacts {
                mcc: entry.mcc.clone(),
                mnc: entry.mnc.clone(),
                ims_apn: entry.apn.clone(),
                ..Default::default()
            }
            .to_record()
            .ok_or_else(|| "vowifi_external_profile_invalid".to_string())?,
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
        let Some(row) = self
            .database
            .get_vowifi_carrier_profile(profile_id)
            .map_err(|error| error.to_string())?
        else {
            return Ok(None);
        };
        serde_json::from_str(&row.payload_json)
            .map(Some)
            .map_err(|error| error.to_string())
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
        match self.list() {
            Ok(stored) => {
                let interned = stored
                    .iter()
                    .filter(|entry| entry.record.validate().is_ok())
                    .map(|entry| entry.record.intern())
                    .collect::<Vec<_>>();
                profiles::publish_database_profiles(&interned);
            }
            Err(error) => {
                tracing::warn!(error = %error, "Failed to publish VoWiFi carrier profiles to the resolver");
            }
        }
    }

    /// Resolve the profile for a PLMN, following database → built-in → derived.
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
        // The database was already consulted above, so fall back without the
        // overlay: re-entering it would consult the same rows twice.
        profiles::resolve_builtin_or_derived_by_plmn(mcc, mnc).map(|profile| ResolvedProfile {
            profile,
            // Derivation tags its generated ids, which is how the two are told
            // apart here.
            origin: if profile.meta.profile_id.starts_with("dynamic_3gpp_") {
                ProfileOrigin::Derived
            } else {
                ProfileOrigin::Builtin
            },
        })
    }

    pub fn resolve_by_profile_id(&self, profile_id: &str) -> Option<ResolvedProfile> {
        if let Ok(Some(record)) = self.get(profile_id) {
            if record.validate().is_ok() {
                return Some(ResolvedProfile {
                    profile: record.intern(),
                    origin: ProfileOrigin::Database,
                });
            }
        }
        profiles::resolve_by_profile_id(profile_id).map(|profile| ResolvedProfile {
            profile,
            origin: if profile.meta.profile_id.starts_with("dynamic_3gpp_") {
                ProfileOrigin::Derived
            } else {
                ProfileOrigin::Builtin
            },
        })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredProfile {
    pub profile_id: String,
    pub plmn: String,
    /// Where the row came from: `builtin`, `manual`, `aosp`, `ipcc`, …
    pub source: String,
    pub updated_at: String,
    pub record: CarrierProfileRecord,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn store() -> ProfileStore {
        let database = Arc::new(Database::new(PathBuf::from(":memory:")).expect("db"));
        ProfileStore::new(database)
    }

    #[test]
    fn seeding_is_idempotent_and_covers_every_builtin() {
        let store = store();
        let first = store.seed_builtins().expect("seed");
        assert_eq!(first, BUILTIN_PROFILES.len());
        let second = store.seed_builtins().expect("reseed");
        assert_eq!(second, 0, "existing rows must not be rewritten");
        assert_eq!(store.list().expect("list").len(), BUILTIN_PROFILES.len());
    }

    #[test]
    fn database_profile_wins_over_the_builtin() {
        let store = store();
        store.seed_builtins().expect("seed");

        let baseline = store
            .resolve_by_plmn("234", "33")
            .expect("EE resolves from the seeded database");
        assert_eq!(baseline.origin, ProfileOrigin::Database);

        let mut record = store
            .get("gb_ee_23433")
            .expect("read")
            .expect("seeded row present");
        record.epdg.host = "epdg.example.test".to_string();
        store.save(&record, "manual").expect("save edit");

        let resolved = store.resolve_by_plmn("234", "33").expect("resolve");
        assert_eq!(resolved.origin, ProfileOrigin::Database);
        assert_eq!(resolved.profile.epdg.host, "epdg.example.test");
    }

    #[test]
    fn unknown_carrier_falls_back_to_derivation() {
        let store = store();
        store.seed_builtins().expect("seed");
        // 46001 (China Unicom) has no builtin profile.
        let resolved = store.resolve_by_plmn("460", "01").expect("resolve");
        assert_eq!(resolved.origin, ProfileOrigin::Derived);
        assert_eq!(
            resolved.profile.epdg.host,
            "epdg.epc.mnc001.mcc460.pub.3gppnetwork.org"
        );
        assert_eq!(
            resolved.profile.ims.domain,
            "ims.mnc001.mcc460.3gppnetwork.org"
        );
    }

    #[test]
    fn legacy_profiles_file_is_folded_into_the_database_and_archived() {
        let store = store();
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
      "profile_id": "my_unicom",
      "mcc": "460",
      "mnc": "01",
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

        // The override survived, and the rest of the profile came from derivation.
        let resolved = store.resolve_by_plmn("460", "01").expect("resolve");
        assert_eq!(resolved.origin, ProfileOrigin::Database);
        assert_eq!(resolved.profile.epdg.host, "epdg.custom.test");
        assert_eq!(resolved.profile.epdg.port, 4500);
        assert_eq!(resolved.profile.epdg.ip_stack, "ipv4");
        assert_eq!(resolved.profile.epdg.apn, Some("cmims"));
        assert_eq!(resolved.profile.epdg.dns_servers, &["8.8.8.8"]);
        assert_eq!(
            resolved.profile.ims.domain, "ims.mnc001.mcc460.3gppnetwork.org",
            "fields the file never carried still come from 3GPP derivation"
        );

        // The file is archived, so a restart does not migrate it twice.
        assert!(!path.exists());
        let archived = path.with_extension("conf.migrated");
        assert!(archived.exists());
        assert_eq!(store.migrate_legacy_profiles_file(&path).expect("rerun"), 0);

        let _ = std::fs::remove_file(archived);
    }

    /// The API returning an edited profile is not enough — the live matcher
    /// (`profiles::resolve_by_imsi`, used from modules with no database handle)
    /// has to see it too, otherwise an edit silently does nothing at connect time.
    #[test]
    fn database_edits_reach_the_live_imsi_matcher() {
        // Deliberately not seeded: the override table is process-global, so this
        // test publishes only its own row and clears it again at the end.
        let store = store();

        // 46001 has no builtin profile; after publishing it must win over the
        // derived answer for an IMSI carrying that PLMN.
        let mut record = super::super::profile_import::ImportedCarrierFacts {
            mcc: "460".to_string(),
            mnc: "01".to_string(),
            ..Default::default()
        }
        .to_record()
        .expect("derived record");
        record.meta.profile_id = "unicom_live_test".to_string();
        record.epdg.host = "epdg.live.test".to_string();
        store.save(&record, "manual").expect("save");

        let matched = profiles::resolve_by_imsi("460010123456789").expect("imsi match");
        assert_eq!(matched.profile.epdg.host, "epdg.live.test");
        assert_eq!(matched.matched_prefix, "46001");

        // Deleting it puts the derived answer back.
        assert!(store.delete("unicom_live_test").expect("delete"));
        let matched = profiles::resolve_by_imsi("460010123456789").expect("imsi match");
        assert_eq!(
            matched.profile.epdg.host,
            "epdg.epc.mnc001.mcc460.pub.3gppnetwork.org"
        );

        // Leave the global override table empty so other tests are unaffected.
        profiles::publish_database_profiles(&[]);
    }

    #[test]
    fn external_shaped_edit_preserves_the_tuned_register_policy() {
        let store = store();
        store.seed_builtins().expect("seed");
        // Tune something the legacy shape cannot express.
        let mut record = store.get("gb_ee_23433").expect("read").expect("present");
        record.ims.register.sec_agree_mode = "required".to_string();
        store.save(&record, "manual").expect("save");

        store
            .save_external(&crate::platform::config::ExternalVowifiProfile {
                profile_id: "gb_ee_23433".to_string(),
                mcc: "234".to_string(),
                mnc: "33".to_string(),
                epdg_host: "epdg.new.test".to_string(),
                epdg_port: 4500,
                ip_stack: "ipv4v6".to_string(),
                apn: Some("ims".to_string()),
                dns_server: Some("1.1.1.1".to_string()),
            })
            .expect("save external");

        let updated = store.get("gb_ee_23433").expect("read").expect("present");
        assert_eq!(updated.epdg.host, "epdg.new.test");
        assert_eq!(
            updated.ims.register.sec_agree_mode, "required",
            "an ePDG-only edit must not reset the REGISTER policy"
        );
    }

    #[test]
    fn invalid_records_are_rejected_before_they_reach_the_database() {
        let store = store();
        let mut record = CarrierProfileRecord::from_profile(&profiles::GB_EE_23433);
        record.meta.plmn = "00000".to_string();
        let error = store.save(&record, "manual").unwrap_err();
        assert_eq!(error, "plmn_mismatch");
        assert!(store.list().expect("list").is_empty());
    }

    #[test]
    fn deleting_a_row_restores_the_builtin_answer() {
        let store = store();
        store.seed_builtins().expect("seed");
        let mut record = store.get("gb_ee_23433").expect("read").expect("present");
        record.epdg.host = "epdg.override.test".to_string();
        store.save(&record, "manual").expect("save");
        assert_eq!(
            store
                .resolve_by_plmn("234", "33")
                .unwrap()
                .profile
                .epdg
                .host,
            "epdg.override.test"
        );

        assert!(store.delete("gb_ee_23433").expect("delete"));
        let resolved = store.resolve_by_plmn("234", "33").expect("resolve");
        assert_eq!(resolved.origin, ProfileOrigin::Builtin);
        assert_eq!(
            resolved.profile.epdg.host,
            profiles::GB_EE_23433.epdg.host,
            "removing the row must fall back to the compiled-in profile"
        );
    }
}
