//! Schema-v7 reader for the compiled `config_json` carrier catalog.
//!
//! v7 deliberately has its own query path. Its readiness flags and protocol
//! baseline replace the NULL/inheritance rules used by the normalized v5/v6
//! schema, so sharing deep-table SQL between the versions would change the
//! meaning of missing values.

use std::collections::HashSet;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use super::{
    db_error, default_access_network_info, expand_ims_static_template, expand_static_template,
    normalize_home_plmn, normalize_ip_family, CatalogAccessKind, CatalogIdentityMatch,
    CatalogProfile, CatalogRelease, ProfileMetaRow,
};
use crate::connectivity::modems::ims::vowifi::profile_record::{
    CarrierProfileMetaRecord, CarrierProfileRecord, E911PolicyRecord, EpdgPolicyRecord,
    Ikev2PolicyRecord, ImsPolicyRecord, ProfileIdentityPolicyRecord, RegisterPolicyRecord,
    SmsPolicyRecord, UtPolicyRecord, VoiceCodecPolicyRecord, VoicePolicyRecord,
};
use crate::connectivity::modems::ims::vowifi::profiles;

const PROTOCOL_BASELINE: &str = "carrier-bundles-ims-v1";
const BASELINE_IKE_PROPOSALS: &[&str] = &[
    "aes128-sha256-modp2048",
    "aes128-sha1-modp2048",
    "aes128-sha256-modp1024",
];
const BASELINE_ESP_PROPOSALS: &[&str] = &["aes128-sha256", "aes128-sha1"];
const BASELINE_SECURITY_CLIENT: &str = "hmac-sha-1-96/aes-cbc/esp/trans";
const DEFAULT_VOICE_CODECS: &[&str] = &["amr-wb", "amr", "pcmu", "pcma"];
const EVS_BIT_RATES: &[&str] = &[
    "5.9", "7.2", "8", "9.6", "13.2", "16.4", "24.4", "32", "48", "64", "96", "128",
];
const EVS_BANDWIDTHS: &[&str] = &["nb", "wb", "swb", "fb", "nb-wb", "nb-swb", "nb-fb"];

pub(super) fn validate_schema(conn: &Connection) -> Result<(), String> {
    for table in [
        "catalog_metadata",
        "visual_assets",
        "carriers",
        "carrier_profiles",
        "profile_match_rules",
        "source_artifacts",
        "profile_sources",
        "field_evidence",
    ] {
        let exists = conn
            .query_row(
                "SELECT EXISTS (
                     SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = ?1
                 )",
                [table],
                |row| row.get::<_, bool>(0),
            )
            .map_err(db_error)?;
        if !exists {
            return Err(format!("carrier_catalog_v7_table_missing:{table}"));
        }
    }
    let metadata_version = conn
        .query_row(
            "SELECT CAST(schema_version AS INTEGER) FROM catalog_metadata LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(db_error)?;
    let metadata_rows = conn
        .query_row("SELECT COUNT(*) FROM catalog_metadata", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(db_error)?;
    if metadata_rows != 1 {
        return Err(format!(
            "carrier_catalog_v7_metadata_cardinality:{metadata_rows}"
        ));
    }
    if metadata_version != 7 {
        return Err(format!(
            "carrier_catalog_schema_version_mismatch:7:{metadata_version}"
        ));
    }
    let config_contract = conn
        .query_row(
            "SELECT config_contract FROM catalog_metadata LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .map_err(db_error)?;
    if config_contract != PROTOCOL_BASELINE {
        return Err(format!(
            "carrier_catalog_config_contract_unsupported:{config_contract}"
        ));
    }
    require_columns(
        conn,
        "carrier_profiles",
        &[
            "profile_id",
            "carrier_id",
            "display_name",
            "priority",
            "confidence",
            "lte_ims_status",
            "vowifi_status",
            "config_json",
        ],
    )?;
    require_columns(
        conn,
        "profile_match_rules",
        &[
            "profile_id",
            "priority",
            "plmn",
            "imsi_prefix",
            "iccid_prefix",
            "gid1",
            "gid2",
            "spn",
            "is_exclusion",
        ],
    )?;
    require_columns(
        conn,
        "carriers",
        &[
            "canonical_name",
            "brand_name",
            "country_iso2",
            "aliases_json",
        ],
    )?;
    read_release(conn).map(|_| ())
}

fn require_columns(conn: &Connection, table: &str, required: &[&str]) -> Result<(), String> {
    let mut stmt = conn
        .prepare("SELECT name FROM pragma_table_info(?1)")
        .map_err(db_error)?;
    let columns = stmt
        .query_map([table], |row| row.get::<_, String>(0))
        .map_err(db_error)?
        .collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(db_error)?;
    for column in required {
        if !columns.contains(*column) {
            return Err(format!(
                "carrier_catalog_v7_column_missing:{table}:{column}"
            ));
        }
    }
    Ok(())
}

pub(super) fn read_release(conn: &Connection) -> Result<CatalogRelease, String> {
    conn.query_row(
        "SELECT release_id, generated_at, sealed FROM catalog_metadata LIMIT 1",
        [],
        |row| {
            Ok(CatalogRelease {
                release_id: row.get(0)?,
                generated_at: row.get(1)?,
                sealed: row.get::<_, i64>(2)? != 0,
            })
        },
    )
    .map_err(db_error)
}

pub(super) fn list(
    conn: &Connection,
    access: CatalogAccessKind,
) -> Result<Vec<CatalogProfile>, String> {
    let release = read_release(conn)?;
    let sql = format!(
        "SELECT profile_id FROM carrier_profiles
         WHERE {} = 'ready' ORDER BY priority, profile_id",
        access.v7_status_column()
    );
    let mut stmt = conn.prepare(&sql).map_err(db_error)?;
    let profile_ids = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let mut profiles = Vec::with_capacity(profile_ids.len());
    for profile_id in profile_ids {
        match load_profile(conn, &profile_id, None, access, &release) {
            Ok(profile) => profiles.push(profile),
            Err(error) => tracing::warn!(
                profile_id,
                access_kind = access.as_str(),
                error = %error,
                "Skipping incomplete schema-v7 carrier catalog profile"
            ),
        }
    }
    Ok(profiles)
}

pub(super) fn public_identity_matches(
    conn: &Connection,
    access: CatalogAccessKind,
) -> Result<Vec<CatalogIdentityMatch>, String> {
    let release = read_release(conn)?;
    let sql = format!(
        "SELECT cp.profile_id, mr.plmn, mr.imsi_prefix
         FROM carrier_profiles AS cp
         JOIN profile_match_rules AS mr ON mr.profile_id = cp.profile_id
         WHERE cp.{} = 'ready'
           AND mr.is_exclusion = 0 AND mr.plmn IS NOT NULL
           AND mr.iccid_prefix IS NULL
           AND mr.gid1 IS NULL AND mr.gid2 IS NULL AND mr.spn IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM profile_match_rules AS ex
               WHERE ex.profile_id = cp.profile_id AND ex.is_exclusion = 1
           )
         ORDER BY length(COALESCE(mr.imsi_prefix, mr.plmn)) DESC,
                  mr.priority, cp.priority, cp.confidence DESC, cp.profile_id",
        access.v7_status_column()
    );
    let mut stmt = conn.prepare(&sql).map_err(db_error)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    let mut matches = Vec::new();
    let mut seen = HashSet::new();
    for (profile_id, plmn, imsi_prefix) in rows {
        let match_prefix = match imsi_prefix {
            Some(prefix) if prefix.starts_with(&plmn) => prefix,
            Some(prefix) if plmn.starts_with(&prefix) => plmn.clone(),
            Some(_) => continue,
            None => plmn.clone(),
        };
        if !seen.insert((profile_id.clone(), match_prefix.clone())) {
            continue;
        }
        match load_profile(conn, &profile_id, Some(&plmn), access, &release) {
            Ok(profile) => matches.push(CatalogIdentityMatch {
                profile,
                match_prefix,
            }),
            Err(error) => tracing::warn!(
                profile_id,
                access_kind = access.as_str(),
                error = %error,
                "Skipping incomplete schema-v7 carrier catalog match"
            ),
        }
    }
    Ok(matches)
}

pub(super) fn get(
    conn: &Connection,
    profile_id: &str,
    access: CatalogAccessKind,
) -> Result<Option<CatalogProfile>, String> {
    let sql = format!(
        "SELECT {} FROM carrier_profiles WHERE profile_id = ?1",
        access.v7_status_column()
    );
    let status = conn
        .query_row(&sql, [profile_id], |row| row.get::<_, String>(0))
        .optional()
        .map_err(db_error)?;
    let Some(status) = status else {
        return Ok(None);
    };
    if status != "ready" {
        return Err(format!(
            "carrier_catalog_profile_not_ready:{profile_id}:{}:{status}",
            access.as_str()
        ));
    }
    load_profile(conn, profile_id, None, access, &read_release(conn)?).map(Some)
}

pub(super) fn imsi_has_ambiguous_plmn(conn: &Connection, imsi: &str) -> Result<bool, String> {
    if imsi.len() < 6 {
        return Ok(false);
    }
    conn.query_row(
        "SELECT EXISTS (
             SELECT 1 FROM profile_match_rules AS short
             JOIN profile_match_rules AS long
               ON long.plmn = substr(?1, 1, 6)
             WHERE short.plmn = substr(?1, 1, 5)
               AND short.is_exclusion = 0 AND long.is_exclusion = 0
         )",
        [imsi],
        |row| row.get::<_, bool>(0),
    )
    .map_err(db_error)
}

