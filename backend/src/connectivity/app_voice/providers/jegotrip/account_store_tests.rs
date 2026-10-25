//! Actions-only SQLite actions. No network, device, SDK, compiled local tests,
//! fake SIM registration, provider readiness or paid service simulation.
use super::*;
use super::super::{
    account_config::{IncomingBinding, ProfileConfig, ProfileProperty, ProfileRoute},
    credentials::Secret,
    tests::imported,
    tls_engine::tests::Certificates,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::{atomic::{AtomicU64, Ordering}, Arc, Barrier}, time::Duration};

fn secret(json: &str) -> Secret { serde_json::from_str(json).unwrap() }
fn fixture(root: &str) -> AccountConfig {
    let mut credentials = imported();
    credentials.sdk.root_der_base64 = Secret::deserialize(serde::de::value::StrDeserializer::<serde::de::value::Error>::new(root)).unwrap();
    credentials.sdk.server_address = secret("\"#fixture-account:tcp -h fixture.invalid -p 443;\"");
    credentials.sdk.selected_session_id = Some(secret("\"fixture-original-selector\""));
    AccountConfig { credentials, bootstrap:None, profile: ProfileConfig {
        route: ProfileRoute::DirectTlsCarrier, authority: secret("\"fixture.invalid\""), enable_sni: true,
        identity_type: secret("\"username\""), domain_name: Some(secret("\"fixture.invalid\"")),
        domain_id: Some(secret("\"fixture-domain\"")), application_id: Some(secret("\"fixture-app\"")),
        device_info_cc: Some(secret("\"fixture-cc\"")), include_device_id: true,
        client_properties: vec![ProfileProperty { key: b"ClientId".to_vec(), value: b"fixture-exact-client-id".to_vec() },
            ProfileProperty { key: vec![0, 0xff], value: vec![0xff, 0] }],
        login_routing: vec![], gateway_uri: secret("\"fixture-exact-gateway-uri\""),
        gateway_metadata: vec![ProfileProperty { key: b"id".to_vec(), value: b"#fixture-gateway".to_vec() },
            ProfileProperty { key: b"domain".to_vec(), value: b"fixture-domain".to_vec() }],
        poll_routing: vec![], logout_routing: vec![],
        incoming: Some(IncomingBinding { object_id: b"#fixture-client".to_vec(), domain: b"fixture-domain".to_vec(),
            receiver_kind: b"fixture.kind".to_vec() }),
        call_incoming:None,
        connect_timeout_ms: 1000, handshake_timeout_ms: 3000, partial_timeout_ms: 300,
        io_timeout_ms: 2000, rpc_timeout_ms: 2000,
    } }
}
fn id(number: u64) -> String { format!("app-account-{number:032x}") }
fn memory() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    initialize_schema(&conn).unwrap();
    conn
}
static NEXT_DB: AtomicU64 = AtomicU64::new(1);
struct TempDb(PathBuf);
impl TempDb {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("simadmin-app-account-{}-{}.db",
            std::process::id(), NEXT_DB.fetch_add(1, Ordering::Relaxed)));
        // Never overwrite an existing unrelated file, even on stale PID reuse.
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        Self(path)
    }
    fn open(&self) -> Connection {
        let conn = Connection::open(&self.0).unwrap();
        conn.busy_timeout(Duration::from_secs(5)).unwrap();
        conn
    }
}
impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut path = self.0.as_os_str().to_os_string(); path.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(path));
        }
    }
}

#[test]
fn schema_is_idempotent_and_does_not_create_sim_trunk_or_readiness_tables() {
    let conn = memory();
    initialize_schema(&conn).unwrap();
    assert!(list(&conn).unwrap().is_empty());
    let names: Vec<String> = conn.prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
        .unwrap().query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(names, ["app_voice_accounts"]);
    let mut statement = conn.prepare("PRAGMA table_info(app_voice_accounts)").unwrap();
    let columns: Vec<String> = statement.query_map([], |row| row.get(1)).unwrap().map(Result::unwrap).collect();
    assert_eq!(columns, ["line_id", "provider_id", "revision", "private_config", "created_at", "updated_at"]);
    assert!(conn.execute("INSERT INTO app_voice_accounts VALUES (?1, 'other', 1, X'00', 't', 't')", [id(1)]).is_err());
    assert!(conn.execute("INSERT INTO app_voice_accounts VALUES (?1, 'app_voice', 0, X'00', 't', 't')", [id(1)]).is_err());
}

