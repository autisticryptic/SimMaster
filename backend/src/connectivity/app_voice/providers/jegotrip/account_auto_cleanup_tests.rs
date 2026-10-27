//! Actions-only opt-in original-run cleanup. No timer can start a new SDK run.
use super::*;
use std::{future::Future, task::Poll};
use super::super::super::{account_config::AccountConfig, transport_packets::CallKind};

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(8), future).await.expect("bounded original automatic cleanup fixture")
}
fn automatic(root: &str) -> AccountConfig {
    let mut config = config(root); config.cleanup_on_disconnect = true;
    config.profile.incoming.as_mut().unwrap().receiver_kind = b"Sip.Request".to_vec(); config
}
fn count(db: &Database, table: &str) -> i64 {
    db.with_conn(|conn| conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0)).map_err(|_| "test_query_failed")).unwrap()
}
async fn sms(peer: &mut super::super::super::runtime_backend::tests::Peer) {
    peer.incoming_with_fields(91, b"Sip.Request", b"fixture-preserved-SMS", Some(b"fixture-sender"), None).await;
    let reply = next_non_poll(peer).await; assert_eq!(reply.kind, CallKind::Reply);
}
async fn finish(mut peer: super::super::super::runtime_backend::tests::Peer) {
    peer.swap(Some(STATUS_UNREGISTER), true).await;
    let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method, super::super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await; assert!(peer.receive().await.is_none());
}
async fn released(manager: &AccountManager, line: &str) -> AccountView {
    bounded(async { loop {
        let view = manager.get(line, 100).await.unwrap();
        if !view.io_cleanup_required { break view; } tokio::task::yield_now().await;
    }}).await
}