pub(super) fn ambiguous_plmn_prefixes(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT short.plmn
             FROM profile_match_rules AS short
             JOIN profile_match_rules AS long
               ON substr(long.plmn, 1, 5) = short.plmn
             WHERE short.is_exclusion = 0 AND long.is_exclusion = 0
               AND length(short.plmn) = 5 AND length(long.plmn) = 6
             ORDER BY short.plmn",
        )
        .map_err(db_error)?;
    let prefixes = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(db_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(db_error)?;
    Ok(prefixes)
}

pub(super) fn resolve_for_imsi(
    conn: &Connection,
    imsi: &str,
    home_plmn: Option<&str>,
    access: CatalogAccessKind,
) -> Result<Option<CatalogProfile>, String> {
    let home_plmn = normalize_home_plmn(imsi, home_plmn);
    if home_plmn.is_none() && imsi_has_ambiguous_plmn(conn, imsi)? {
        return Ok(None);
    }
    let sql = format!(
        "SELECT cp.profile_id, mr.plmn
         FROM carrier_profiles AS cp
         JOIN profile_match_rules AS mr ON mr.profile_id = cp.profile_id
         WHERE cp.{} = 'ready' AND mr.is_exclusion = 0
           AND (mr.plmn IS NULL OR ?1 LIKE mr.plmn || '%')
           AND (mr.imsi_prefix IS NULL OR ?1 LIKE mr.imsi_prefix || '%')
           AND (?2 IS NULL OR mr.plmn IS NULL OR mr.plmn = ?2)
           AND mr.iccid_prefix IS NULL
           AND mr.gid1 IS NULL AND mr.gid2 IS NULL AND mr.spn IS NULL
           AND NOT EXISTS (
               SELECT 1 FROM profile_match_rules AS ex
               WHERE ex.profile_id = cp.profile_id AND ex.is_exclusion = 1
                 AND ex.iccid_prefix IS NULL
                 AND ex.gid1 IS NULL AND ex.gid2 IS NULL AND ex.spn IS NULL
                 AND (ex.plmn IS NULL OR ?1 LIKE ex.plmn || '%')
                 AND (ex.imsi_prefix IS NULL OR ?1 LIKE ex.imsi_prefix || '%')
                 AND (?2 IS NULL OR ex.plmn IS NULL OR ex.plmn = ?2)
           )
         ORDER BY mr.priority,
                  length(COALESCE(mr.imsi_prefix, mr.plmn, '')) DESC,
                  cp.priority, cp.confidence DESC, cp.profile_id
         LIMIT 1",
        access.v7_status_column()
    );
    let matched = conn
        .query_row(&sql, params![imsi, home_plmn], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })
        .optional()
        .map_err(db_error)?;
    let Some((profile_id, matched_plmn)) = matched else {
        return Ok(None);
    };
    load_profile(
        conn,
        &profile_id,
        matched_plmn.as_deref(),
        access,
        &read_release(conn)?,
    )
    .map(Some)
}

fn load_profile(
    conn: &Connection,
    profile_id: &str,
    matched_plmn: Option<&str>,
    access: CatalogAccessKind,
    release: &CatalogRelease,
) -> Result<CatalogProfile, String> {
    let sql = format!(
        "SELECT cp.display_name,
                COALESCE(c.brand_name, c.canonical_name, cp.display_name),
                COALESCE(c.country_iso2, ''), COALESCE(c.aliases_json, '[]'),
                mr.plmn, cp.config_json, cp.{}
         FROM carrier_profiles AS cp
         LEFT JOIN carriers AS c ON c.carrier_id = cp.carrier_id
         JOIN profile_match_rules AS mr
           ON mr.profile_id = cp.profile_id AND mr.is_exclusion = 0
          AND mr.plmn IS NOT NULL
         WHERE cp.profile_id = ?1
         ORDER BY CASE WHEN ?2 IS NOT NULL AND mr.plmn = ?2 THEN 0 ELSE 1 END,
                  mr.priority, length(mr.plmn) DESC LIMIT 1",
        access.v7_status_column()
    );
    let row = conn
        .query_row(&sql, params![profile_id, matched_plmn], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })
        .map_err(db_error)?;
    if row.6 != "ready" {
        return Err(format!(
            "carrier_catalog_profile_not_ready:{profile_id}:{}:{}",
            access.as_str(),
            row.6
        ));
    }
    let config: Value = serde_json::from_str(&row.5)
        .map_err(|error| format!("carrier_catalog_v7_config_invalid:{profile_id}:{error}"))?;
    validate_readiness(profile_id, access, &config)?;
    let baseline = string_at(&config, "/protocol_baseline")
        .ok_or_else(|| v7_required(profile_id, access, "/protocol_baseline"))?;
    if baseline != PROTOCOL_BASELINE {
        return Err(format!(
            "carrier_catalog_protocol_baseline_unsupported:{profile_id}:{baseline}"
        ));
    }

    let aliases = serde_json::from_str::<Vec<String>>(&row.3)
        .map_err(|error| format!("carrier_catalog_v7_aliases_invalid:{profile_id}:{error}"))?;
    let meta = profile_meta(&row.0, &row.1, &row.2, &row.4)?;
    let record = project_config(profile_id, &meta, aliases, release, access, &config)?;
    Ok(CatalogProfile {
        record,
        release: release.clone(),
    })
}

fn profile_meta(
    profile_name: &str,
    brand: &str,
    country_iso2: &str,
    plmn: &str,
) -> Result<ProfileMetaRow, String> {
    if !matches!(plmn.len(), 5 | 6) || !plmn.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("carrier_catalog_v7_plmn_invalid:{plmn}"));
    }
    Ok(ProfileMetaRow {
        profile_name: profile_name.to_string(),
        brand: brand.to_string(),
        legal_name: brand.to_string(),
        country_iso2: country_iso2.to_ascii_lowercase(),
        plmn: plmn.to_string(),
        mcc: plmn[..3].to_string(),
        mnc: plmn[3..].to_string(),
        mnc_length: (plmn.len() - 3) as u8,
    })
}

fn validate_readiness(
    profile_id: &str,
    access: CatalogAccessKind,
    config: &Value,
) -> Result<(), String> {
    let path = format!("/readiness/{}_missing", access.v7_config_key());
    let Some(value) = config.pointer(&path) else {
        return Ok(());
    };
    let missing = value.as_array().ok_or_else(|| {
        format!(
            "carrier_catalog_v7_readiness_invalid:{profile_id}:{}",
            access.as_str()
        )
    })?;
    if missing.is_empty() {
        return Ok(());
    }
    let fields = missing
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(",");
    Err(format!(
        "carrier_catalog_profile_not_ready:{profile_id}:{}:missing:{fields}",
        access.as_str()
    ))
}