#[test]
fn line_ids_accept_only_exact_lowercase_opaque_account_namespace() {
    assert!(valid_line_id(&id(0)));
    assert!(valid_line_id("app-account-0123456789abcdef0123456789abcdef"));
    for invalid in ["", "sim-1", "trunk-1", "app-account-", "app-account-0123456789ABCDEF0123456789abcdef",
        "app-account-0123456789abcdef0123456789abcdeg", "app-account-0123456789abcdef0123456789abcdef0",
        "app-account-0123456789abcdef0123456789abcde", " app-account-0123456789abcdef0123456789abcdef",
        "app-account-0123456789abcdef0123456789abcdef\n", "app-account-０１２３４５６７８９abcdef0123456789abcdef"] {
        assert!(!valid_line_id(invalid));
        let conn = memory();
        assert_eq!(metadata(&conn, invalid).err(), Some("app_account_line_invalid"));
        assert_eq!(load(&conn, invalid).err(), Some("app_account_line_invalid"));
        assert_eq!(delete(&conn, invalid, 1).err(), Some("app_account_line_invalid"));
    }
}

#[test]
fn create_never_upserts_metadata_is_redacted_and_lines_are_independent() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let config = fixture(&cert.root);
    let conn = memory();
    let a = id(1); let b = id(2);
    let first = create(&conn, &a, &config, 100).unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(first.provider_id, super::super::PROVIDER_ID);
    assert_eq!(first.provider_id, "app_voice");
    assert_eq!(first.created_at, "1970-01-01T00:01:40Z");
    assert_eq!(first.updated_at, first.created_at);
    assert_eq!(create(&conn, &a, &config, 101).err(), Some(CONFLICT));
    create(&conn, &b, &config, 100).unwrap();
    let mut replacement = config.clone();
    replacement.profile.client_properties[1].value = vec![1, 0xff, 0];
    let next = replace(&conn, &a, 1, &replacement, 101).unwrap();
    assert_eq!(next.revision, 2);
    assert_eq!(next.created_at, first.created_at);
    assert_eq!(next.updated_at, "1970-01-01T00:01:41Z");
    assert_eq!(load(&conn, &a).unwrap().unwrap().config.profile.client_properties[1].value, [1, 0xff, 0]);
    assert_eq!(load(&conn, &b).unwrap().unwrap().config.profile.client_properties[1].value, [0xff, 0]);
    let rows = list(&conn).unwrap();
    assert_eq!(rows.len(), 2);
    let json = serde_json::to_string(&rows).unwrap();
    let debug = format!("{rows:?}");
    for text in [&json, &debug] {
        for forbidden in ["fixture-", "password", "root_der", "private_config", "identity", "endpoint", "ready", "label"] {
            assert!(!text.contains(forbidden));
        }
    }
    assert_eq!(format!("{:?}", load(&conn, &a).unwrap().unwrap()), "StoredAccount(<redacted>)");
    assert!(metadata(&conn, &id(3)).unwrap().is_none());
    assert!(load(&conn, &id(3)).unwrap().is_none());
}

#[test]
fn strict_cas_missing_accounts_invalid_revisions_and_exhaustion_never_change_rows() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let config = fixture(&cert.root);
    let conn = memory(); let line = id(1);
    assert_eq!(replace(&conn, &line, 1, &config, 100).err(), Some(NOT_FOUND));
    assert_eq!(delete(&conn, &line, 1).err(), Some(NOT_FOUND));
    create(&conn, &line, &config, 100).unwrap();
    for revision in [0, 2, i64::MAX as u64 + 1, u64::MAX] {
        assert_eq!(replace(&conn, &line, revision, &config, 101).err(), Some(CONFLICT));
        assert_eq!(delete(&conn, &line, revision).err(), Some(CONFLICT));
        assert_eq!(metadata(&conn, &line).unwrap().unwrap().revision, 1);
    }
    replace(&conn, &line, 1, &config, 101).unwrap();
    assert_eq!(replace(&conn, &line, 1, &config, 102).err(), Some(CONFLICT));
    assert_eq!(delete(&conn, &line, 1).err(), Some(CONFLICT));
    conn.execute("UPDATE app_voice_accounts SET revision = ?1 WHERE line_id = ?2", params![i64::MAX, line]).unwrap();
    assert_eq!(replace(&conn, &line, i64::MAX as u64, &config, 102).err(), Some(CONFLICT));
    assert_eq!(metadata(&conn, &line).unwrap().unwrap().revision, i64::MAX as u64);
    assert!(delete(&conn, &line, i64::MAX as u64).unwrap());
    assert_eq!(delete(&conn, &line, i64::MAX as u64).err(), Some(NOT_FOUND));
    assert!(list(&conn).unwrap().is_empty());
}