async fn original_join_before_restart(cancel_stop: bool) {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (mut first, _, first_dropped) = peer(&cert, &factory);
    let (mut second, _, second_dropped) = peer(&cert, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let line = manager.create(automatic(&cert.root), 100).await.unwrap().account.line_id;
    let (received, receipt) = oneshot::channel(); let (disconnect, disconnected) = oneshot::channel();
    let first_task = tokio::spawn(async move { bounded(async move {
        first.authenticate().await; first.swap(Some(STATUS_REGISTER), true).await;
        sms(&mut first).await; received.send(()).unwrap(); disconnected.await.unwrap(); drop(first);
    }).await });
    bounded(manager.start(&line, 1, 100)).await.unwrap(); bounded(receipt).await.unwrap();
    let slot = lock(&manager.slots).get(&line).unwrap().clone(); let run = lock(&slot.run).clone().unwrap();
    assert!(run.cleanup_on_disconnect); assert!(manager.get(&line, 100).await.unwrap().incoming_message_ready);
    let held_consumer = run.task.lock().await; disconnect.send(()).unwrap(); bounded(first_task).await.unwrap();
    bounded(async { loop {
        let released = run.controller.try_lock().is_ok_and(|controller| controller.resources_released());
        if released && first_dropped.load(Ordering::SeqCst) { break; } tokio::task::yield_now().await;
    }}).await;
    assert!(!run.finalized.load(Ordering::SeqCst)); assert_eq!(count(&temp.database, "app_voice_inbox"), 1);
    assert!(temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
    assert_eq!(manager.start(&line, 1, 100).await.err(), Some("app_account_stop_before_replace"));
    let other_factory = Arc::new(QueuedFactory::default());
    let other = AccountManager::with_factory(Arc::new(Database::new(temp.path.clone()).unwrap()), other_factory.clone());
    assert_eq!(other.start(&line, 1, 100).await.err(), Some("app_account_stop_before_replace"));
    assert_eq!(other_factory.build_count(), 0);
    if cancel_stop {
        let stop = manager.stop(&line, 1, 100); tokio::pin!(stop);
        assert!(std::future::poll_fn(|cx| Poll::Ready(stop.as_mut().poll(cx).is_pending())).await);
    }
    assert!(!run.finalized.load(Ordering::SeqCst)); assert_eq!(factory.build_count(), 1);
    drop(held_consumer);
    let view = released(&manager, &line).await;
    assert_eq!(view.cleanup_on_disconnect, Some(true)); assert!(view.cleanup_error.is_none());
    assert!(!view.incoming_call_ready && !view.incoming_message_ready && !view.outbound_authorized);
    if !cancel_stop { assert_eq!(view.phase, "disconnected"); assert_eq!(view.last_error, Some("app_account_disconnected")); }
    assert_eq!(count(&temp.database, "app_voice_inbox"), 0); assert_eq!(manager.messages(&line, 50).unwrap().len(), 1);
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
    assert_eq!(factory.build_count(), 1, "automatic cleanup never starts the queued replacement connector");
    let second_task = tokio::spawn(async move { bounded(async move {
        second.authenticate().await; second.swap(Some(STATUS_REGISTER), true).await; sms(&mut second).await; finish(second).await;
    }).await });
    // Only this explicit start may create a fresh owner, after the original
    // cleanup worker, actor and consumer have actually joined.
    bounded(manager.start(&line, 1, 100)).await.unwrap();
    bounded(async { while manager.messages(&line, 50).unwrap().len() != 2 { tokio::task::yield_now().await; } }).await;
    assert_eq!(factory.build_count(), 2); let stopped = bounded(manager.stop(&line, 1, 100)).await.unwrap();
    assert!(!stopped.io_cleanup_required); bounded(second_task).await.unwrap(); assert!(second_dropped.load(Ordering::SeqCst));
    assert_eq!(count(&temp.database, "app_voice_inbox"), 0); assert_eq!(count(&temp.database, "sms_messages"), 2);
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
}

#[tokio::test]
async fn opt_in_cleanup_keeps_consumer_join_fence_and_sms_before_explicit_restart() {
    original_join_before_restart(false).await;
}
#[tokio::test]
async fn cancelled_manual_stop_does_not_detach_automatic_cleanup_or_admit_replacement() {
    original_join_before_restart(true).await;
}

#[tokio::test]
async fn automatic_gc_failure_is_visible_and_retries_only_original_cleanup_after_repair() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let (mut peer, connector, dropped) = peer(&cert, &factory);
    let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let line = manager.create(automatic(&cert.root), 100).await.unwrap().account.line_id;
    let (received, receipt) = oneshot::channel(); let (disconnect, disconnected) = oneshot::channel();
    let worker = tokio::spawn(async move { bounded(async move {
        peer.authenticate().await; peer.swap(Some(STATUS_REGISTER), true).await; sms(&mut peer).await;
        received.send(()).unwrap(); disconnected.await.unwrap(); drop(peer);
    }).await });
    bounded(manager.start(&line, 1, 100)).await.unwrap(); bounded(receipt).await.unwrap();
    temp.database.with_conn(|conn| conn.execute_batch("CREATE TRIGGER refuse_auto_gc BEFORE DELETE ON app_voice_inbox BEGIN SELECT RAISE(ABORT,'fixture-private-error');END;")
        .map_err(|_| "test_trigger_failed")).unwrap();
    disconnect.send(()).unwrap(); bounded(worker).await.unwrap();
    let view = bounded(async { loop {
        let view = manager.get(&line, 100).await.unwrap();
        if view.cleanup_error.is_some() { break view; } tokio::task::yield_now().await;
    }}).await;
    assert!(view.io_cleanup_required); assert_eq!(view.phase, "disconnected");
    assert_eq!(view.last_error, Some("app_account_disconnected")); assert_eq!(view.cleanup_error, Some("app_account_run_storage_failed"));
    assert!(dropped.load(Ordering::SeqCst)); assert_eq!(count(&temp.database, "app_voice_inbox"), 1);
    assert!(temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
    assert_eq!(manager.start(&line, 1, 100).await.err(), Some("app_account_stop_before_replace"));
    temp.database.with_conn(|conn| conn.execute_batch("DROP TRIGGER refuse_auto_gc").map_err(|_| "test_trigger_failed")).unwrap();
    let stopped = bounded(manager.stop(&line, 1, 100)).await.unwrap(); assert!(!stopped.io_cleanup_required);
    assert_eq!(count(&temp.database, "app_voice_inbox"), 0); assert_eq!(count(&temp.database, "sms_messages"), 1);
    assert_eq!(connector.connection_calls(), 1); assert_eq!(factory.build_count(), 1);
    assert!(!temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
}

#[tokio::test]
async fn persisted_cleanup_opt_in_never_reclaims_an_unowned_previous_process_run() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default()); let manager = AccountManager::with_factory(temp.database.clone(), factory.clone());
    let line = manager.create(automatic(&cert.root), 100).await.unwrap().account.line_id; let nonce = local_nonce().unwrap();
    // Data-only unknown-owner fixture: no original actor/consumer/Join exists in
    // this manager, so neither its age nor the opt-in setting can prove release.
    temp.database.with_conn(|conn| account_runs::register_runtime(conn, &line, 1, &nonce)).unwrap();
    let reopened = AccountManager::with_factory(Arc::new(Database::new(temp.path.clone()).unwrap()), factory.clone());
    for current in [&manager, &reopened] {
        let view = current.get(&line, 100).await.unwrap(); assert!(view.io_cleanup_required);
        assert_eq!(view.last_error, Some("app_account_runtime_owner_unconfirmed")); assert_eq!(view.cleanup_on_disconnect, None);
        assert_eq!(current.start(&line, 1, 100).await.err(), Some("app_account_stop_before_replace"));
        assert_eq!(current.stop(&line, 1, 100).await.err(), Some("app_account_cleanup_unconfirmed"));
        assert_eq!(current.delete(&line, 1).await.err(), Some("app_account_stop_before_replace"));
    }
    assert_eq!(factory.build_count(), 0);
    assert!(temp.database.with_conn(|conn| account_runs::require_live(conn, &line, 1, &nonce)).is_ok());
    assert!(temp.database.with_conn(|conn| account_runs::ownership_pending(conn, &line)).unwrap());
}