fn project_config(
    profile_id: &str,
    meta: &ProfileMetaRow,
    aliases: Vec<String>,
    release: &CatalogRelease,
    access: CatalogAccessKind,
    config: &Value,
) -> Result<CarrierProfileRecord, String> {
    let standard_domain = format!("ims.mnc{:0>3}.mcc{}.3gppnetwork.org", meta.mnc, meta.mcc);
    let domain = string_at(config, "/ims/home_domain")
        .map(|value| expand_static_template(value, meta, "ims_home_domain"))
        .transpose()?
        .unwrap_or(standard_domain);
    let realm = string_at(config, "/ims/realm")
        .map(|value| expand_ims_static_template(value, meta, &domain, "ims_realm"))
        .transpose()?
        .unwrap_or_else(|| domain.clone());
    let authentication = string_at(config, "/ims/authentication/scheme").unwrap_or("ims_aka");
    if authentication != "ims_aka" {
        return Err(format!(
            "carrier_catalog_ims_authentication_unsupported:{profile_id}:{authentication}"
        ));
    }
    if let Some(algorithm) = string_at(config, "/ims/authentication/algorithm") {
        let algorithm = algorithm.trim();
        if !algorithm.is_empty()
            && !algorithm.eq_ignore_ascii_case("AKAv1-MD5")
            && !algorithm.eq_ignore_ascii_case("AKAv2-MD5")
        {
            return Err(format!(
                "carrier_catalog_ims_aka_algorithm_unsupported:{profile_id}:{algorithm}"
            ));
        }
    }
    let identity_source = config
        .pointer("/ims/identity_templates")
        .and_then(Value::as_array)
        .and_then(|templates| {
            templates.iter().find_map(|template| {
                (string_at(template, "/role") == Some("impi"))
                    .then(|| string_at(template, "/source"))
                    .flatten()
            })
        })
        .unwrap_or("derived_imsi")
        .to_string();

    let access_path = format!("/access/{}", access.v7_config_key());
    let access_config = config.pointer(&access_path).unwrap_or(&Value::Null);
    let apn = string_at(access_config, "/apn")
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("ims")
        .to_string();
    let ip_family = normalize_ip_family(string_at(access_config, "/ip_family").unwrap_or("ipv4v6"));

    let (epdg, ikev2) = match access {
        CatalogAccessKind::LteEpc => empty_non_wifi_access(&apn, &ip_family),
        CatalogAccessKind::WifiEpdg => {
            project_vowifi_access(profile_id, meta, access_config, &apn, &ip_family)?
        }
    };
    let register = project_register(profile_id, meta, access, config, &domain)?;
    let transport = match string_at(config, "/ims/transport")
        .or_else(|| string_at(config, "/sip/common/transport"))
        .unwrap_or("udp")
    {
        "udp" | "auto" => "udp".to_string(),
        "tcp" => "tcp".to_string(),
        unsupported => {
            return Err(format!(
                "carrier_catalog_ims_transport_unsupported:{profile_id}:{}:{unsupported}",
                access.as_str()
            ))
        }
    };
    let registrar = expanded_optional(config, "/sip/common/registrar", meta, &domain)?;
    let pcscf = expanded_optional(access_config, "/pcscf", meta, &domain)?;
    let vowifi_enabled =
        bool_at(config, "/services/vowifi").unwrap_or(access == CatalogAccessKind::WifiEpdg);
    let e911_enabled = bool_at(config, "/services/emergency").unwrap_or(false);
    let codec_policies = project_voice_codec_policies(profile_id, config)?;
    let preferred_codecs = if codec_policies.is_empty() {
        string_array_at(config, "/services/voice_codecs")
            .unwrap_or_default()
            .into_iter()
            .filter_map(|codec| normalize_voice_codec(&codec).map(str::to_string))
            .collect::<Vec<_>>()
    } else {
        codec_policies
            .iter()
            .map(|policy| policy.codec.clone())
            .collect()
    };
    let preferred_codecs = if preferred_codecs.is_empty() {
        DEFAULT_VOICE_CODECS
            .iter()
            .map(|codec| (*codec).to_string())
            .collect()
    } else {
        preferred_codecs
    };

    let record = CarrierProfileRecord {
        meta: CarrierProfileMetaRecord {
            profile_id: profile_id.to_string(),
            mcc: meta.mcc.clone(),
            mnc: meta.mnc.clone(),
            mnc_len: meta.mnc_length,
            plmn: meta.plmn.clone(),
            country_iso2: meta.country_iso2.clone(),
            brand: if meta.brand.is_empty() {
                meta.profile_name.clone()
            } else {
                meta.brand.clone()
            },
            operator_legal_name: meta.legal_name.clone(),
            aliases,
            source_refs: vec![format!("carrier_catalog:{}", release.release_id)],
            last_verified: release.generated_at.chars().take(10).collect(),
        },
        identity: ProfileIdentityPolicyRecord {
            device_model_hint: String::new(),
            spoof_imei: false,
            device_identity_enabled: false,
            device_identity_imei: None,
        },
        epdg,
        ikev2,
        ims: ImsPolicyRecord {
            domain,
            realm,
            registrar,
            pcscf,
            transport,
            local_port: u16_at(config, "/ims/local_port").unwrap_or(5060),
            user_agent: string_at(config, "/sip/common/register/user_agent")
                .unwrap_or("SimAdmin IMS")
                .to_string(),
            identity_source,
            tcp_keepalive_seconds: u16_at(config, "/sip/common/tcp_keepalive_seconds")
                .unwrap_or(profiles::DEFAULT_IMS_TCP_KEEPALIVE_SECONDS),
            options_ping_interval_seconds: u16_at(
                config,
                "/sip/common/options_ping_interval_seconds",
            )
            .unwrap_or(profiles::DEFAULT_IMS_OPTIONS_PING_INTERVAL_SECONDS),
            register,
        },
        sms: SmsPolicyRecord {
            receiver_transport: string_at(config, "/services/smsoip_transport")
                .unwrap_or("tcp")
                .to_string(),
            smsc_auth_required: bool_at(config, "/services/smsoip_auth_required").unwrap_or(false),
        },
        voice: VoicePolicyRecord {
            vowifi_enabled,
            carrier_fallback_enabled: false,
            preferred_codecs,
            codec_policies,
            amr_octet_align: bool_at(config, "/services/amr_octet_align").unwrap_or(false),
            ptime_ms: u16_at(config, "/services/ptime_ms").unwrap_or(20),
            sip_endpoint_exposed: false,
            voicemail_number: string_at(config, "/services/voicemail_number").map(str::to_string),
        },
        // TODO(E911): catalog metadata is exposed, but SimAdmin still has no
        // address-provisioning execution path.
        e911: E911PolicyRecord {
            enabled: e911_enabled,
            provider: string_at(config, "/services/e911/provider").map(str::to_string),
            entitlement_url: string_at(config, "/services/e911/entitlement_url")
                .map(str::to_string),
            websheet_host_policy: None,
        },
        ut: project_ut_policy(config),
    };
    match access {
        CatalogAccessKind::LteEpc => record.validate_ims_only()?,
        CatalogAccessKind::WifiEpdg => record.validate()?,
    }
    Ok(record)
}

/// UT is unavailable unless the carrier catalog explicitly provides a complete
/// XCAP endpoint. This keeps generic catalog entries from accidentally sending
/// XCAP traffic over the host's default internet route.
fn project_ut_policy(config: &Value) -> UtPolicyRecord {
    let enabled = bool_at(config, "/services/ut/enabled").unwrap_or(false);
    UtPolicyRecord {
        enabled,
        xcap_root: string_at(config, "/services/ut/xcap/root").map(str::to_string),
        document_selector: string_at(config, "/services/ut/xcap/document_selector")
            .map(str::to_string),
        namespace: string_at(config, "/services/ut/xcap/namespace").map(str::to_string),
        authentication: string_at(config, "/services/ut/xcap/authentication")
            .unwrap_or("none")
            .to_string(),
        partial_update: bool_at(config, "/services/ut/xcap/partial_update/enabled")
            .unwrap_or(false),
        call_waiting_selector: string_at(
            config,
            "/services/ut/xcap/partial_update/call_waiting_selector",
        )
        .map(str::to_string),
        diversion_rule_selector: string_at(
            config,
            "/services/ut/xcap/partial_update/diversion_rule_selector",
        )
        .map(str::to_string),
        oip_selector: string_at(config, "/services/ut/xcap/partial_update/oip_selector")
            .map(str::to_string),
        oir_selector: string_at(config, "/services/ut/xcap/partial_update/oir_selector")
            .map(str::to_string),
        tls_min_version: string_at(config, "/services/ut/xcap/tls/min_version")
            .unwrap_or("1.2")
            .to_string(),
        tls_max_version: string_at(config, "/services/ut/xcap/tls/max_version")
            .unwrap_or("1.3")
            .to_string(),
        tls_builtin_roots: bool_at(config, "/services/ut/xcap/tls/builtin_roots").unwrap_or(true),
        tls_additional_ca_pem: string_at(config, "/services/ut/xcap/tls/additional_ca_pem")
            .map(str::to_string),
    }
}

/// Project the schema-v7 media codec list into the signaling policy consumed by
/// both VoLTE and VoWiFi. Unknown future codecs are ignored; recognized codecs
/// are validated so a malformed ready profile cannot advertise invalid RTP.
fn project_voice_codec_policies(
    profile_id: &str,
    config: &Value,
) -> Result<Vec<VoiceCodecPolicyRecord>, String> {
    let Some(codecs) = config
        .pointer("/media/audio/codecs")
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };

    let mut policies = Vec::new();
    for value in codecs {
        let Some(codec) = string_at(value, "/name").and_then(normalize_voice_codec) else {
            continue;
        };
        let payload_type = value
            .pointer("/payload_type")
            .and_then(Value::as_u64)
            .map(|payload_type| {
                u8::try_from(payload_type)
                    .ok()
                    .filter(|payload_type| *payload_type <= 127)
                    .ok_or_else(|| {
                        format!(
                            "carrier_catalog_voice_payload_type_invalid:{profile_id}:{payload_type}"
                        )
                    })
            })
            .transpose()?;
        // Apple bundles use zero as an "unspecified" sentinel on a small set
        // of AMR records. RTP never has a zero clock, so let the codec's
        // registered rate supply the default instead of rejecting the profile.
        let sample_rate = u32_at(value, "/sample_rate").filter(|sample_rate| *sample_rate != 0);
        let expected_sample_rate = match codec {
            "amr" | "pcmu" | "pcma" => 8000,
            "evs" | "amr-wb" => 16000,
            _ => unreachable!("codec is normalized above"),
        };
        if sample_rate.is_some_and(|sample_rate| sample_rate != expected_sample_rate) {
            return Err(format!(
                "carrier_catalog_voice_sample_rate_invalid:{profile_id}:{codec}:{}",
                sample_rate.unwrap_or_default()
            ));
        }

        let fmtp = if codec == "evs" {
            let mut parameters = Vec::new();
            if let Some(bit_rate) = string_at(value, "/bitrate") {
                validate_evs_bit_rate(profile_id, bit_rate)?;
                parameters.push(format!("br={bit_rate}"));
            }
            if let Some(bandwidth) = string_at(value, "/bandwidth") {
                if !EVS_BANDWIDTHS.contains(&bandwidth) {
                    return Err(format!(
                        "carrier_catalog_evs_bandwidth_invalid:{profile_id}:{bandwidth}"
                    ));
                }
                parameters.push(format!("bw={bandwidth}"));
            }
            (!parameters.is_empty()).then(|| parameters.join("; "))
        } else {
            None
        };
        policies.push(VoiceCodecPolicyRecord {
            codec: codec.to_string(),
            payload_type,
            sample_rate,
            fmtp,
        });
    }
    Ok(policies)
}

