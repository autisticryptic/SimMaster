//! Actions-only account lifecycle tests. Real memory TLS/Common peers and
//! SQLite, not a fake successful provider or a local/real service execution.
use super::*;
use super::super::{account_test_support::{config, peer, QueuedFactory, TempDatabase},
    rpc_envelope::decode_request, rpc_stream::{Reader, Writer},
    runtime_backend::tests::next_non_poll,
    runtime_connector::{ConnectFuture, ConnectionInput, ConnectedIo, RuntimeConnector},
    session::{AttemptOwner, STATUS_REGISTER, STATUS_UNREGISTER},
    tls_engine::tests::Certificates};
use tokio::sync::oneshot;

#[path = "account_inbox_native_tests.rs"]
mod inbox_native;
#[path = "account_smgw_native_tests.rs"]
mod smgw_native;
#[path = "account_maintenance_native_tests.rs"]
mod maintenance_native;
#[path = "account_auto_cleanup_tests.rs"]
mod auto_cleanup;
#[path = "account_long_reception_tests.rs"]
mod long_direct;
#[path = "account_long_batch_tests.rs"]
mod long_batch;

async fn phase(manager: &AccountManager, line: &str, wanted: &str) -> AccountView {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let state = manager.get(line, 100).await.unwrap();
            if state.phase == wanted { return state; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap()
}
async fn normal_peer(mut peer: super::super::runtime_backend::tests::Peer) {
    peer.authenticate().await;
    peer.swap(Some(STATUS_REGISTER), true).await;
    peer.swap(Some(STATUS_UNREGISTER), true).await;
    let message = next_non_poll(&mut peer).await;
    let request = decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method, super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await;
    assert!(peer.receive().await.is_none());
}

#[tokio::test]
async fn import_list_reopen_and_crud_never_construct_backend_or_restore_ready() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default());
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let created = manager.create(config(&cert.root), 100).await.unwrap(); let line = &created.account.line_id;
    assert!(account_store::valid_line_id(line)); assert!(!line.contains("fixture"));
    assert_eq!(created.phase, "stopped"); assert_eq!(created.account.revision, 1);
    assert_eq!(manager.list(100).await.unwrap().len(), 1);
    assert_eq!(factory.build_count(), 0); assert!(!created.io_cleanup_required);
    assert!(!created.incoming_call_ready && !created.incoming_message_ready && !created.outbound_authorized);
    assert_eq!(created.line_kind, "app_account"); assert_eq!(created.access, "application_data");
    assert_eq!(created.provider_availability, "unsupported");
    assert!(!crate::connectivity::modems::ims::virtual_sim::is_virtual_line(line));
    let json = serde_json::to_string(&created).unwrap();
    for forbidden in ["fixture-", "root_der", "phone_login_token", "SipPwd", "selected_session", "iccid", "imsi", "private_config"] {
        assert!(!json.contains(forbidden));
    }
    let reopen = AccountManager::with_factory(Arc::new(Database::new(temp.path.clone()).unwrap()), factory.clone());
    assert_eq!(reopen.get(line, 100).await.unwrap().phase, "stopped"); assert_eq!(factory.build_count(), 0);
    let next = manager.replace(line, 1, config(&cert.root), 101).await.unwrap(); assert_eq!(next.account.revision, 2);
    assert_eq!(manager.replace(line, 1, config(&cert.root), 102).await.err(), Some("app_account_revision_conflict"));
    assert_eq!(manager.delete(line, 1).await.err(), Some("app_account_revision_conflict"));
    manager.delete(line, 2).await.unwrap(); assert!(manager.list(100).await.unwrap().is_empty());
    assert_eq!(manager.get(line, 100).await.err(), Some("app_account_not_found"));
}