#[test]
fn capacity_is_64_even_across_connections_and_deletion_releases_one_slot() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let config = fixture(&cert.root);
    let temp = TempDb::new(); let a = temp.open(); let b = temp.open();
    initialize_schema(&a).unwrap(); initialize_schema(&b).unwrap();
    for n in 0..MAX_ACCOUNTS as u64 {
        create(if n % 2 == 0 { &a } else { &b }, &id(n), &config, 100).unwrap();
    }
    assert_eq!(list(&a).unwrap().len(), MAX_ACCOUNTS as usize);
    assert_eq!(create(&b, &id(64), &config, 100).err(), Some("app_account_limit"));
    assert_eq!(create(&b, &id(0), &config, 100).err(), Some(CONFLICT));
    assert!(delete(&a, &id(0), 1).unwrap());
    create(&b, &id(64), &config, 100).unwrap();
    assert_eq!(list(&b).unwrap().len(), MAX_ACCOUNTS as usize);
}

#[test]
fn immediate_transactions_serialize_concurrent_capacity_checks() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let config = fixture(&cert.root);
    let temp = TempDb::new(); let conn = temp.open();
    initialize_schema(&conn).unwrap();
    for n in 0..63 { create(&conn, &id(n), &config, 100).unwrap(); }
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [64, 65].into_iter().map(|number| {
        let path = temp.0.clone(); let config = config.clone(); let barrier = barrier.clone();
        std::thread::spawn(move || {
            let conn = Connection::open(path).unwrap(); conn.busy_timeout(Duration::from_secs(5)).unwrap();
            barrier.wait(); create(&conn, &id(number), &config, 100)
        })
    }).collect();
    let results: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|result| result.as_ref().err() == Some(&"app_account_limit")).count(), 1);
    assert_eq!(list(&conn).unwrap().len(), 64);
}

#[test]
fn private_configuration_survives_reopen_without_readiness_state() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let mut config = fixture(&cert.root);
    config.credentials.sdk.client_config_decode_key = Some(secret("\"fixture-client-context\""));
    config.credentials.sdk.reference_decode_key = Some(secret("\"fixture-reference-context\""));
    config.credentials.sdk.expires_at_unix_seconds = Some(u64::MAX);
    let original = config.encode_private().unwrap();
    let temp = TempDb::new();
    {
        let conn = temp.open(); initialize_schema(&conn).unwrap();
        create(&conn, &id(1), &config, 100).unwrap();
    }
    {
        let conn = temp.open(); initialize_schema(&conn).unwrap();
        let loaded = load(&conn, &id(1)).unwrap().unwrap();
        assert_eq!(loaded.metadata.revision, 1);
        assert_eq!(loaded.config.encode_private().unwrap().as_slice(), original.as_slice());
        assert_eq!(loaded.config.credentials.sdk.expires_at_unix_seconds, Some(u64::MAX));
        assert!(delete(&conn, &id(1), 1).unwrap());
    }
    assert!(list(&temp.open()).unwrap().is_empty());
}

#[test]
fn malformed_oversize_and_wrong_type_blobs_fail_closed_but_listing_never_parses_them() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let config = fixture(&cert.root); let conn = memory();
    create(&conn, &id(1), &config, 100).unwrap();
    for blob in [b"".as_slice(), b"{", b"null", b"{\"format_version\":2,\"config\":null}"] {
        conn.execute("UPDATE app_voice_accounts SET private_config = ?1", [blob]).unwrap();
        assert_eq!(load(&conn, &id(1)).err(), Some(CONFIGURATION));
        assert_eq!(list(&conn).unwrap().len(), 1);
        assert_eq!(metadata(&conn, &id(1)).unwrap().unwrap().revision, 1);
    }
    conn.execute("UPDATE app_voice_accounts SET private_config = zeroblob(?1)", [MAX_PRIVATE_BYTES + 1]).unwrap();
    assert_eq!(load(&conn, &id(1)).err(), Some(CONFIGURATION));
    conn.execute("UPDATE app_voice_accounts SET private_config = 'plaintext-not-a-blob'", []).unwrap();
    assert_eq!(load(&conn, &id(1)).err(), Some(CONFIGURATION));
    assert_eq!(list(&conn).unwrap().len(), 1);
    replace(&conn, &id(1), 1, &config, 101).unwrap();
    assert!(load(&conn, &id(1)).unwrap().is_some());
}