fn normalize_voice_codec(codec: &str) -> Option<&'static str> {
    match codec.trim().to_ascii_lowercase().as_str() {
        "evs" => Some("evs"),
        "amr" => Some("amr"),
        "amr-wb" | "amr_wb" | "amrwb" => Some("amr-wb"),
        "pcmu" | "g711u" | "g.711u" => Some("pcmu"),
        "pcma" | "g711a" | "g.711a" => Some("pcma"),
        _ => None,
    }
}

fn validate_evs_bit_rate(profile_id: &str, value: &str) -> Result<(), String> {
    let valid = if let Some((lower, upper)) = value.split_once('-') {
        match (
            EVS_BIT_RATES
                .iter()
                .position(|candidate| *candidate == lower),
            EVS_BIT_RATES
                .iter()
                .position(|candidate| *candidate == upper),
        ) {
            (Some(lower), Some(upper)) => lower < upper,
            _ => false,
        }
    } else {
        EVS_BIT_RATES.contains(&value)
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "carrier_catalog_evs_bit_rate_invalid:{profile_id}:{value}"
        ))
    }
}

fn empty_non_wifi_access(apn: &str, ip_family: &str) -> (EpdgPolicyRecord, Ikev2PolicyRecord) {
    (
        EpdgPolicyRecord {
            host: String::new(),
            port: 0,
            apn: Some(apn.to_string()),
            ip_stack: ip_family.to_string(),
            dns_server: None,
            dns_servers: Vec::new(),
        },
        Ikev2PolicyRecord {
            nat_keepalive_seconds: 0,
            dpd_interval_seconds: 0,
            reauth_interval_seconds: None,
            ike_proposals: Vec::new(),
            esp_proposals: Vec::new(),
            aka_challenge_mode: String::new(),
            include_epdg_idr: false,
        },
    )
}

fn project_vowifi_access(
    profile_id: &str,
    meta: &ProfileMetaRow,
    access: &Value,
    apn: &str,
    ip_family: &str,
) -> Result<(EpdgPolicyRecord, Ikev2PolicyRecord), String> {
    let epdg = access
        .pointer("/epdg")
        .and_then(Value::as_array)
        .and_then(|values| values.first());
    // Last-resort ePDG endpoint: when the catalog carries no ePDG entry (or no
    // address), derive the TS 24.302 default FQDN from the profile PLMN, e.g.
    // epdg.epc.mnc012.mcc502.pub.3gppnetwork.org. The runtime DNS resolution
    // still decides whether the operator actually publishes the name.
    let (host, epdg_port) = match epdg {
        Some(entry) => match string_at(entry, "/address") {
            Some(address) => (
                expand_static_template(address, meta, "epdg_endpoint")?,
                u16_at(entry, "/port"),
            ),
            None => (derived_epdg_fqdn(meta), u16_at(entry, "/port")),
        },
        None => (derived_epdg_fqdn(meta), None),
    };
    let ike = access.pointer("/ike").unwrap_or(&Value::Null);
    let dns_servers = string_array_at(access, "/dns_servers").unwrap_or_default();
    let baseline_ike = || {
        BASELINE_IKE_PROPOSALS
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>()
    };
    let ike_proposals = if let Some(values) =
        string_array_at(ike, "/ike_proposals").filter(|values| !values.is_empty())
    {
        values
    } else if let Some(proposals) = ike
        .pointer("/ike_sa_proposals")
        .and_then(Value::as_array)
        .filter(|proposals| !proposals.is_empty())
    {
        let structured = structured_ike_proposals(proposals, profile_id);
        if structured.is_empty() {
            baseline_ike()
        } else {
            structured
        }
    } else {
        baseline_ike()
    };
    let baseline_esp = || {
        BASELINE_ESP_PROPOSALS
            .iter()
            .map(|value| (*value).to_string())
            .collect::<Vec<_>>()
    };
    let esp_proposals = if let Some(values) =
        string_array_at(ike, "/esp_proposals").filter(|values| !values.is_empty())
    {
        values
    } else if let Some(proposals) = ike
        .pointer("/child_sa_proposals")
        .and_then(Value::as_array)
        .filter(|proposals| !proposals.is_empty())
    {
        let structured = structured_esp_proposals(proposals, profile_id);
        if structured.is_empty() {
            baseline_esp()
        } else {
            structured
        }
    } else {
        baseline_esp()
    };
    let include_epdg_idr = ike
        .pointer("/identities/idr")
        .and_then(Value::as_array)
        .is_some_and(|rules| !rules.is_empty());
    let eap_method = string_at(ike, "/eap_method").unwrap_or("eap_aka");
    if eap_method != "eap_aka" {
        return Err(format!(
            "carrier_catalog_v7_eap_method_unsupported:{profile_id}:{eap_method}"
        ));
    }
    Ok((
        EpdgPolicyRecord {
            host,
            port: epdg_port
                .or_else(|| u16_at(ike, "/initial_port"))
                .unwrap_or(500),
            apn: Some(apn.to_string()),
            ip_stack: ip_family.to_string(),
            dns_server: dns_servers.first().cloned(),
            dns_servers,
        },
        Ikev2PolicyRecord {
            nat_keepalive_seconds: u16_at(ike, "/nat_keepalive_seconds").unwrap_or(20),
            dpd_interval_seconds: u16_at(ike, "/dpd_interval_seconds").unwrap_or(600),
            reauth_interval_seconds: u16_at(ike, "/reauth_interval_seconds"),
            ike_proposals,
            esp_proposals,
            aka_challenge_mode: eap_method.to_string(),
            include_epdg_idr,
        },
    ))
}

fn derived_epdg_fqdn(meta: &ProfileMetaRow) -> String {
    format!(
        "epdg.epc.mnc{:0>3}.mcc{}.pub.3gppnetwork.org",
        meta.mnc, meta.mcc
    )
}

fn structured_ike_proposals(proposals: &[Value], profile_id: &str) -> Vec<String> {
    proposals
        .iter()
        .filter_map(|proposal| {
            let encryption = algorithm_token(
                scalar_string_at(proposal, "/encryption"),
                profile_id,
                "ike_encryption",
            )
            .ok()?;
            let integrity = algorithm_token(
                scalar_string_at(proposal, "/integrity"),
                profile_id,
                "ike_integrity",
            )
            .ok()?;
            let prf = match scalar_string_at(proposal, "/prf") {
                Some(value) => {
                    let normalized = algorithm_token(Some(value), profile_id, "ike_prf").ok()?;
                    (normalized != integrity)
                        .then(|| format!("prf{normalized}"))
                        .unwrap_or_default()
                }
                _ => String::new(),
            };
            let dh_group = dh_group_token(
                proposal
                    .pointer("/dh_group")
                    .and_then(Value::as_u64)
                    .and_then(|value| u16::try_from(value).ok()),
                profile_id,
            )
            .ok()?;
            let mut canonical = format!("{encryption}-{integrity}");
            if !prf.is_empty() {
                canonical.push('-');
                canonical.push_str(&prf);
            }
            canonical.push('-');
            canonical.push_str(&dh_group);
            Some(canonical)
        })
        .collect::<Vec<_>>()
}

fn structured_esp_proposals(proposals: &[Value], profile_id: &str) -> Vec<String> {
    proposals
        .iter()
        .filter_map(|proposal| {
            let encryption = algorithm_token(
                scalar_string_at(proposal, "/encryption"),
                profile_id,
                "child_encryption",
            )
            .ok()?;
            let integrity = algorithm_token(
                scalar_string_at(proposal, "/integrity"),
                profile_id,
                "child_integrity",
            )
            .ok()?;
            Some(format!("{encryption}-{integrity}"))
        })
        .collect::<Vec<_>>()
}

fn scalar_string_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    let value = value.pointer(pointer)?;
    if let Some(value) = value.as_str() {
        return Some(value);
    }
    value.as_array()?.iter().find_map(Value::as_str)
}

fn algorithm_token(value: Option<&str>, profile_id: &str, field: &str) -> Result<String, String> {
    let value = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("carrier_catalog_v7_proposal_field_missing:{profile_id}:{field}"))?;
    let normalized = value.to_ascii_lowercase().replace([' ', '_'], "-");
    let token = match normalized.as_str() {
        "aes-128" | "aes128" => "aes128",
        "aes-256" | "aes256" => "aes256",
        "aes-cbc" | "aes-128-cbc" => "aes128",
        "aes-256-cbc" => "aes256",
        "md5-96" | "md5-128" | "md5" | "hmac-md5" | "hmac-md5-96" => "md5",
        "sha-1" | "sha1" | "sha1-96" | "sha1-160" | "sha-1-96" | "sha-1-160" | "hmac-sha-1"
        | "hmac-sha1" | "hmac-sha1-96" => "sha1",
        "sha-256" | "sha2-256" | "sha256" | "hmac-sha2-256" | "hmac-sha-256" => "sha256",
        "sha-384" | "sha2-384" | "sha384" | "hmac-sha2-384" | "hmac-sha-384" => "sha384",
        "sha-512" | "sha2-512" | "sha512" | "hmac-sha2-512" | "hmac-sha-512" => "sha512",
        "aes-xcbc" => "aes-xcbc",
        _ => {
            return Err(format!(
                "carrier_catalog_v7_proposal_algorithm_unsupported:{profile_id}:{field}:{value}"
            ))
        }
    };
    Ok(token.to_string())
}