#[tokio::test]
async fn real_native_login_hosted_unregister_logout_and_join_keep_account_independent() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (peer, connector, dropped) = peer(&cert, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let created = manager.create(config(&cert.root), 100).await.unwrap(); let line = &created.account.line_id;
    let serving = tokio::spawn(normal_peer(peer));
    let start = manager.start(line, 1, 100).await.unwrap();
    assert_eq!(start.phase, "sdk_login_pending"); assert!(start.io_cleanup_required);
    let hosted = phase(&manager, line, "hosted").await;
    assert!(!hosted.incoming_call_ready && !hosted.incoming_message_ready && !hosted.outbound_authorized);
    assert_eq!(manager.replace(line, 1, config(&cert.root), 101).await.err(), Some("app_account_stop_before_replace"));
    assert_eq!(manager.delete(line, 1).await.err(), Some("app_account_stop_before_replace"));
    assert_eq!(manager.start(line, 1, 100).await.err(), Some("app_account_stop_before_replace"));
    assert_eq!(connector.connection_calls(), 1); assert_eq!(factory.build_count(), 1);
    let original = connector.captured_scope().unwrap(); assert!(original.is_open());
    let stopped = manager.stop(line, 1, 100).await.unwrap();
    assert_eq!(stopped.phase, "stopped"); assert!(!stopped.io_cleanup_required);
    assert!(!original.is_open()); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
    assert!(!manager.stop(line, 1, 100).await.unwrap().io_cleanup_required);
    assert_eq!(connector.connection_calls(), 1); // no logout/unregister replay
    let replaced = manager.replace(line, 1, config(&cert.root), 101).await.unwrap(); assert_eq!(replaced.account.revision, 2);
    manager.delete(line, 2).await.unwrap();
}

struct HeldConnector { entered: tokio::sync::Notify, scope: Mutex<Option<super::super::rpc_pending::RpcConnectionScope>> }
impl RuntimeConnector for HeldConnector {
    fn connect<'a>(&'a self, _owner: &'a AttemptOwner, scope: &'a super::super::rpc_pending::RpcConnectionScope,
        _input: ConnectionInput<'a>) -> ConnectFuture<'a> {
        Box::pin(async move {
            *lock(&self.scope) = Some(scope.clone()); self.entered.notify_one();
            std::future::pending::<Result<ConnectedIo, super::super::runtime_connector::ConnectError>>().await
        })
    }
}
#[tokio::test]
async fn cancelled_start_retains_original_scope_and_blocks_replacement_until_cleanup() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default());
    let held = Arc::new(HeldConnector { entered: tokio::sync::Notify::new(), scope: Mutex::new(None) }); factory.add(held.clone());
    let manager = Arc::new(AccountManager::with_factory(temp.database.clone(), factory.clone()));
    let line = manager.create(config(&cert.root), 100).await.unwrap().account.line_id;
    let active = manager.clone(); let id = line.clone();
    let starting = tokio::spawn(async move { active.start(&id, 1, 100).await });
    tokio::time::timeout(Duration::from_secs(2), held.entered.notified()).await.unwrap();
    let original = lock(&held.scope).clone().unwrap(); assert!(original.is_open());
    starting.abort(); assert!(starting.await.unwrap_err().is_cancelled()); assert!(!original.is_open());
    assert!(manager.get(&line, 100).await.unwrap().io_cleanup_required);
    assert_eq!(manager.replace(&line, 1, config(&cert.root), 101).await.err(), Some("app_account_stop_before_replace"));
    assert_eq!(manager.delete(&line, 1).await.err(), Some("app_account_stop_before_replace"));
    manager.stop(&line, 1, 100).await.unwrap(); assert!(!manager.get(&line, 100).await.unwrap().io_cleanup_required);
    assert_eq!(factory.build_count(), 1); manager.delete(&line, 1).await.unwrap();
}

#[tokio::test]
async fn stale_revision_invalid_ids_and_missing_accounts_never_admit_a_connection() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let line = manager.create(config(&cert.root), 100).await.unwrap().account.line_id;
    for bad in ["", "virtual-123", "line-modem-1", "app-account-ABCDEFABCDEFABCDEFABCDEFABCDEFABCD"] {
        assert_eq!(manager.start(bad, 1, 100).await.err(), Some("app_account_line_invalid"));
        assert_eq!(manager.stop(bad, 1, 100).await.err(), Some("app_account_line_invalid"));
    }
    let missing = format!("app-account-{}", local_nonce().unwrap());
    assert_eq!(manager.start(&missing, 1, 100).await.err(), Some("app_account_not_found"));
    assert_eq!(manager.start(&line, 2, 100).await.err(), Some("app_account_revision_conflict"));
    assert_eq!(manager.stop(&line, 2, 100).await.err(), Some("app_account_revision_conflict"));
    for revision in [0, u64::MAX] {
        assert_eq!(manager.start(&line, revision, 100).await.err(), Some("app_account_revision_invalid"));
    }
    assert_eq!(factory.build_count(), 0);
    assert_eq!(lock(&manager.slots).len(), 1); // Only known line, not missing/invalid IDs.
}

