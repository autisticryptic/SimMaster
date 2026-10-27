//! Actions-only opt-in bounded reconnect supervisor. Real in-memory TLS/Common
//! peers and SQLite; no service, device, token renewal or session resume.
use super::*;
use std::{future::Future, sync::atomic::AtomicUsize};
use super::super::super::{account_config::{AccountConfig, ReconnectPolicy},
    runtime_backend::tests::Peer, runtime_connector::ConnectError};

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(40), future).await.expect("bounded supervised reconnect fixture")
}
fn supervised(root: &str, max_attempts: u32) -> AccountConfig {
    let mut config = config(root); config.cleanup_on_disconnect = true;
    config.reconnect = Some(ReconnectPolicy { max_attempts, initial_delay_ms: 1000, max_delay_ms: 2000 }); config
}
fn manager(temp: &TempDatabase, factory: &Arc<QueuedFactory>) -> Arc<AccountManager> {
    let manager = Arc::new(AccountManager::with_factory(temp.database.clone(), factory.clone()));
    manager.enable_reconnect_supervisor(); manager
}
fn current(manager: &AccountManager, line: &str) -> Arc<ManagedRun> {
    let slot = lock(&manager.slots).get(line).unwrap().clone(); let run = lock(&slot.run).clone().unwrap(); run
}
async fn wait_view<F: Fn(&AccountView) -> bool>(manager: &AccountManager, line: &str, wanted: F) -> AccountView {
    bounded(async { loop {
        let view = manager.get(line, unix_now()).await.unwrap();
        if wanted(&view) { break view; } tokio::time::sleep(Duration::from_millis(10)).await;
    }}).await
}
async fn finish(mut peer: Peer) {
    peer.swap(Some(STATUS_UNREGISTER), true).await;
    let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method, super::super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await; assert!(peer.receive().await.is_none());
}
struct FailingConnector { calls: AtomicUsize }
impl RuntimeConnector for FailingConnector {
    fn connect<'a>(&'a self, _owner: &'a AttemptOwner, _scope: &'a super::super::super::rpc_pending::RpcConnectionScope,
        _input: ConnectionInput<'a>) -> ConnectFuture<'a> {
        Box::pin(async move { self.calls.fetch_add(1, Ordering::SeqCst); Err(ConnectError::Io) })
    }
}

#[tokio::test]
async fn disconnect_after_full_original_cleanup_starts_one_fresh_run_and_manual_stop_ends_supervision() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default());
    let (mut first, first_connector, first_dropped) = peer(&cert, &factory);
    let (mut second, second_connector, second_dropped) = peer(&cert, &factory);
    let manager = manager(&temp, &factory);
    let line = manager.create(supervised(&cert.root, 3), 100).await.unwrap().account.line_id;
    let (drop_first, first_released) = oneshot::channel(); let (stop_second, second_stopping) = oneshot::channel();
    let first_task = tokio::spawn(async move { bounded(async move {
        first.authenticate().await; first.swap(Some(STATUS_REGISTER), true).await;
        first_released.await.unwrap(); drop(first);
    }).await });
    let second_task = tokio::spawn(async move { bounded(async move {
        second.authenticate().await; second.swap(Some(STATUS_REGISTER), true).await;
        second_stopping.await.unwrap(); finish(second).await;
    }).await });
    bounded(manager.start(&line, 1, unix_now())).await.unwrap();
    let hosted = wait_view(&manager, &line, |view| view.phase == "hosted").await;
    assert_eq!(hosted.reconnect_attempt, Some(1)); assert_eq!(factory.build_count(), 1);
    let original = current(&manager, &line); let original_nonce = original.inbox_nonce.clone();
    drop_first.send(()).unwrap(); bounded(first_task).await.unwrap();
    // The fresh start is admitted only after the retired run finalized its
    // original Joins and fence: never two owners for one account.
    let restarted = wait_view(&manager, &line, |view| view.phase == "hosted" && factory.build_count() == 2).await;
    assert!(original.finalized.load(Ordering::SeqCst)); assert!(first_dropped.load(Ordering::SeqCst));
    let fresh = current(&manager, &line); assert!(!Arc::ptr_eq(&original, &fresh));
    assert_ne!(fresh.inbox_nonce, original_nonce, "a fresh run never inherits the retired original nonce or grants");
    assert_eq!(restarted.reconnect_attempt, Some(1), "own HostingAck resets the failure series");
    assert_eq!(first_connector.connection_calls(), 1); assert_eq!(second_connector.connection_calls(), 1);
    assert!(!restarted.outbound_authorized); assert!(!temp.database.with_conn(|conn| account_runs::require_live(conn, &line, 1, &original_nonce)).is_ok());
    stop_second.send(()).unwrap();
    assert!(!bounded(manager.stop(&line, 1, unix_now())).await.unwrap().io_cleanup_required);
    bounded(second_task).await.unwrap(); assert!(second_dropped.load(Ordering::SeqCst));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(factory.build_count(), 2, "a manual stop is never followed by a supervised restart");
    assert!(lock(&fresh.reconnect_task).is_none());
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
}