fn dh_group_token(value: Option<u16>, profile_id: &str) -> Result<String, String> {
    let token = match value {
        Some(1) => "modp768",
        Some(2) => "modp1024",
        Some(5) => "modp1536",
        Some(14) => "modp2048",
        Some(15) => "modp3072",
        Some(16) => "modp4096",
        Some(18) => "modp8192",
        None => "modp2048",
        _ => {
            return Err(format!(
                "carrier_catalog_v7_proposal_dh_group_unsupported:{profile_id}:{}",
                value.map_or_else(|| "missing".to_string(), |value| value.to_string())
            ))
        }
    };
    Ok(token.to_string())
}

fn project_register(
    profile_id: &str,
    meta: &ProfileMetaRow,
    access: CatalogAccessKind,
    config: &Value,
    domain: &str,
) -> Result<RegisterPolicyRecord, String> {
    let register = config
        .pointer("/sip/common/register")
        .unwrap_or(&Value::Null);
    let sec_agree_mode = match string_at(register, "/security_agreement").unwrap_or("auto") {
        value @ ("auto" | "required" | "disabled") => value.to_string(),
        unsupported => {
            return Err(format!(
                "carrier_catalog_sec_agree_unsupported:{profile_id}:{unsupported}"
            ))
        }
    };
    let mut supported = string_array_or_csv(register, "/supported").unwrap_or_default();
    if sec_agree_mode == "required"
        && !supported
            .iter()
            .any(|value| value.eq_ignore_ascii_case("sec-agree"))
    {
        supported.push("sec-agree".to_string());
    }
    let mut security_client = security_client_values(config, access)?;
    if sec_agree_mode != "disabled" && security_client.is_empty() {
        security_client.push(BASELINE_SECURITY_CLIENT.to_string());
    }
    // iOS bundles express PANI policy through `country_of_origination_format`
    // (`PANI`/`BOTH` = the REGISTER carries P-Access-Network-Info; `NONE` =
    // never). Carriers that set it want PANI even when the legacy
    // include_pani_* booleans are absent, so treat it as the default.
    let country_of_origination_format = string_at(register, "/country_of_origination_format")
        .unwrap_or("")
        .to_string();
    let country_needs_pani = matches!(country_of_origination_format.as_str(), "PANI" | "BOTH");
    let include_pani_initial =
        bool_at(register, "/include_pani_initial").unwrap_or(country_needs_pani);
    let include_pani_authenticated =
        bool_at(register, "/include_pani_authenticated").unwrap_or(country_needs_pani);
    let pani = string_at(register, "/access_network_info")
        .map(|value| expand_ims_static_template(value, meta, domain, "sip_access_network_info"))
        .transpose()?
        .unwrap_or_else(|| default_access_network_info(access).to_string());
    let visited_network_header = string_at(register, "/visited_network_header")
        .map(|value| expand_ims_static_template(value, meta, domain, "sip_visited_network_id"))
        .transpose()?;
    let contact_param_order = contact_parameters(config, meta, domain)?;
    let include_mmtel_features = contact_param_order.iter().any(|parameter| {
        matches!(
            parameter
                .split_once('=')
                .map_or(parameter.as_str(), |(name, _)| name)
                .to_ascii_lowercase()
                .as_str(),
            "audio" | "+g.3gpp.icsi-ref"
        )
    });
    Ok(RegisterPolicyRecord {
        supported_header: supported.join(","),
        request_uri_policy: string_at(register, "/request_uri_policy")
            .unwrap_or("home_domain")
            .to_string(),
        include_pani_initial,
        include_pani_authenticated,
        // TS 24.229 §5.1.1.1: an AKA UE's initial REGISTER SHALL carry an
        // empty Authorization (username/realm/uri populated, nonce/response
        // empty) so the network can challenge it. Default to that shape when
        // the carrier requires sec-agree (the normal IMS AKA path); the
        // explicit JSON value still wins.
        initial_authorization: string_at(register, "/initial_authorization")
            .map(str::to_string)
            .unwrap_or_else(|| {
                if sec_agree_mode == "required" {
                    "aka_empty".to_string()
                } else {
                    "none".to_string()
                }
            }),
        include_mmtel_features,
        include_route_header: bool_at(register, "/include_route_header").unwrap_or(false),
        include_visited_network: visited_network_header.is_some(),
        include_p_preferred_identity: bool_at(register, "/include_p_preferred_identity")
            .unwrap_or(true),
        visited_network_header,
        allow_methods: string_array_or_csv(register, "/allow_methods")
            .map(|methods| methods.join(",")),
        strict_security_server_offer: !security_client.is_empty(),
        enable_initial_reject_fallback: false,
        use_plain_digest_placeholder: false,
        require_sec_agree_headers: bool_at(register, "/require_sec_agree_headers").unwrap_or(false),
        // RFC 3329 §2.3: the client MUST add both Require and Proxy-Require
        // with "sec-agree" when it offers sec-agree. When the carrier marks
        // security_agreement=required, default Proxy-Require to on so the
        // REGISTER is complete even if the bundle omitted the flag.
        proxy_require_sec_agree_headers: bool_at(register, "/proxy_require_sec_agree_headers")
            .unwrap_or(sec_agree_mode == "required"),
        sec_agree_mode,
        security_client_mechanisms: security_client,
        live_header_variant_set: "catalog_v7".to_string(),
        expires_seconds: u32_at(register, "/requested_expires_seconds")
            .unwrap_or(profiles::DEFAULT_REGISTER_EXPIRES_SECONDS),
        access_network_info: pani,
        contact_mode: string_at(register, "/contact_mode")
            .unwrap_or("standard")
            .to_string(),
        contact_param_order,
        always_add_sip_instance: bool_at(register, "/always_add_sip_instance").unwrap_or(false),
        enable_cellular_network_info: bool_at(register, "/enable_cellular_network_info")
            .unwrap_or(false),
        temporary_status_codes: u16_array_at(register, "/temporary_status_codes")
            .unwrap_or_else(|| profiles::DEFAULT_TEMPORARY_STATUS_CODES.to_vec()),
        forbidden_status_codes: u16_array_at(register, "/forbidden_status_codes")
            .unwrap_or_else(|| profiles::DEFAULT_FORBIDDEN_STATUS_CODES.to_vec()),
        initial_reject_fallback_status_codes: u16_array_at(
            register,
            "/initial_reject_fallback_status_codes",
        )
        .unwrap_or_else(|| profiles::DEFAULT_INITIAL_REJECT_FALLBACK_STATUS_CODES.to_vec()),
        temporary_retry_seconds: u16_at(register, "/temporary_retry_seconds")
            .unwrap_or(profiles::DEFAULT_TEMPORARY_RETRY_SECONDS),
    })
}

fn security_client_values(
    config: &Value,
    access: CatalogAccessKind,
) -> Result<Vec<String>, String> {
    let Some(values) = config
        .pointer("/sip/common/security_client")
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };
    values
        .iter()
        .map(|value| {
            if let Some(value) = value.as_str() {
                validate_security_client(value, access)?;
                return Ok(value.to_string());
            }
            if string_at(value, "/mechanism").is_some_and(|value| value != "ipsec-3gpp") {
                return Err("carrier_catalog_v7_security_mechanism_unsupported".to_string());
            }
            let integrity = string_at(value, "/integrity_algorithm");
            let encryption = string_at(value, "/encryption_algorithm");
            let protocol = string_at(value, "/protocol");
            let mode = string_at(value, "/mode");
            match (integrity, encryption, protocol, mode) {
                (Some(integrity), Some(encryption), Some(protocol), Some(mode)) => {
                    let value = format!("{integrity}/{encryption}/{protocol}/{mode}");
                    validate_security_client(&value, access)?;
                    Ok(value)
                }
                _ => Err("carrier_catalog_v7_security_client_invalid".to_string()),
            }
        })
        .collect()
}

fn validate_security_client(value: &str, access: CatalogAccessKind) -> Result<(), String> {
    let fields = value.split('/').collect::<Vec<_>>();
    if fields.len() != 4 {
        return Err("carrier_catalog_v7_security_client_invalid".to_string());
    }
    let supported = match access {
        CatalogAccessKind::WifiEpdg => {
            fields[0].eq_ignore_ascii_case("hmac-sha-1-96")
                && matches!(fields[1].to_ascii_lowercase().as_str(), "aes-cbc" | "null")
                && fields[2].eq_ignore_ascii_case("esp")
                && fields[3].eq_ignore_ascii_case("trans")
        }
        CatalogAccessKind::LteEpc => {
            matches!(
                fields[0].to_ascii_lowercase().as_str(),
                "hmac-md5-96" | "hmac-sha-1-96" | "hmac-sha1-96"
            ) && matches!(fields[1].to_ascii_lowercase().as_str(), "null" | "aes-cbc")
                && fields[2].eq_ignore_ascii_case("esp")
                && fields[3].eq_ignore_ascii_case("trans")
        }
    };
    if supported {
        Ok(())
    } else {
        Err(format!(
            "carrier_catalog_v7_security_client_unsupported:{}:{value}",
            access.as_str()
        ))
    }
}