#[test]
fn invalid_configuration_and_unrepresentable_timestamp_do_not_persist_or_mutate() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let config = fixture(&cert.root); let conn = memory();
    let mut invalid = config.clone(); invalid.credentials.sdk.selected_session_id = None;
    assert_eq!(create(&conn, &id(1), &invalid, 100).err(), Some(CONFIGURATION));
    assert_eq!(create(&conn, "sim-1", &config, 100).err(), Some("app_account_line_invalid"));
    assert!(list(&conn).unwrap().is_empty());
    create(&conn, &id(1), &config, 100).unwrap();
    assert_eq!(replace(&conn, &id(1), 1, &invalid, 101).err(), Some(CONFIGURATION));
    let mut unknown_expiry = config.clone(); unknown_expiry.credentials.sdk.expires_at_unix_seconds = None;
    assert_eq!(create(&conn, &id(2), &unknown_expiry, u64::MAX).err(), Some(STORAGE));
    assert_eq!(replace(&conn, &id(1), 1, &unknown_expiry, u64::MAX).err(), Some(STORAGE));
    assert_eq!(metadata(&conn, &id(1)).unwrap().unwrap().revision, 1);
    assert_eq!(list(&conn).unwrap().len(), 1);
}

#[test]
fn known_hyphenated_schema_migrates_atomically_to_the_existing_product_provider_id() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let config = fixture(&cert.root);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE app_voice_accounts(
        line_id TEXT PRIMARY KEY NOT NULL, provider_id TEXT NOT NULL CHECK(provider_id='app-voice'),
        revision INTEGER NOT NULL CHECK(revision>0), private_config BLOB NOT NULL,
        created_at TEXT NOT NULL,updated_at TEXT NOT NULL);").unwrap();
    let private = config.encode_private().unwrap();
    conn.execute("INSERT INTO app_voice_accounts VALUES(?1,'app-voice',7,?2,'created','updated')", params![id(1),private.as_slice()]).unwrap();
    initialize_schema(&conn).unwrap(); initialize_schema(&conn).unwrap();
    let stored = load(&conn, &id(1)).unwrap().unwrap();
    assert_eq!(stored.metadata.provider_id, super::super::PROVIDER_ID); assert_eq!(stored.metadata.revision, 7);
    assert_eq!(stored.metadata.created_at, "created"); assert_eq!(stored.metadata.updated_at, "updated");
    assert_eq!(stored.config.encode_private().unwrap().as_slice(), private.as_slice());
    assert!(stored.config.validate(100).is_ok());
    assert_eq!(create(&conn, &id(2), &config, 100).unwrap().provider_id, super::super::PROVIDER_ID);
    let legacy_exists: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='app_voice_accounts_legacy_provider')", [], |row| row.get(0)).unwrap();
    assert!(!legacy_exists);
}

#[test]
fn provider_migration_failure_preserves_old_private_rows_and_unrelated_objects() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let config = fixture(&cert.root);
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE app_voice_accounts(
        line_id TEXT PRIMARY KEY NOT NULL,provider_id TEXT NOT NULL CHECK(provider_id='app-voice'),
        revision INTEGER NOT NULL CHECK(revision>0),private_config BLOB NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL);
        CREATE TABLE app_voice_accounts_legacy_provider(unrelated TEXT);
        INSERT INTO app_voice_accounts_legacy_provider VALUES('must-not-clean');").unwrap();
    let private = config.encode_private().unwrap();
    conn.execute("INSERT INTO app_voice_accounts VALUES(?1,'app-voice',3,?2,'t','t')", params![id(1),private.as_slice()]).unwrap();
    assert_eq!(initialize_schema(&conn), Err(STORAGE));
    let original: (String,i64,Vec<u8>) = conn.query_row("SELECT provider_id,revision,private_config FROM app_voice_accounts WHERE line_id=?1", [id(1)], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(original.0, "app-voice"); assert_eq!(original.1, 3); assert_eq!(original.2, private.as_slice());
    let unrelated: String = conn.query_row("SELECT unrelated FROM app_voice_accounts_legacy_provider", [], |row| row.get(0)).unwrap();
    assert_eq!(unrelated, "must-not-clean");
}

#[test]
fn overcap_corruption_and_missing_schema_return_only_stable_errors() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let config = fixture(&cert.root); let conn = memory();
    let private = config.encode_private().unwrap();
    for n in 0..65 {
        conn.execute("INSERT INTO app_voice_accounts VALUES (?1, 'app_voice', 1, ?2, 't', 't')",
            params![id(n), private.as_slice()]).unwrap();
    }
    assert_eq!(list(&conn).err(), Some("app_account_limit"));
    assert_eq!(create(&conn, &id(65), &config, 100).err(), Some("app_account_limit"));
    let missing = Connection::open_in_memory().unwrap();
    assert_eq!(list(&missing).err(), Some(STORAGE));
    assert_eq!(metadata(&missing, &id(1)).err(), Some(STORAGE));
    assert_eq!(load(&missing, &id(1)).err(), Some(STORAGE));
    assert_eq!(create(&missing, &id(1), &config, 100).err(), Some(STORAGE));
    assert_eq!(replace(&missing, &id(1), 1, &config, 100).err(), Some(STORAGE));
    assert_eq!(delete(&missing, &id(1), 1).err(), Some(STORAGE));
}