#[tokio::test]
async fn actual_negative_login_keeps_private_failure_and_requires_original_cleanup() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (mut peer, _connector, dropped) = peer(&cert, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory);
    let line = manager.create(config(&cert.root), 100).await.unwrap().account.line_id;
    let serving = tokio::spawn(async move {
        let message = peer.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        let mut w = Writer::new(); w.boolean(false).unwrap(); w.bytes(b"").unwrap(); w.bytes(b"").unwrap();
        w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); peer.reply(request.call_id, w.finish().expose_to_transport()).await;
        assert!(peer.receive().await.is_none());
    });
    let _ = manager.start(&line, 1, 100).await;
    let state = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let state = manager.get(&line, 100).await.unwrap();
            if state.last_error == Some("app_account_authentication_rejected") { break state; }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.unwrap();
    // A fast rejected reply may race the historical submit receipt. Its actual
    // private reason survives either ordering; it never upgrades to ready.
    assert!(matches!(state.phase, "authentication_failed" | "disconnected"));
    assert_eq!(state.last_error, Some("app_account_authentication_rejected")); assert!(state.io_cleanup_required);
    manager.stop(&line, 1, 100).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
}

#[tokio::test]
async fn cancellation_during_unregister_retains_join_and_cleanup_retry_does_not_replay() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (mut peer, connector, dropped) = peer(&cert, &factory);
    let manager = Arc::new(AccountManager::with_factory(temp.database.clone(), factory));
    let line = manager.create(config(&cert.root), 100).await.unwrap().account.line_id;
    let (entered_tx, entered_rx) = oneshot::channel();
    let serving = tokio::spawn(async move {
        peer.authenticate().await; peer.swap(Some(STATUS_REGISTER), true).await;
        let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
        assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
        entered_tx.send(()).unwrap();
        // Deliberately do not acknowledge unregistration. Cancellation retires
        // the original actor and never resubmits this unknown result.
        // A poll already accepted before cancellation can still be observed;
        // it must contain no repeated unregister/logout operation.
        while let Some(message) = peer.receive().await {
            let request = decode_request(message.payload.expose_to_transport()).unwrap();
            assert_eq!(request.method, super::super::rpc_batch::SWAP_CALLS_METHOD.as_bytes());
            let mut fields = Reader::versioned(request.arguments).unwrap(); fields.bytes().unwrap(); fields.bytes().unwrap();
            let payload = super::super::rpc_batch::read_payload(&mut fields).unwrap();
            assert!(payload.requests.is_empty() && payload.results.is_empty());
        }
    });
    manager.start(&line, 1, 100).await.unwrap(); phase(&manager, &line, "hosted").await;
    let active = manager.clone(); let id = line.clone();
    let stopping = tokio::spawn(async move { active.stop(&id, 1, 100).await });
    tokio::time::timeout(Duration::from_secs(3), entered_rx).await.unwrap().unwrap();
    stopping.abort(); assert!(stopping.await.unwrap_err().is_cancelled());
    let original = connector.captured_scope().unwrap(); assert!(!original.is_open());
    assert!(manager.get(&line, 100).await.unwrap().io_cleanup_required);
    manager.stop(&line, 1, 100).await.unwrap(); assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
    assert_eq!(connector.connection_calls(), 1); assert!(!manager.get(&line, 100).await.unwrap().io_cleanup_required);
}

#[tokio::test]
async fn two_original_accounts_do_not_share_credentials_owner_or_teardown() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default());
    let (peer_a, connector_a, dropped_a) = peer(&cert, &factory);
    let (peer_b, connector_b, dropped_b) = peer(&cert, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory);
    let a = manager.create(config(&cert.root), 100).await.unwrap().account.line_id;
    let b = manager.create(config(&cert.root), 100).await.unwrap().account.line_id; assert_ne!(a, b);
    let first = tokio::spawn(normal_peer(peer_a)); let second = tokio::spawn(normal_peer(peer_b));
    manager.start(&a, 1, 100).await.unwrap(); manager.start(&b, 1, 100).await.unwrap();
    phase(&manager, &a, "hosted").await; phase(&manager, &b, "hosted").await;
    let scope_a = connector_a.captured_scope().unwrap(); let scope_b = connector_b.captured_scope().unwrap();
    assert!(!scope_a.matches(&scope_b));
    manager.stop(&a, 1, 100).await.unwrap(); assert!(!scope_a.is_open()); assert!(scope_b.is_open());
    assert!(dropped_a.load(Ordering::SeqCst)); assert!(!dropped_b.load(Ordering::SeqCst));
    assert_eq!(manager.get(&b, 100).await.unwrap().phase, "hosted");
    manager.stop(&b, 1, 100).await.unwrap(); first.await.unwrap(); second.await.unwrap();
    assert!(dropped_b.load(Ordering::SeqCst));
}

