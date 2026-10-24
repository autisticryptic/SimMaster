//! Actions-only connector tests. Never called locally, no external SDK/device.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    session::HostingSession, tests::imported, tls_engine::tests::Certificates,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn owner() -> AttemptOwner {
    let mut session = HostingSession::new("fixture-connector-line".into());
    session.import(imported(), 100).unwrap(); session.begin_login(100).unwrap()
}
fn input<'a>(reference: &'a str, root: &'a str) -> ConnectionInput<'a> {
    ConnectionInput { server_reference: reference, authority: "fixture.invalid", root_der_base64: root,
        enable_sni: true, reference_decode_key: None }
}

#[test]
fn connector_configuration_is_explicit_bounded_and_debug_redacted() {
    assert!(TcpRuntimeConnector::new(Duration::ZERO).is_err());
    assert!(TcpRuntimeConnector::new(Duration::from_secs(11)).is_err());
    assert!(TcpRuntimeConnector::new(Duration::from_secs(1)).is_ok());
    let config = input("#fixture:tcp -h fixture.invalid -p 443;", "private-fixture-root");
    assert!(!format!("{config:?}").contains("fixture"));
    assert!(!format!("{:?}", TcpRuntimeConnector::new(Duration::from_secs(1)).unwrap()).contains("443"));
    assert_eq!(validate_input(config), Err(ConnectError::InvalidConfiguration));
}

#[test]
fn direct_tcp_target_does_not_use_domain_or_authority_as_a_port_default() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let target = targets(input("#fixture:tcp -h fixture.invalid -p 443;", &cert.root)).unwrap();
    assert_eq!(target.len(), 1); assert_eq!(target[0].host.as_str(), "fixture.invalid"); assert_eq!(target[0].port, 443);
    for reference in ["#fixture:tcp -h fixture.invalid;", "#fixture:tcp -p 443;", "#fixture:tcp -h fixture.invalid -p 0;"] {
        assert!(targets(input(reference, &cert.root)).is_err());
    }
}

#[test]
fn identity_adapter_locator_and_alternate_transports_are_not_tcp_aliases() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    for reference in ["#fixture", "#fixture@adapter$locator", "#fixture:tls -h fixture.invalid -p 443;",
        "#fixture:udp -h fixture.invalid -p 443;", "#fixture:tcp -h fixture.invalid -p 443;tls -h fixture.invalid -p 443;"] {
        assert_eq!(targets(input(reference, &cert.root)).err(), Some(ConnectError::UnsupportedProfile));
    }
}

#[test]
fn cross_authority_candidate_is_refused_before_any_dns_or_io() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    assert_eq!(targets(input("#fixture:tcp -h other.invalid -p 443;", &cert.root)).err(), Some(ConnectError::UnsupportedProfile));
    let target = targets(input("#fixture:tcp -h FIXTURE.invalid -p 443;", &cert.root)).unwrap();
    assert_eq!(target[0].host.as_str(), "FIXTURE.invalid");
    assert!(same_authority("2001:db8::1", "2001:0db8:0:0:0:0:0:1"));
    assert!(!same_authority("127.0.0.1", "fixture.invalid"));
    assert!(!same_authority("*.fixture.invalid", "fixture.invalid"));
}

#[test]
fn duplicate_candidates_are_bounded_and_preserve_native_port_order() {
    let cert = Certificates::generate("DNS:fixture.invalid");
    let reference = "#fixture:tcp -h fixture.invalid -p 443;tcp -h fixture.invalid -p 443;tcp -h fixture.invalid -p 444;";
    let target = targets(input(reference, &cert.root)).unwrap();
    assert_eq!(target.iter().map(|t| t.port).collect::<Vec<_>>(), vec![443, 444]);
    let too_many = format!("#fixture:{}", "tcp -h fixture.invalid -p 443;".repeat(MAX_ADDRESSES + 1));
    assert_eq!(targets(input(&too_many, &cert.root)).err(), Some(ConnectError::ResourceLimit));
}

#[test]
fn malformed_root_authority_or_unbounded_reference_is_rejected_without_connecting() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let reference = "#fixture:tcp -h fixture.invalid -p 443;";
    for authority in ["", "scheme://fixture.invalid", "fixture.invalid:443", "fixture.invalid.", "*.fixture.invalid"] {
        assert_eq!(validate_input(ConnectionInput { authority, ..input(reference, &cert.root) }), Err(ConnectError::InvalidConfiguration));
    }
    for root in ["", "not_base64", "AA=="] {
        assert_eq!(validate_input(input(reference, root)), Err(ConnectError::InvalidConfiguration));
    }
    assert_eq!(validate_input(input(&"x".repeat(MAX_TARGET_BYTES + 1), &cert.root)), Err(ConnectError::InvalidConfiguration));
}

#[test]
fn encrypted_reference_uses_only_supplied_common_key_not_a_root_or_password_substitute() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let password = b"fixture-common-context";
    let reference = super::super::legacy_config::tests::fixture_envelope(b"#fixture:tcp -h fixture.invalid -p 443;", password, [4; 8]);
    let reference = std::str::from_utf8(&reference).unwrap();
    assert_eq!(targets(input(reference, &cert.root)).err(), Some(ConnectError::InvalidConfiguration));
    let key = ConfigDecodeKey::supplied(password).unwrap();
    let target = targets(ConnectionInput { reference_decode_key: Some(&key), ..input(reference, &cert.root) }).unwrap();
    assert_eq!(target[0].port, 443);
}