fn contact_parameters(
    config: &Value,
    meta: &ProfileMetaRow,
    domain: &str,
) -> Result<Vec<String>, String> {
    let Some(values) = config
        .pointer("/sip/common/contact_parameters")
        .and_then(Value::as_array)
    else {
        return Ok(Vec::new());
    };
    let mut parameters = Vec::new();
    for value in values {
        let Some(name) = string_at(value, "/name") else {
            return Err("carrier_catalog_v7_contact_parameter_name_missing".to_string());
        };
        let action = string_at(value, "/action").unwrap_or("add");
        if matches!(action, "omit" | "replace") {
            parameters.retain(|parameter: &String| {
                !parameter
                    .split_once('=')
                    .map_or(parameter.as_str(), |(candidate, _)| candidate)
                    .eq_ignore_ascii_case(name)
            });
        }
        if action == "omit" {
            continue;
        }
        let parameter = string_at(value, "/value_template")
            .map(|template| {
                expand_ims_static_template(template, meta, domain, "sip_contact_parameter_value")
                    .map(|expanded| format!("{name}={expanded}"))
            })
            .transpose()?
            .unwrap_or_else(|| name.to_string());
        parameters.push(parameter);
    }
    Ok(parameters)
}

fn expanded_optional(
    value: &Value,
    path: &str,
    meta: &ProfileMetaRow,
    domain: &str,
) -> Result<Option<String>, String> {
    string_at(value, path)
        .map(|value| expand_ims_static_template(value, meta, domain, path))
        .transpose()
}

fn string_at<'a>(value: &'a Value, pointer: &str) -> Option<&'a str> {
    value.pointer(pointer).and_then(Value::as_str)
}

fn bool_at(value: &Value, pointer: &str) -> Option<bool> {
    value.pointer(pointer).and_then(Value::as_bool)
}

fn u16_at(value: &Value, pointer: &str) -> Option<u16> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .and_then(|value| u16::try_from(value).ok())
}

fn u32_at(value: &Value, pointer: &str) -> Option<u32> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

fn string_array_at(value: &Value, pointer: &str) -> Option<Vec<String>> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
}

fn string_array_or_csv(value: &Value, pointer: &str) -> Option<Vec<String>> {
    let value = value.pointer(pointer)?;
    if let Some(values) = value.as_array() {
        return Some(
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        );
    }
    value.as_str().map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect()
    })
}

fn u16_array_at(value: &Value, pointer: &str) -> Option<Vec<u16>> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_u64)
                .filter_map(|value| u16::try_from(value).ok())
                .collect()
        })
}

fn v7_required(profile_id: &str, access: CatalogAccessKind, path: &str) -> String {
    format!(
        "carrier_catalog_required_field_missing:{profile_id}:{}:{path}",
        access.as_str()
    )
}