#[tokio::test]
async fn changed_private_revision_retires_old_io_but_keeps_its_cleanup_owner() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (mut peer, connector, dropped) = peer(&cert, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let line = manager.create(config(&cert.root), 100).await.unwrap().account.line_id;
    let serving = tokio::spawn(async move {
        peer.authenticate().await; peer.swap(Some(STATUS_REGISTER), true).await; peer.drain_until_eof().await;
    });
    manager.start(&line, 1, 100).await.unwrap(); phase(&manager, &line, "hosted").await;
    let original = connector.captured_scope().unwrap(); assert!(original.is_open());
    assert_eq!(temp.database.with_conn(|conn| account_store::replace(conn, &line, 1, &config(&cert.root), 101)).err(),Some("app_account_stop_before_replace"));
    // An external database mutation is not an authorized store/API update.
    // Keep this original revision-race regression by injecting it explicitly,
    // while also proving the new production store guard refuses that write.
    temp.database.with_conn(|conn| conn.execute("UPDATE app_voice_accounts SET revision=2 WHERE line_id=?1 AND revision=1",[&line])
        .map(|changed|assert_eq!(changed,1)).map_err(|_|"test_mutation_failed")).unwrap();
    tokio::time::timeout(Duration::from_secs(3), original.closed()).await.unwrap();
    assert_eq!(manager.get(&line, 100).await.err(), Some("app_account_revision_conflict"));
    assert_eq!(manager.start(&line, 2, 100).await.err(), Some("app_account_stop_before_replace"));
    assert_eq!(factory.build_count(), 1);
    // Cleanup uses the original controller/backend, not configuration rev2.
    let released = manager.stop(&line, 2, 100).await.unwrap(); assert!(!released.io_cleanup_required);
    assert!(dropped.load(Ordering::SeqCst)); serving.await.unwrap();
    manager.delete(&line, 2).await.unwrap();
}

#[tokio::test]
async fn failed_storage_does_not_drop_original_configuration_or_mint_runtime_work() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let line = manager.create(config(&cert.root), 100).await.unwrap().account.line_id;
    temp.database.with_conn(|conn| {
        conn.execute_batch("CREATE TRIGGER app_account_fail_update BEFORE UPDATE ON app_voice_accounts
            BEGIN SELECT RAISE(ABORT, 'synthetic-secret-must-not-escape'); END;")
            .map_err(|_| "test_trigger_failed")
    }).unwrap();
    assert_eq!(manager.replace(&line, 1, config(&cert.root), 101).await.err(), Some("app_account_storage_failed"));
    assert_eq!(manager.get(&line, 100).await.unwrap().account.revision, 1); assert_eq!(factory.build_count(), 0);
    assert_eq!(manager.start(&line, 2, 100).await.err(), Some("app_account_revision_conflict"));
}