#[tokio::test]
async fn retired_scope_is_checked_before_first_dns_or_connect_future_poll() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let scope = RpcConnectionScope::fresh(); let polls = AtomicUsize::new(0); scope.close();
    let operation = poll_fn(|_| { polls.fetch_add(1, Ordering::SeqCst); Poll::Ready(7) });
    assert_eq!(poll_scoped(&scope, operation).await, Err(ConnectError::Scope));
    assert_eq!(polls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn every_pending_connect_poll_rechecks_the_same_original_retirement_gate() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let scope = RpcConnectionScope::fresh(); let polls = AtomicUsize::new(0);
    let operation = poll_fn(|_| { polls.fetch_add(1, Ordering::SeqCst); Poll::<()>::Pending });
    let waiting = poll_scoped(&scope, operation); tokio::pin!(waiting);
    assert!(tokio::time::timeout(Duration::from_millis(10), waiting.as_mut()).await.is_err());
    let before = polls.load(Ordering::SeqCst); assert!(before > 0); scope.close();
    assert_eq!(waiting.await, Err(ConnectError::Scope)); assert_eq!(polls.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn closed_original_scope_cannot_begin_a_dns_or_connect_group() {
    let original = owner(); let scope = RpcConnectionScope::fresh(); scope.close();
    let connector = TcpRuntimeConnector::new(Duration::from_secs(1)).unwrap();
    assert_eq!(connector.connect(&original, &scope, input("not-a-reference", "not-a-root")).await.err(), Some(ConnectError::Scope));
}

#[tokio::test]
async fn connected_duplex_io_is_consumed_only_by_exact_original_owner_scope_and_inputs() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let scope = RpcConnectionScope::fresh();
    let config = input("#fixture:tcp -h fixture.invalid -p 443;", &cert.root);
    for mismatch in 0..6 {
        let (client, mut peer) = tokio::io::duplex(32); let connected = ConnectedIo::original(&original, &scope, config, client).unwrap();
        assert!(!format!("{connected:?}").contains("fixture"));
        let wrong_owner = owner(); let wrong_scope = RpcConnectionScope::fresh();
        let (requested_owner, requested_scope) = match mismatch { 0 => (&wrong_owner, &scope), 1 => (&original, &wrong_scope), _ => (&original, &scope) };
        let requested = match mismatch {
            2 => ConnectionInput { server_reference: "#other:tcp -h fixture.invalid -p 443;", ..config },
            3 => ConnectionInput { authority: "other.invalid", ..config },
            4 => ConnectionInput { enable_sni: false, ..config },
            5 => ConnectionInput { root_der_base64: "AA==", ..config }, _ => config,
        };
        assert_eq!(connected.into_original_io(requested_owner, requested_scope, requested).err(), Some(ConnectError::Scope));
        assert_eq!(peer.read(&mut [0; 1]).await.unwrap(), 0); assert!(scope.is_open()); assert!(wrong_scope.is_open());
    }
    let (client, mut peer) = tokio::io::duplex(32); let connected = ConnectedIo::original(&original, &scope, config, client).unwrap();
    let mut client = connected.into_original_io(&original, &scope, config).unwrap();
    client.write_all(b"fixture").await.unwrap(); let mut data = [0; 7]; peer.read_exact(&mut data).await.unwrap(); assert_eq!(&data, b"fixture");
}

#[tokio::test]
async fn retired_connection_receipt_cannot_attach_owned_io_to_a_later_scope() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let original = owner(); let scope = RpcConnectionScope::fresh();
    let config = input("#fixture:tcp -h fixture.invalid -p 443;", &cert.root);
    let (client, mut peer) = tokio::io::duplex(32); let connected = ConnectedIo::original(&original, &scope, config, client).unwrap(); scope.close();
    assert_eq!(connected.into_original_io(&original, &scope, config).err(), Some(ConnectError::Scope));
    assert_eq!(peer.read(&mut [0; 1]).await.unwrap(), 0);
}

#[tokio::test]
async fn real_loopback_tcp_connector_preserves_original_binding_but_does_not_send_login() {
    // Actions-only loopback listener; no public DNS/Jegotrip/SIM is contacted.
    let cert = Certificates::generate("IP:127.0.0.1"); let original = owner(); let scope = RpcConnectionScope::fresh();
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let reference = format!("#fixture:tcp -h 127.0.0.1 -p {};", listener.local_addr().unwrap().port());
    let config = ConnectionInput { authority: "127.0.0.1", ..input(&reference, &cert.root) };
    let connector = TcpRuntimeConnector::new(Duration::from_secs(1)).unwrap();
    let connected = connector.connect(&original, &scope, config).await.unwrap(); let (mut peer, _) = listener.accept().await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(10), peer.read(&mut [0; 32])).await.is_err());
    let mut io = connected.into_original_io(&original, &scope, config).unwrap();
    io.write_all(b"fixture-original-io").await.unwrap(); let mut data = [0; 19]; peer.read_exact(&mut data).await.unwrap(); assert_eq!(&data, b"fixture-original-io");
}