#[tokio::test]
async fn rejected_authentication_is_not_retried_even_with_policy_and_spare_connector() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default());
    let (mut rejected, _, dropped) = peer(&cert, &factory); let (_spare, spare_connector, _) = peer(&cert, &factory);
    let manager = manager(&temp, &factory);
    let line = manager.create(supervised(&cert.root, 3), 100).await.unwrap().account.line_id;
    let serving = tokio::spawn(async move { bounded(async move {
        let message = rejected.receive().await.unwrap(); let request = decode_request(message.payload.expose_to_transport()).unwrap();
        let mut w = Writer::new(); w.boolean(false).unwrap(); w.bytes(b"").unwrap(); w.bytes(b"").unwrap();
        w.properties(&[]).unwrap(); w.bytes(b"").unwrap(); rejected.reply(request.call_id, w.finish().expose_to_transport()).await;
        assert!(rejected.receive().await.is_none());
    }).await });
    let _ = manager.start(&line, 1, unix_now()).await;
    let view = wait_view(&manager, &line, |view| view.reconnect_state == Some("not_retryable")).await;
    assert_eq!(view.last_error, Some("app_account_authentication_rejected")); assert!(!view.io_cleanup_required);
    bounded(serving).await.unwrap(); assert!(dropped.load(Ordering::SeqCst));
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(factory.build_count(), 1); assert_eq!(spare_connector.connection_calls(), 0);
    assert!(lock(&current(&manager, &line).reconnect_task).is_none());
}

#[tokio::test]
async fn failed_fresh_starts_are_bounded_then_exhausted_without_extra_builds() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (mut first, _, first_dropped) = peer(&cert, &factory);
    let failing = Arc::new(FailingConnector { calls: AtomicUsize::new(0) });
    for _ in 0..3 { factory.add(failing.clone()); }
    let manager = manager(&temp, &factory);
    let line = manager.create(supervised(&cert.root, 2), 100).await.unwrap().account.line_id;
    let (drop_first, released) = oneshot::channel();
    let first_task = tokio::spawn(async move { bounded(async move {
        first.authenticate().await; first.swap(Some(STATUS_REGISTER), true).await; released.await.unwrap(); drop(first);
    }).await });
    bounded(manager.start(&line, 1, unix_now())).await.unwrap(); wait_view(&manager, &line, |view| view.phase == "hosted").await;
    drop_first.send(()).unwrap(); bounded(first_task).await.unwrap(); assert!(first_dropped.load(Ordering::SeqCst));
    let view = wait_view(&manager, &line, |view| view.reconnect_state == Some("exhausted")).await;
    assert_eq!(view.reconnect_attempt, Some(2)); assert_eq!(view.phase, "disconnected");
    assert_eq!(view.last_error, Some("app_account_disconnected")); assert!(!view.io_cleanup_required);
    assert_eq!(failing.calls.load(Ordering::SeqCst), 2); assert_eq!(factory.build_count(), 3);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(factory.build_count(), 3, "exhaustion requires an explicit user start");
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
    assert!(!bounded(manager.stop(&line, 1, unix_now())).await.unwrap().io_cleanup_required);
}

#[tokio::test]
async fn stop_during_scheduled_delay_cancels_supervised_start_and_unsupervised_manager_never_restarts() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default());
    let (mut first, _, _) = peer(&cert, &factory); let (_spare, spare_connector, _) = peer(&cert, &factory);
    let manager = manager(&temp, &factory);
    let line = manager.create(supervised(&cert.root, 3), 100).await.unwrap().account.line_id;
    let (drop_first, released) = oneshot::channel();
    let first_task = tokio::spawn(async move { bounded(async move {
        first.authenticate().await; first.swap(Some(STATUS_REGISTER), true).await; released.await.unwrap(); drop(first);
    }).await });
    bounded(manager.start(&line, 1, unix_now())).await.unwrap(); wait_view(&manager, &line, |view| view.phase == "hosted").await;
    drop_first.send(()).unwrap(); bounded(first_task).await.unwrap();
    wait_view(&manager, &line, |view| view.reconnect_state == Some("scheduled")).await;
    let stopped = bounded(manager.stop(&line, 1, unix_now())).await.unwrap(); assert!(!stopped.io_cleanup_required);
    assert!(lock(&current(&manager, &line).reconnect_task).is_none());
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(factory.build_count(), 1); assert_eq!(spare_connector.connection_calls(), 0);

    // Same opted-in policy without an owning supervisor: reported, not hidden.
    let temp = TempDatabase::new(); let factory = Arc::new(QueuedFactory::default()); let (mut lone, _, _) = peer(&cert, &factory);
    let unsupervised = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let line = unsupervised.create(supervised(&cert.root, 3), 100).await.unwrap().account.line_id;
    let (drop_lone, released) = oneshot::channel();
    let lone_task = tokio::spawn(async move { bounded(async move {
        lone.authenticate().await; lone.swap(Some(STATUS_REGISTER), true).await; released.await.unwrap(); drop(lone);
    }).await });
    bounded(unsupervised.start(&line, 1, unix_now())).await.unwrap(); wait_view(&unsupervised, &line, |view| view.phase == "hosted").await;
    drop_lone.send(()).unwrap(); bounded(lone_task).await.unwrap();
    wait_view(&unsupervised, &line, |view| view.reconnect_state == Some("unsupervised")).await;
    assert_eq!(factory.build_count(), 1);
}