#[tokio::test]
async fn second_manager_cannot_start_stop_or_mutate_original_account_before_owning_cleanup() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();
    let first_factory=Arc::new(QueuedFactory::default());
    let held=Arc::new(HeldConnector{entered:tokio::sync::Notify::new(),scope:Mutex::new(None)});first_factory.add(held.clone());
    let first=Arc::new(AccountManager::with_factory(temp.database.clone(),first_factory.clone()));
    let line=first.create(config(&cert.root),100).await.unwrap().account.line_id;
    let second_factory=Arc::new(QueuedFactory::default());
    let second=AccountManager::with_factory(Arc::new(Database::new(temp.path.clone()).unwrap()),second_factory.clone());
    let active=first.clone();let id=line.clone();let starting=tokio::spawn(async move{active.start(&id,1,100).await});
    tokio::time::timeout(Duration::from_secs(2),held.entered.notified()).await.unwrap();
    let original=lock(&held.scope).clone().unwrap();assert!(original.is_open());
    let view=second.get(&line,100).await.unwrap();assert!(view.io_cleanup_required);
    assert_eq!(view.last_error,Some("app_account_runtime_owner_unconfirmed"));assert_eq!(view.phase,"disconnected");
    assert_eq!(second.start(&line,1,100).await.err(),Some("app_account_stop_before_replace"));
    assert_eq!(second.replace(&line,1,config(&cert.root),101).await.err(),Some("app_account_stop_before_replace"));
    assert_eq!(second.delete(&line,1).await.err(),Some("app_account_stop_before_replace"));
    assert_eq!(second.stop(&line,1,100).await.err(),Some("app_account_cleanup_unconfirmed"));
    assert!(original.is_open());assert_eq!(second_factory.build_count(),0);
    starting.abort();assert!(starting.await.unwrap_err().is_cancelled());assert!(!original.is_open());
    assert!(temp.database.with_conn(|conn|account_runs::runtime_held(conn,&line)).unwrap());
    assert_eq!(second.start(&line,1,100).await.err(),Some("app_account_stop_before_replace"));
    first.stop(&line,1,100).await.unwrap();assert!(!temp.database.with_conn(|conn|account_runs::runtime_held(conn,&line)).unwrap());
    assert!(!second.get(&line,100).await.unwrap().io_cleanup_required);
    assert_eq!(second.replace(&line,1,config(&cert.root),101).await.unwrap().account.revision,2);
    second.delete(&line,2).await.unwrap();assert_eq!(first_factory.build_count(),1);
}

#[tokio::test]
async fn foreign_manager_stop_never_closes_hosted_original_actor_or_claims_its_join() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());
    let(peer,connector,dropped)=peer(&cert,&factory);let first=AccountManager::with_factory(temp.database.clone(),factory);
    let line=first.create(config(&cert.root),100).await.unwrap().account.line_id;
    let serving=tokio::spawn(normal_peer(peer));first.start(&line,1,100).await.unwrap();phase(&first,&line,"hosted").await;
    let original=connector.captured_scope().unwrap();
    let second=AccountManager::with_factory(Arc::new(Database::new(temp.path.clone()).unwrap()),Arc::new(QueuedFactory::default()));
    assert_eq!(second.stop(&line,1,100).await.err(),Some("app_account_cleanup_unconfirmed"));
    assert!(original.is_open());assert!(!dropped.load(Ordering::SeqCst));
    assert_eq!(second.start(&line,1,100).await.err(),Some("app_account_stop_before_replace"));
    first.stop(&line,1,100).await.unwrap();assert!(!original.is_open());assert!(dropped.load(Ordering::SeqCst));
    assert!(!second.get(&line,100).await.unwrap().io_cleanup_required);serving.await.unwrap();
}

#[tokio::test]
async fn reopened_manager_keeps_legacy_live_run_unconfirmed_instead_of_guessing_process_exit() {
    let cert=Certificates::generate("DNS:fixture.invalid");let temp=TempDatabase::new();let factory=Arc::new(QueuedFactory::default());
    let first=AccountManager::with_factory(temp.database.clone(),factory.clone());
    let line=first.create(config(&cert.root),100).await.unwrap().account.line_id;let nonce=local_nonce().unwrap();
    temp.database.with_conn(|conn|account_runs::register(conn,&line,1,&nonce)).unwrap();
    let second=AccountManager::with_factory(Arc::new(Database::new(temp.path.clone()).unwrap()),factory.clone());
    let view=second.get(&line,100).await.unwrap();assert!(view.io_cleanup_required);
    assert!(!view.incoming_call_ready&&!view.incoming_message_ready&&!view.outbound_authorized);
    assert_eq!(view.last_error,Some("app_account_runtime_owner_unconfirmed"));
    assert_eq!(second.start(&line,1,100).await.err(),Some("app_account_stop_before_replace"));
    assert_eq!(second.stop(&line,1,100).await.err(),Some("app_account_cleanup_unconfirmed"));
    assert_eq!(second.delete(&line,1).await.err(),Some("app_account_stop_before_replace"));
    assert!(temp.database.with_conn(|conn|account_runs::require_live(conn,&line,1,&nonce)).is_ok());assert_eq!(factory.build_count(),0);
}
