//! Consumer-side guard for carrier_Bundles' optional-defaults-v1 variants.
//! This proves projection equivalence, not network registration or NR support.

use super::*;
use serde_json::json;
use std::path::Path;

const DEFAULT_PATHS: &[&str] = &[
    "/ims/identity_templates",
    "/ims/transport",
    "/ims/local_port",
    "/access/lte/ip_family",
    "/access/vowifi/ip_family",
    "/access/vowifi/apn",
    "/access/vowifi/ike/initial_port",
    "/access/vowifi/ike/nat_keepalive_seconds",
    "/access/vowifi/ike/dpd_interval_seconds",
];

fn default_config() -> Value {
    json!({
        "protocol_baseline": PROTOCOL_BASELINE,
        "ims": {
            "home_domain": "ims.mnc026.mcc310.3gppnetwork.org",
            "realm": "ims.mnc026.mcc310.3gppnetwork.org",
            "authentication": {"scheme": "ims_aka"},
            "identity_templates": [
                {"identity_type": "nai", "role": "impi", "source": "derived_imsi",
                 "use_when": "if_isim_missing", "value_template": "{imsi}@{home_domain}"},
                {"identity_type": "sip_uri", "role": "impu", "source": "derived_imsi",
                 "use_when": "if_isim_missing", "value_template": "sip:{imsi}@{home_domain}"}
            ],
            "transport": "udp", "local_port": 5060
        },
        "access": {
            "lte": {"apn": "ims", "ip_family": "ipv4v6", "pcscf_discovery": ["pco", "epco"]},
            "nr": {"dnn": "ims", "ip_family": "ipv4v6", "pcscf_discovery": ["epco", "pco"]},
            "vowifi": {
                "apn": "ims", "ip_family": "ipv4v6",
                "epdg": [{"address": "epdg.epc.mnc026.mcc310.pub.3gppnetwork.org"}],
                "pcscf_discovery": ["ike_cfg"],
                "ike": {
                    "eap_method": "eap_aka", "initial_port": 500,
                    "nat_keepalive_seconds": 20, "dpd_interval_seconds": 600,
                    "identities": {
                        "idi": [{"identity_type": "id_rfc822_addr", "value_template": "0{imsi}@nai.epc.mnc{mnc3}.mcc{mcc}.3gppnetwork.org"}],
                        "idr": [{"identity_type": "id_fqdn", "value_template": "{epdg_fqdn}"}]
                    }
                }
            }
        },
        "services": {"volte": false, "vonr": false, "smsoip": true, "vowifi": true}
    })
}

fn project(config: &Value, access: CatalogAccessKind) -> CarrierProfileRecord {
    project_config(
        "variant-fixture",
        &ProfileMetaRow {
            profile_name: "Test".into(),
            brand: "Test".into(),
            legal_name: "Test".into(),
            country_iso2: "US".into(),
            plmn: "31026".into(),
            mcc: "310".into(),
            mnc: "26".into(),
            mnc_length: 2,
        },
        vec![],
        &CatalogRelease {
            release_id: "fixture".into(),
            generated_at: "2026-10-01T00:00:00Z".into(),
            sealed: true,
        },
        access,
        config,
    )
    .expect("project fixture")
}

fn remove(config: &mut Value, pointer: &str) {
    let (parent, key) = pointer.rsplit_once('/').expect("pointer");
    config
        .pointer_mut(parent)
        .and_then(Value::as_object_mut)
        .expect("parent")
        .remove(key)
        .expect("existing default");
}

#[test]
fn optional_defaults_individually_and_together_preserve_complete_projection() {
    let original = default_config();
    for access in [CatalogAccessKind::LteEpc, CatalogAccessKind::WifiEpdg] {
        let before = project(&original, access);
        for pointer in DEFAULT_PATHS {
            let mut reduced = original.clone();
            remove(&mut reduced, pointer);
            assert_eq!(before, project(&reduced, access), "{pointer}: {access:?}");
        }
        let mut reduced = original.clone();
        for pointer in DEFAULT_PATHS {
            remove(&mut reduced, pointer);
        }
        assert_eq!(before, project(&reduced, access));
        assert_eq!(
            original.pointer("/access/nr"),
            reduced.pointer("/access/nr")
        );
    }
}

#[test]
fn udp_must_not_be_elided_over_an_alternate_tcp_policy() {
    let mut original = default_config();
    original["sip"] = json!({"common": {"transport": "tcp"}});
    let mut reduced = original.clone();
    remove(&mut reduced, "/ims/transport");
    assert_eq!(
        project(&original, CatalogAccessKind::LteEpc).ims.transport,
        "udp"
    );
    assert_eq!(
        project(&reduced, CatalogAccessKind::LteEpc).ims.transport,
        "tcp"
    );
}

