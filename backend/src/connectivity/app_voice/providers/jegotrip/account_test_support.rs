//! Shared Actions-only account fixtures. Synthetic credentials/certificates and
//! in-memory native TLS/Common peers; no user secrets or service connections.
use super::{account_config::AccountConfig, account_runtime::AccountBackendFactory,
    runtime_backend::{BackendEvent, NativeHostingBackend}, runtime_connector::RuntimeConnector,
    tls_engine::tests::Certificates};
use crate::platform::db::Database;
use std::{collections::VecDeque, path::PathBuf, sync::{atomic::{AtomicU64, AtomicUsize, Ordering}, Arc, Mutex}};
use tokio::sync::mpsc;

pub(crate) fn config_json(root: &str) -> serde_json::Value {
    serde_json::json!({
        "credentials": {
            "sdk": { "account_identity": "fixture-account", "phone_login_token": "fixture-original-pwd",
                "application_key": "fixture-original-appkey", "server_address": "#fixture-account:tcp -h fixture.invalid -p 443;",
                "selected_session_id": "fixture-original-selector", "device_id": "fixture-original-device",
                "root_der_base64": root, "tls_only": true, "expires_at_unix_seconds": null },
            "hosting": { "sip_number": "fixture-number", "sip_password": "fixture-SipPwd", "authname_prefix": "fixture-prefix",
                "channel_id": "fixture-channel", "delete_int_code": "1", "enrollment_confirmed": true }
        },
        "profile": {
            "route": "direct_tls_carrier", "authority": "fixture.invalid", "enable_sni": true,
            "identity_type": "username", "domain_name": "fixture.invalid", "domain_id": "fixture-domain",
            "application_id": "fixture-app", "device_info_cc": "fixture-cc", "include_device_id": true,
            "client_properties": [{"key": b"ClientId", "value": b"fixture-exact-client-id"}],
            "login_routing": [], "gateway_uri": "fixture-exact-gateway-uri",
            "gateway_metadata": [{"key": b"id", "value": b"#fixture-gateway"}, {"key": b"domain", "value": b"fixture-domain"}],
            "poll_routing": [], "logout_routing": [],
            "incoming": {"object_id": b"#fixture-client", "domain": b"fixture-domain", "receiver_kind": b"fixture.kind"},
            "connect_timeout_ms": 1000, "handshake_timeout_ms": 3000, "partial_timeout_ms": 300,
            "io_timeout_ms": 2000, "rpc_timeout_ms": 2000
        }
    })
}
pub(crate) fn config(root: &str) -> AccountConfig { serde_json::from_value(config_json(root)).unwrap() }

#[derive(Default)]
pub(crate) struct QueuedFactory {
    connectors: Mutex<VecDeque<Arc<dyn RuntimeConnector>>>,
    builds: AtomicUsize,
}
impl QueuedFactory {
    pub(crate) fn add(&self, connector: Arc<dyn RuntimeConnector>) { self.connectors.lock().unwrap().push_back(connector); }
    pub(crate) fn build_count(&self) -> usize { self.builds.load(Ordering::SeqCst) }
}
impl AccountBackendFactory for QueuedFactory {
    fn build(&self, config: &AccountConfig) -> Result<(Arc<NativeHostingBackend>, mpsc::Receiver<BackendEvent>), &'static str> {
        let profile = config.runtime_profile()?;
        self.builds.fetch_add(1, Ordering::SeqCst);
        let connector = self.connectors.lock().unwrap().pop_front().ok_or("app_account_backend_unavailable")?;
        Ok(NativeHostingBackend::new(profile, connector))
    }
}
pub(crate) fn peer(cert: &Certificates, factory: &QueuedFactory) -> (super::runtime_backend::tests::Peer,
    Arc<super::runtime_backend::tests::DuplexConnector>, Arc<std::sync::atomic::AtomicBool>) {
    let (unused, events, connector, peer, dropped, _) = super::runtime_backend::tests::fixture(cert, false);
    // Unstarted fixture backend owns no actor/IO. The new factory constructs
    // its backend from the ACTUAL stored account profile, not a fixture profile.
    drop(unused); drop(events); factory.add(connector.clone()); (peer, connector, dropped)
}

static NEXT_DB: AtomicU64 = AtomicU64::new(1);
pub(crate) struct TempDatabase { pub database: Arc<Database>, pub path: PathBuf }
impl TempDatabase {
    pub(crate) fn new() -> Self {
        let nonce = super::account_runtime::local_nonce().unwrap();
        let path = std::env::temp_dir().join(format!("simadmin-app-runtime-{}-{}-{nonce}.db",
            std::process::id(), NEXT_DB.fetch_add(1, Ordering::Relaxed)));
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
        Self { database: Arc::new(Database::new(path.clone()).unwrap()), path }
    }
}
impl Drop for TempDatabase {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        for suffix in ["-journal", "-wal", "-shm"] {
            let mut path = self.path.as_os_str().to_os_string(); path.push(suffix);
            let _ = std::fs::remove_file(PathBuf::from(path));
        }
    }
}
