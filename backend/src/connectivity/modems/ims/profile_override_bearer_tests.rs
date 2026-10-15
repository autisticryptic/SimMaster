use super::*;
use crate::hardware::devices::ims_bearer_policy::{ImsApnAuthentication, ImsApnAuthMethod};

fn override_with_credentials() -> SimOverride {
    let mut override_ = SimOverride::default();
    override_.ims_cellular.bearer = Some(ImsBearerRequirements {
        authentication: ImsApnAuthentication {
            method: ImsApnAuthMethod::PapOrChap,
            username: Some(" synthetic-override-user ".into()),
            password: Some(" synthetic-override-password ".into()),
        },
        ..Default::default()
    });
    override_
}

fn temp_dir() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("simadmin-bearer-override-{}-{}",
        std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed)))
}

#[test]
fn override_sanitized_hides_credentials_but_preserves_atomic_policy_intent() {
    let value = override_with_credentials();
    let sanitized = value.sanitized();
    assert!(!value.is_empty());
    assert!(!sanitized.is_empty());
    let json = serde_json::to_string(&sanitized).unwrap();
    let debug = format!("{value:?}");
    for secret in ["synthetic-override-user", "synthetic-override-password"] {
        assert!(!json.contains(secret));
        assert!(!debug.contains(secret));
    }
    let bearer = sanitized.ims_cellular.bearer.as_ref().unwrap();
    assert_eq!(bearer.authentication.method, ImsApnAuthMethod::PapOrChap);
    assert!(bearer.authentication.username.is_none());
    assert!(bearer.authentication.password.is_none());
    assert!(sanitized.validate_bearer_requirements().is_err());
    assert!(value.validate_bearer_requirements().is_ok());
}

#[test]
fn private_file_and_database_roundtrip_preserve_credentials_and_sim_isolation() {
    let dir = temp_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let database = Arc::new(crate::platform::db::Database::new(dir.join("data.db")).unwrap());
    let first = SimBindingKey::Plain { iccid: "synthetic-sim-a".into() };
    let second = SimBindingKey::Plain { iccid: "synthetic-sim-b".into() };
    let value = override_with_credentials();
    for store in [SimOverrideStore::new(dir.join("files")), SimOverrideStore::database(database.clone())] {
        store.save(&first, &value).unwrap();
        assert_eq!(store.load(&first).unwrap(), Some(value.clone()));
        assert!(store.load(&second).unwrap().is_none());
        let mut empty_password = value.clone();
        empty_password.ims_cellular.bearer.as_mut().unwrap().authentication.password = Some(String::new());
        store.save(&second, &empty_password).unwrap();
        assert_eq!(store.load(&second).unwrap(), Some(empty_password));
        // An explicit default remains stored instead of falling back to catalog.
        let mut explicit_default = SimOverride::default();
        explicit_default.ims_cellular.bearer = Some(ImsBearerRequirements::default());
        store.save(&second, &explicit_default).unwrap();
        assert_eq!(store.load(&second).unwrap(), Some(explicit_default));
    }
    drop(database);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn invalid_bearer_store_input_and_import_fail_without_echoing_secret_values() {
    let dir = temp_dir();
    let store = SimOverrideStore::new(&dir);
    let key = SimBindingKey::Plain { iccid: "synthetic-sim-invalid".into() };
    let mut value = override_with_credentials();
    value.ims_cellular.bearer.as_mut().unwrap().authentication.method = ImsApnAuthMethod::None;
    assert_eq!(store.save(&key, &value).unwrap_err().code(), "ims_cellular.ims_bearer_auth_credentials_conflict");
    let document = serde_json::to_value(OverrideFile::from_override(&key, &value)).unwrap();
    assert!(validate_stored_override_document(&key.sha256(), OVERRIDE_SCHEMA_VERSION, &document).is_err());
    assert!(read_stored_override(OVERRIDE_SCHEMA_VERSION, &document.to_string(), &key).is_err());
    let mut malformed = document;
    malformed["ims"]["cellular_ims"]["bearer"]["authentication"]["method"] = "misplaced-synthetic-secret".into();
    let error = read_stored_override(OVERRIDE_SCHEMA_VERSION, &malformed.to_string(), &key).unwrap_err();
    assert!(!error.code().contains("misplaced-synthetic-secret"));
    assert!(error.code().starts_with("sim_override_corrupt:"));
    assert!(!dir.exists());
}

#[test]
fn persisted_wifi_pap_requirement_is_rejected_not_reinterpreted_as_eap() {
    let mut value = override_with_credentials();
    value.ims_vowifi.bearer = value.ims_cellular.bearer.take();
    assert_eq!(value.validate_bearer_requirements().unwrap_err().code(),
        "ims_vowifi.ims_bearer_wifi_policy_unsupported");
}

#[test]
fn old_override_without_bearer_stays_empty_and_default_inherits_catalog() {
    let value: SimOverride = serde_json::from_str(r#"{"ims_cellular":{},"ims_vowifi":{}}"#).unwrap();
    assert!(value.is_empty());
    assert!(value.ims_cellular.bearer.is_none());
    assert!(value.ims_vowifi.bearer.is_none());
}