#[test]
fn standard_looking_realm_and_idr_are_not_equivalent_to_absent() {
    let original = default_config();
    let before = project(&original, CatalogAccessKind::WifiEpdg);
    let mut no_realm = original.clone();
    remove(&mut no_realm, "/ims/realm");
    assert_ne!(
        before.meta.source_refs,
        project(&no_realm, CatalogAccessKind::WifiEpdg)
            .meta
            .source_refs
    );
    let mut no_idr = original.clone();
    remove(&mut no_idr, "/access/vowifi/ike/identities/idr");
    assert!(before.ikev2.include_epdg_idr);
    assert!(
        !project(&no_idr, CatalogAccessKind::WifiEpdg)
            .ikev2
            .include_epdg_idr
    );
}

fn open_snapshot(directory: &Path, entry: &Value) -> Connection {
    let name = entry["database"].as_str().expect("database name");
    assert_eq!(
        Path::new(name).file_name().and_then(|s| s.to_str()),
        Some(name)
    );
    let conn = Connection::open_with_flags(
        directory.join(name),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .expect("open snapshot read-only");
    validate_schema(&conn).expect("compatible schema");
    assert!(read_release(&conn).expect("sealed metadata").sealed);
    conn
}

fn canonical_lookup(
    result: Result<Option<CatalogProfile>, String>,
) -> Result<Option<CarrierProfileRecord>, String> {
    result.map(|profile| {
        profile.map(|profile| {
            let mut record = profile.record;
            let expected = format!("carrier_catalog:{}", profile.release.release_id);
            assert_eq!(record.meta.source_refs.first(), Some(&expected));
            // Variant release identity is deliberately different. Do not erase any
            // other provenance flags, capabilities, policies or runtime fields.
            record.meta.source_refs[0] = "carrier_catalog:<source-snapshot>".into();
            record
        })
    })
}

#[test]
#[ignore = "requires the offline catalog-variants.json and all generated databases"]
fn generated_catalog_variants_project_identically() {
    let directory = std::env::var("SIMADMIN_CATALOG_VARIANTS_DIR").expect("set variants directory");
    let directory = Path::new(&directory);
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(directory.join("catalog-variants.json")).expect("read manifest"),
    )
    .expect("parse manifest");
    assert_eq!(manifest["policy"]["id"], "simadmin-1.1.5-optional-defaults-v1");
    let defaults = manifest["policy"]["optional_defaults"]
        .as_object()
        .expect("defaults");
    assert_eq!(defaults.len(), DEFAULT_PATHS.len());
    let fixture = default_config();
    for pointer in DEFAULT_PATHS {
        assert_eq!(
            defaults.get(*pointer),
            fixture.pointer(pointer),
            "untested producer rule: {pointer}"
        );
    }
    let catalogs = manifest["catalogs"].as_array().expect("sources");
    assert!(!catalogs.is_empty(), "no source was checked");
    let mut checked = 0;
    for catalog in catalogs {
        let variants = &catalog["variants"];
        let full = open_snapshot(directory, &variants["full"]);
        let mut stmt = full
            .prepare("SELECT profile_id FROM carrier_profiles ORDER BY profile_id")
            .expect("profile query");
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .expect("ids")
            .collect::<rusqlite::Result<Vec<_>>>()
            .expect("collect ids");
        assert_eq!(
            ids.len() as u64,
            catalog["source"]["counts"]["carrier_profiles"]
                .as_u64()
                .expect("count")
        );
        for variant in ["no-icons", "minimal-no-icons"] {
            let compact = open_snapshot(directory, &variants[variant]);
            assert_eq!(
                compact
                    .query_row("SELECT count(*) FROM visual_assets", [], |r| r
                        .get::<_, usize>(0))
                    .unwrap(),
                0
            );
            for access in [CatalogAccessKind::LteEpc, CatalogAccessKind::WifiEpdg] {
                let mut successes = 0;
                let mut retained_errors = 0;
                for id in &ids {
                    let before = canonical_lookup(get(&full, id, access));
                    let after = canonical_lookup(get(&compact, id, access));
                    if before.is_ok() {
                        successes += 1;
                    } else {
                        retained_errors += 1;
                    }
                    assert_eq!(
                        before, after,
                        "{} {variant} {id} {access:?}",
                        catalog["source"]["database"]
                    );
                    checked += 1;
                }
                eprintln!("VARIANT_EQ {} {variant} {access:?}: {successes} projections, {retained_errors} identical unavailable/error results",
                    catalog["source"]["database"]);
            }
        }
    }
    eprintln!("VARIANT_EQ_TOTAL {checked} lookups compared; this is not a registration test");
}