#[cfg(test)]
pub(super) fn test_catalog_fixture() -> (super::CarrierCatalog, std::path::PathBuf) {
    tests::fixture()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use chrono::Utc;

    use super::*;
    use crate::connectivity::modems::ims::vowifi::carrier_catalog::CarrierCatalog;

    pub(super) fn fixture() -> (CarrierCatalog, PathBuf) {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "simadmin-carrier-catalog-v7-{}-{}-{}.sqlite3",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default(),
            SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        let conn = Connection::open(&path).expect("create v7 fixture");
        conn.execute_batch(
            r#"
            PRAGMA application_id = 1128419922;
            PRAGMA user_version = 7;
            CREATE TABLE catalog_metadata (
                singleton INTEGER PRIMARY KEY,
                schema_name TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                config_contract TEXT NOT NULL,
                release_id TEXT NOT NULL,
                generated_at TEXT NOT NULL,
                generator_name TEXT NOT NULL,
                generator_version TEXT NOT NULL,
                source_manifest_sha256 TEXT,
                sealed INTEGER NOT NULL,
                notes TEXT
            );
            CREATE TABLE visual_assets(asset_id TEXT PRIMARY KEY);
            CREATE TABLE carriers (
                carrier_id TEXT PRIMARY KEY,
                canonical_name TEXT NOT NULL,
                brand_name TEXT,
                legal_name TEXT,
                carrier_kind TEXT NOT NULL,
                country_iso2 TEXT,
                tadig TEXT,
                website TEXT,
                aliases_json TEXT NOT NULL,
                primary_asset_id TEXT,
                notes TEXT
            );
            CREATE TABLE carrier_profiles (
                profile_id TEXT PRIMARY KEY,
                carrier_id TEXT NOT NULL,
                display_name TEXT NOT NULL,
                profile_kind TEXT NOT NULL,
                priority INTEGER NOT NULL,
                confidence INTEGER NOT NULL,
                lte_ims_status TEXT NOT NULL,
                nr_ims_status TEXT NOT NULL,
                vowifi_status TEXT NOT NULL,
                profile_asset_id TEXT,
                config_json TEXT NOT NULL
            );
            CREATE TABLE profile_match_rules (
                match_rule_id INTEGER PRIMARY KEY,
                profile_id TEXT NOT NULL,
                priority INTEGER NOT NULL,
                plmn TEXT,
                imsi_prefix TEXT,
                iccid_prefix TEXT,
                gid1 TEXT,
                gid2 TEXT,
                spn TEXT,
                is_exclusion INTEGER NOT NULL
            );
            CREATE TABLE source_artifacts(source_id INTEGER PRIMARY KEY);
            CREATE TABLE profile_sources(profile_id TEXT, source_id INTEGER);
            CREATE TABLE field_evidence(evidence_id INTEGER PRIMARY KEY);

            INSERT INTO catalog_metadata VALUES (
                1, 'carrier_bundles', 7, 'carrier-bundles-ims-v1',
                'v7-test-release', '2026-08-08T00:00:00Z',
                'fixture', '1', NULL, 1, NULL
            );
            INSERT INTO carriers VALUES (
                'test-carrier', 'Test Mobile', 'Test', 'Test Mobile Ltd',
                'mno', 'GB', NULL, NULL,
                '["Test", "Test Telecom"]', NULL, NULL
            );
            INSERT INTO carrier_profiles VALUES (
                'test-v7-23433', 'test-carrier', 'Test v7', 'default', 10, 100,
                'ready', 'unknown', 'ready', NULL,
                '{
                  "protocol_baseline": "carrier-bundles-ims-v1",
                  "ims": {
                    "home_domain": "ims.mnc{mnc3}.mcc{mcc}.example",
                    "realm": "{home_domain}",
                    "authentication": {"scheme": "ims_aka", "algorithm": "AKAv1-MD5"},
                    "identity_templates": [
                      {"role": "impi", "source": "derived_imsi", "value_template": "{imsi}@{home_domain}"}
                    ]
                  },
                  "access": {
                    "lte": {"apn": "lte-ims", "ip_family": "ipv6", "pcscf_discovery": ["pco"]},
                    "vowifi": {
                      "apn": "wifi-ims",
                      "ip_family": "ipv4v6",
                      "epdg": [{"address": "epdg.mnc{mnc3}.mcc{mcc}.example", "discovery": "static"}],
                      "pcscf_discovery": ["ike_cfg"],
                      "ike": {
                        "eap_method": "eap_aka",
                        "initial_port": 500,
                        "nat_keepalive_seconds": 20,
                        "dpd_interval_seconds": 60,
                        "ike_sa_proposals": [
                          {
                            "authentication": "None",
                            "dh_group": 14,
                            "eap_method": "EAP-AKA",
                            "encryption": "AES-256",
                            "integrity": "SHA2-256",
                            "prf": "SHA2-256"
                          }
                        ],
                        "child_sa_proposals": [
                          {
                            "dh_group": 14,
                            "encryption": ["AES-256"],
                            "integrity": ["SHA2-256"]
                          }
                        ],
                        "identities": {"idr": [{"source": "epdg_fqdn"}]}
                      }
                    }
                  },
                  "sip": {
                    "common": {
                      "register": {
                        "security_agreement": "required",
                        "requested_expires_seconds": 1800,
                        "contact_mode": "standard",
                        "include_pani_initial": true,
                        "user_agent": "Catalog v7 IMS"
                      },
                      "security_client": [
                        {
                          "integrity_algorithm": "hmac-sha-1-96",
                          "encryption_algorithm": "aes-cbc",
                          "protocol": "esp",
                          "mode": "trans"
                        }
                      ]
                    }
                  },
                  "media": {
                    "audio": {
                      "codecs": [
                        {
                          "name": "EVS",
                          "payload_type": 109,
                          "sample_rate": 16000,
                          "bandwidth": "nb-swb",
                          "bitrate": "5.9-24.4"
                        },
                        {"name": "AMR-WB", "payload_type": 104, "sample_rate": 16000}
                      ]
                    }
                  },
                  "services": {"volte": true, "vowifi": true, "smsoip": true},
                  "readiness": {"lte_missing": [], "vowifi_missing": []}
                }'
            );
            INSERT INTO profile_match_rules VALUES (
                1, 'test-v7-23433', 1, '23433', '234330', NULL, NULL, NULL, NULL, 0
            );
            INSERT INTO carrier_profiles VALUES (
                'test-v7-23434', 'test-carrier', 'Test v7 AKAv2', 'default', 10, 100,
                'ready', 'unknown', 'ready', NULL,
                '{
                  "protocol_baseline": "carrier-bundles-ims-v1",
                  "ims": {
                    "home_domain": "ims.mnc034.mcc234.example",
                    "realm": "{home_domain}",
                    "authentication": {"scheme": "ims_aka", "algorithm": "AKAv2-MD5"},
                    "identity_templates": [
                      {"role": "impi", "source": "derived_imsi", "value_template": "{imsi}@{home_domain}"}
                    ]
                  },
                  "access": {
                    "lte": {"apn": "lte-ims", "ip_family": "ipv6", "pcscf_discovery": ["pco"]},
                    "vowifi": {
                      "apn": "wifi-ims",
                      "ip_family": "ipv4v6",
                      "epdg": [{"address": "epdg.mnc034.mcc234.example", "discovery": "static"}],
                      "pcscf_discovery": ["ike_cfg"],
                      "ike": {
                        "eap_method": "eap_aka",
                        "initial_port": 500,
                        "ike_sa_proposals": [
                          {
                            "authentication": "None",
                            "dh_group": 1,
                            "eap_method": "EAP-AKA",
                            "encryption": "AES-128",
                            "integrity": "MD5-96",
                            "prf": "MD5-128"
                          },
                          {
                            "authentication": "None",
                            "dh_group": 16,
                            "eap_method": "EAP-AKA",
                            "encryption": "AES-256",
                            "integrity": "SHA2-384",
                            "prf": "SHA2-384"
                          }
                        ],
                        "child_sa_proposals": [
                          {
                            "dh_group": 1,
                            "encryption": ["AES-128"],
                            "integrity": ["MD5-96"]
                          },
                          {
                            "dh_group": 16,
                            "encryption": ["AES-256"],
                            "integrity": ["SHA2-384"]
                          }
                        ],
                        "identities": {"idr": [{"source": "epdg_fqdn"}]}
                      }
                    }
                  },
                  "sip": {"common": {"register": {"security_agreement": "required"}}},
                  "services": {"volte": true, "vowifi": true},
                  "readiness": {"lte_missing": [], "vowifi_missing": []}
                }'
            );
            INSERT INTO profile_match_rules VALUES (
                2, 'test-v7-23434', 1, '23434', '234340', NULL, NULL, NULL, NULL, 0
            );
            "#,
        )
        .expect("populate v7 fixture");
        drop(conn);
        let catalog = CarrierCatalog::open(&path).expect("open v7 fixture");
        (catalog, path)
    }

    #[test]
    fn resolves_compiled_v7_json_for_lte_and_vowifi() {
        let (catalog, path) = fixture();
        let release = catalog.release().expect("release");
        assert_eq!(release.release_id, "v7-test-release");

        let wifi = catalog
            .resolve_for_imsi("234330123456789", None, CatalogAccessKind::WifiEpdg)
            .expect("wifi query")
            .expect("wifi profile");
        assert_eq!(wifi.record.epdg.host, "epdg.mnc033.mcc234.example");
        assert_eq!(wifi.record.epdg.apn.as_deref(), Some("wifi-ims"));
        assert_eq!(wifi.record.ims.local_port, 5060);
        assert_eq!(wifi.record.ims.user_agent, "Catalog v7 IMS");
        assert_eq!(wifi.record.ims.register.expires_seconds, 1800);
        assert_eq!(wifi.record.ims.register.sec_agree_mode, "required");
        assert_eq!(wifi.record.ikev2.ike_proposals, ["aes256-sha256-modp2048"]);
        assert_eq!(wifi.record.ikev2.esp_proposals, ["aes256-sha256"]);
        assert_eq!(wifi.record.voice.preferred_codecs, ["evs", "amr-wb"]);
        assert_eq!(wifi.record.voice.codec_policies.len(), 2);
        assert_eq!(wifi.record.voice.codec_policies[0].payload_type, Some(109));
        assert_eq!(
            wifi.record.voice.codec_policies[0].fmtp.as_deref(),
            Some("br=5.9-24.4; bw=nb-swb")
        );
        let offer = crate::connectivity::modems::ims::vowifi::voice::build_profile_codec_offer(
            wifi.record.intern(),
        );
        assert_eq!(
            offer[0].codec,
            crate::connectivity::core::voice::AudioCodec::Evs
        );
        assert_eq!(offer[0].payload_type, 109);
        assert_eq!(offer[0].clock_rate, 16000);
        assert_eq!(offer[0].fmtp.as_deref(), Some("br=5.9-24.4; bw=nb-swb"));

        let lte = catalog
            .resolve_for_imsi("234330123456789", None, CatalogAccessKind::LteEpc)
            .expect("lte query")
            .expect("lte profile");
        assert_eq!(lte.record.epdg.apn.as_deref(), Some("lte-ims"));
        assert_eq!(lte.record.ims.domain, "ims.mnc033.mcc234.example");
        assert_eq!(lte.record.meta.aliases, ["Test", "Test Telecom"]);

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn projects_explicit_xcap_partial_update_and_tls_policy() {
        let config = serde_json::json!({
            "services": {
                "ut": {
                    "enabled": true,
                    "xcap": {
                        "root": "https://xcap.example.test/simservs",
                        "document_selector": "users/sip:subscriber@example.test",
                        "namespace": "urn:3gpp:ns:xml:simservs",
                        "authentication": "digest_aka",
                        "partial_update": {
                            "enabled": true,
                            "call_waiting_selector": "ss:communication-waiting/ss:active",
                            "diversion_rule_selector": "ss:communication-diversion/cp:ruleset/cp:rule[@id='{rule-id}']"
                        },
                        "tls": {
                            "min_version": "1.3",
                            "max_version": "1.3",
                            "builtin_roots": false,
                            "additional_ca_pem": "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----"
                        }
                    }
                }
            }
        });

        let policy = project_ut_policy(&config);
        assert!(policy.enabled);
        assert!(policy.partial_update);
        assert_eq!(
            policy.call_waiting_selector.as_deref(),
            Some("ss:communication-waiting/ss:active")
        );
        assert_eq!(
            policy.diversion_rule_selector.as_deref(),
            Some("ss:communication-diversion/cp:ruleset/cp:rule[@id='{rule-id}']")
        );
        assert_eq!(policy.tls_min_version, "1.3");
        assert_eq!(policy.tls_max_version, "1.3");
        assert!(!policy.tls_builtin_roots);
        assert!(policy.tls_additional_ca_pem.is_some());
    }

    #[test]
    fn resolves_akav2_and_extended_algorithms_from_structured_proposals() {
        let (catalog, path) = fixture();
        let wifi = catalog
            .resolve_for_imsi("234340123456789", None, CatalogAccessKind::WifiEpdg)
            .expect("wifi query")
            .expect("wifi profile");

        assert_eq!(wifi.record.epdg.host, "epdg.mnc034.mcc234.example");
        assert_eq!(
            wifi.record.ikev2.ike_proposals,
            ["aes128-md5-modp768", "aes256-sha384-modp4096"]
        );
        assert_eq!(
            wifi.record.ikev2.esp_proposals,
            ["aes128-md5", "aes256-sha384"]
        );
        assert_eq!(
            wifi.record.voice.preferred_codecs,
            ["amr-wb", "amr", "pcmu", "pcma"]
        );
        assert!(wifi.record.voice.codec_policies.is_empty());

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn fills_baseline_ike_esp_proposals_when_catalog_proposals_missing_or_empty() {
        let (catalog, path) = fixture();
        {
            let conn = Connection::open(&path).expect("open fixture for mutation");
            conn.execute_batch(
                "INSERT INTO carrier_profiles(
                     profile_id, carrier_id, display_name, profile_kind,
                     priority, confidence, lte_ims_status, nr_ims_status,
                     vowifi_status, profile_asset_id, config_json
                 )
                 SELECT 'test-v7-23435', carrier_id, 'Pixel no proposals', profile_kind,
                        30, confidence, lte_ims_status, nr_ims_status,
                        'ready', profile_asset_id,
                        json_set(
                            json_remove(config_json,
                                '$.access.vowifi.ike.ike_sa_proposals',
                                '$.access.vowifi.ike.child_sa_proposals'),
                            '$.readiness.vowifi_missing', json_array()
                        )
                 FROM carrier_profiles WHERE profile_id = 'test-v7-23434';
                 INSERT INTO profile_match_rules VALUES (
                     4, 'test-v7-23435', 1, '23435', '234350', NULL, NULL, NULL, NULL, 0
                 );
                 INSERT INTO carrier_profiles(
                     profile_id, carrier_id, display_name, profile_kind,
                     priority, confidence, lte_ims_status, nr_ims_status,
                     vowifi_status, profile_asset_id, config_json
                 )
                 SELECT 'test-v7-23436', carrier_id, 'Pixel empty arrays', profile_kind,
                        31, confidence, lte_ims_status, nr_ims_status,
                        'ready', profile_asset_id,
                        json_set(
                            config_json,
                            '$.access.vowifi.ike.ike_sa_proposals', json_array(),
                            '$.access.vowifi.ike.child_sa_proposals', json_array(),
                            '$.readiness.vowifi_missing', json_array()
                        )
                 FROM carrier_profiles WHERE profile_id = 'test-v7-23434';
                 INSERT INTO profile_match_rules VALUES (
                     5, 'test-v7-23436', 1, '23436', '234360', NULL, NULL, NULL, NULL, 0
                 );",
            )
            .expect("insert pixel-like profiles");
        }

        let baseline_ike = [
            "aes128-sha256-modp2048",
            "aes128-sha1-modp2048",
            "aes128-sha256-modp1024",
        ];
        for imsi in ["234350123456789", "234360123456789"] {
            let wifi = catalog
                .resolve_for_imsi(imsi, None, CatalogAccessKind::WifiEpdg)
                .expect("wifi query")
                .expect("wifi profile");
            assert_eq!(wifi.record.ikev2.ike_proposals, baseline_ike);
            assert_eq!(
                wifi.record.ikev2.esp_proposals,
                ["aes128-sha256", "aes128-sha1"]
            );
        }

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn derives_epdg_fqdn_and_baseline_security_client_and_filters_unsupported_proposals() {
        let (catalog, path) = fixture();
        {
            let conn = Connection::open(&path).expect("open fixture for mutation");
            conn.execute_batch(
                "INSERT INTO carrier_profiles(
                     profile_id, carrier_id, display_name, profile_kind,
                     priority, confidence, lte_ims_status, nr_ims_status,
                     vowifi_status, profile_asset_id, config_json
                 )
                 SELECT 'test-v7-23437', carrier_id, 'Pixel derived epdg', profile_kind,
                        32, confidence, lte_ims_status, nr_ims_status,
                        'ready', profile_asset_id,
                        json_remove(
                            json_set(config_json,
                                '$.access.vowifi.epdg', json('[]'),
                                '$.access.vowifi.ike.ike_sa_proposals',
                                    json('[{\"encryption\":\"AES-CBC\",\"integrity\":\"SHA1-96\",\"prf\":\"SHA1-160\",\"dh_group\":14},{\"encryption\":\"AES-CTR-128\",\"integrity\":\"SHA1-96\",\"prf\":\"SHA1-160\",\"dh_group\":14},{\"encryption\":\"AES-CBC\",\"integrity\":\"AES-XCBC-96\",\"prf\":\"AES128-XCBC\",\"dh_group\":14}]'),
                                '$.access.vowifi.ike.child_sa_proposals',
                                    json('[{\"encryption\":\"AES-CBC\",\"integrity\":\"SHA1-96\"}]')),
                            '$.sip.common.register.security_agreement'
                        )
                 FROM carrier_profiles WHERE profile_id = 'test-v7-23434';
                 INSERT INTO profile_match_rules VALUES (
                     6, 'test-v7-23437', 1, '234037', '2340370', NULL, NULL, NULL, NULL, 0
                 );",
            )
            .expect("insert derived-epdg profile");
        }

        let wifi = catalog
            .resolve_for_imsi("2340370123456789", None, CatalogAccessKind::WifiEpdg)
            .expect("wifi query")
            .expect("wifi profile");

        // ePDG missing -> TS 24.302 derived FQDN from PLMN 234037 (mcc 234, mnc 037).
        assert_eq!(
            wifi.record.epdg.host,
            "epdg.epc.mnc037.mcc234.pub.3gppnetwork.org"
        );
        // Unsupported proposal entries (AES-CTR-128, AES-XCBC-96, PRF AES128-XCBC)
        // are filtered out; the supported AES-CBC/SHA1 entry remains.
        assert_eq!(wifi.record.ikev2.ike_proposals, ["aes128-sha1-modp2048"]);
        assert_eq!(wifi.record.ikev2.esp_proposals, ["aes128-sha1"]);
        // security_agreement removed -> mode auto -> baseline Security-Client applied.
        assert_eq!(
            wifi.record.ims.register.security_client_mechanisms,
            ["hmac-sha-1-96/aes-cbc/esp/trans"]
        );

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn rejects_non_ready_v7_profile_before_registration() {
        let (catalog, path) = fixture();
        {
            let conn = Connection::open(&path).expect("open fixture for mutation");
            conn.execute(
                "UPDATE carrier_profiles SET vowifi_status = 'partial' WHERE profile_id = ?1",
                ["test-v7-23433"],
            )
            .expect("mark profile partial");
        }
        let error = catalog
            .get("test-v7-23433", CatalogAccessKind::WifiEpdg)
            .expect_err("partial profile must fail");
        assert_eq!(
            error,
            "carrier_catalog_profile_not_ready:test-v7-23433:wifi_epdg:partial"
        );
        let remaining = catalog
            .list(CatalogAccessKind::WifiEpdg)
            .expect("list ready profiles");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].record.meta.profile_id, "test-v7-23434");

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn one_ready_row_with_missing_requirements_does_not_poison_the_catalog() {
        let (catalog, path) = fixture();
        {
            let conn = Connection::open(&path).expect("open fixture for mutation");
            conn.execute_batch(
                "INSERT INTO carrier_profiles(
                     profile_id, carrier_id, display_name, profile_kind,
                     priority, confidence, lte_ims_status, nr_ims_status,
                     vowifi_status, profile_asset_id, config_json
                 )
                 SELECT 'broken-v7-23433', carrier_id, 'Broken v7', profile_kind,
                        20, confidence, lte_ims_status, nr_ims_status,
                        vowifi_status, profile_asset_id,
                        json_set(
                            config_json,
                            '$.readiness.vowifi_missing',
                            json_array('/access/vowifi/ike/identities/idi')
                        )
                 FROM carrier_profiles WHERE profile_id = 'test-v7-23433';
                 INSERT INTO profile_match_rules VALUES (
                     3, 'broken-v7-23433', 2, '23433', '234331',
                     NULL, NULL, NULL, NULL, 0
                 );",
            )
            .expect("insert incomplete ready row");
        }

        let profiles = catalog
            .list(CatalogAccessKind::WifiEpdg)
            .expect("valid rows remain listable");
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].record.meta.profile_id, "test-v7-23433");
        let error = catalog
            .get("broken-v7-23433", CatalogAccessKind::WifiEpdg)
            .expect_err("missing readiness requirement must fail");
        assert_eq!(
            error,
            "carrier_catalog_profile_not_ready:broken-v7-23433:wifi_epdg:missing:/access/vowifi/ike/identities/idi"
        );

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn v7_gid_rule_is_not_flattened_to_an_imsi_match() {
        let (catalog, path) = fixture();
        {
            let conn = Connection::open(&path).expect("open fixture for mutation");
            conn.execute(
                "UPDATE profile_match_rules SET gid1 = 'A1' WHERE match_rule_id = 1",
                [],
            )
            .expect("add GID constraint");
            conn.execute(
                "UPDATE profile_match_rules SET gid1 = 'B1' WHERE match_rule_id = 2",
                [],
            )
            .expect("add second GID constraint");
        }
        assert!(catalog
            .resolve_for_imsi("234330123456789", Some("23433"), CatalogAccessKind::LteEpc)
            .expect("resolve")
            .is_none());
        assert!(catalog
            .public_identity_matches(CatalogAccessKind::LteEpc)
            .expect("public matches")
            .is_empty());

        std::fs::remove_file(path).expect("remove fixture");
    }

    #[test]
    fn audit_real_v7_catalogs_when_env_set() {
        let Ok(directory) = std::env::var("SIMADMIN_CATALOG_AUDIT_DIR") else {
            return;
        };
        let directory = PathBuf::from(directory);
        for name in [
            "carrier-bundles-ios-ipcc.sqlite3",
            "carrier-bundles-iphone16promax-26.6.sqlite3",
            "carrier-bundles-pixel-mustang.sqlite3",
        ] {
            let path = directory.join(name);
            if !path.exists() {
                eprintln!("MISSING {name}");
                continue;
            }
            let catalog = match CarrierCatalog::open(&path) {
                Ok(catalog) => catalog,
                Err(error) => {
                    eprintln!("OPEN_FAILED {name}: {error}");
                    continue;
                }
            };
            let release = catalog.release().expect("release");
            eprintln!(
                "RELEASE {name}: {} sealed={}",
                release.release_id, release.sealed
            );
            for access in [CatalogAccessKind::LteEpc, CatalogAccessKind::WifiEpdg] {
                let listed = catalog.list(access).expect("list");
                let public = catalog.public_identity_matches(access).expect("public");
                eprintln!(
                    "STATS {name} {} listed={} public={}",
                    access.as_str(),
                    listed.len(),
                    public.len()
                );
                let column = access.v7_status_column();
                let conn = rusqlite::Connection::open_with_flags(
                    &path,
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                )
                .expect("open read-only");
                let sql =
                    format!("SELECT profile_id FROM carrier_profiles WHERE {column} = 'ready'");
                let ids = conn
                    .prepare(&sql)
                    .expect("prepare ids")
                    .query_map([], |row| row.get::<_, String>(0))
                    .expect("query ids")
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .expect("collect ids");
                let mut ok = 0usize;
                let mut errors: std::collections::BTreeMap<String, usize> =
                    std::collections::BTreeMap::new();
                let mut samples: std::collections::BTreeMap<String, String> =
                    std::collections::BTreeMap::new();
                for profile_id in ids {
                    match catalog.get(&profile_id, access) {
                        Ok(Some(_)) => ok += 1,
                        Ok(None) => {
                            *errors.entry("not_found".to_string()).or_default() += 1;
                            samples
                                .entry("not_found".to_string())
                                .or_insert_with(|| profile_id.clone());
                        }
                        Err(error) => {
                            let key = error.split(':').next().unwrap_or("unknown").to_string();
                            *errors.entry(key.clone()).or_default() += 1;
                            samples
                                .entry(key)
                                .or_insert_with(|| format!("{profile_id}:{error}"));
                        }
                    }
                }
                eprintln!(
                    "READY_CHECK {name} {} ok={ok} errors={errors:?}",
                    access.as_str()
                );
                for (key, sample) in samples {
                    eprintln!("ERROR_SAMPLE {name} {} {key}: {sample}", access.as_str());
                }
            }
            if let Ok(Some(profile)) = catalog.get(
                "profile-1and1-de-base-26223-62badc58d5",
                CatalogAccessKind::WifiEpdg,
            ) {
                eprintln!(
                    "SAMPLE {name}: epdg={} ike={:?}",
                    profile.record.epdg.host, profile.record.ikev2.ike_proposals
                );
            }
        }
    }
}