#[test]
#[ignore = "requires the original-format directly pruned catalog set"]
fn generated_pruned_catalogs_preserve_other_accesses_and_resolve_derived() {
    use crate::connectivity::modems::ims::vowifi::{carrier_catalog::CarrierCatalog, profile_store::{ProfileStore, ProfileOrigin}};
    use crate::platform::{config::{ImsProfileCandidate, ImsProfileSource}, db::Database};
    use std::sync::Arc;
    let directory = std::env::var("SIMADMIN_CATALOG_PRUNING_DIR").expect("set pruning directory");
    let root = Path::new(&directory);
    let manifest: Value = serde_json::from_slice(&std::fs::read(root.join("catalog-variants.json")).unwrap()).unwrap();
    assert_eq!(manifest["policy"]["id"], "simadmin-simulated-standard-pruning-v1");
    let mut derived_count = 0;
    let mut retained_count = 0;
    for source in manifest["catalogs"].as_array().unwrap() {
        let original = open_snapshot(root, &source["variants"]["full"]);
        let pruned = open_snapshot(root, &source["variants"]["minimal-no-icons"]);
        let report: Value = serde_json::from_slice(&std::fs::read(root.join(source["minimal_elision"]["report"].as_str().unwrap())).unwrap()).unwrap();
        let rows = report["changes"].as_array().unwrap();
        let ids = original.prepare("SELECT profile_id FROM carrier_profiles ORDER BY profile_id").unwrap()
            .query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        let catalog = CarrierCatalog::open(root.join(source["variants"]["minimal-no-icons"]["database"].as_str().unwrap())).unwrap();
        let store = ProfileStore::new(Arc::new(catalog), Arc::new(Database::new(":memory:".into()).unwrap()));
        assert_eq!(pruned.query_row("SELECT config_contract FROM catalog_metadata", [], |r|r.get::<_,String>(0)).unwrap(),PROTOCOL_BASELINE);
        for id in ids {
            let change = rows.iter().find(|r|r["profile_id"] == id);
            for (kind,access) in [("lte",CatalogAccessKind::LteEpc),("vowifi",CatalogAccessKind::WifiEpdg)] {
                let removed = change.is_some_and(|r|r["removed_accesses"].as_array().unwrap().iter().any(|v|v==kind));
                if removed {
                    assert!(get(&original,&id,access).unwrap().is_some(),"source projection was not usable: {id}");
                    assert!(!matches!(get(&pruned,&id,access),Ok(Some(_))));
                    let plmn = change.unwrap()["plmn"].as_str().unwrap();
                    let imsi = format!("{plmn}{}","0".repeat(15-plmn.len()));
                    let candidate = ImsProfileCandidate { source: ImsProfileSource::CarrierCatalog, profile_id: Some(id.clone()) };
                    let resolved = match access {
                        CatalogAccessKind::LteEpc => store.resolve_cellular_ims_candidate(&candidate,None,&imsi,Some(plmn)),
                        CatalogAccessKind::WifiEpdg => store.resolve_vowifi_candidate(&candidate,None,&imsi,Some(plmn)),
                    }.unwrap().expect("existing source-bound fallback");
                    assert_eq!(resolved.origin,ProfileOrigin::Derived);
                    assert_eq!(resolved.profile.meta.plmn,plmn);
                    derived_count += 1;
                } else if !change.is_some_and(|r|r["removed_profile"]==true) {
                    assert_eq!(canonical_lookup(get(&original,&id,access)),canonical_lookup(get(&pruned,&id,access)),"uncovered access changed: {id}/{kind}");
                    retained_count += 1;
                }
            }
            if !change.is_some_and(|r|r["removed_profile"]==true) {
                let read = |c:&Connection| -> Value {let raw:String=c.query_row("SELECT config_json FROM carrier_profiles WHERE profile_id=?1",[&id],|r|r.get(0)).unwrap();serde_json::from_str(&raw).unwrap()};
                assert_eq!(read(&original).pointer("/access/nr"),read(&pruned).pointer("/access/nr"),"NR was changed: {id}");
            }
        }
    }
    assert!(derived_count>0);
    eprintln!("DIRECT_PRUNING_VERIFIED {derived_count} removed access configurations use existing derived resolution; {retained_count} other access projections unchanged; NR preserved");
}

