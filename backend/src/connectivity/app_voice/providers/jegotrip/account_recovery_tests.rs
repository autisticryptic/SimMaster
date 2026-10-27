//! Actions-only OS-lock-proven orphan recovery. A dropped manager stands in for
//! an exited process; its flock closes with its file. No PID/age inference,
//! service, device or local compiled execution.
use super::*;
use std::future::Future;
use super::super::super::transport_packets::CallKind;

async fn bounded<F: Future>(future: F) -> F::Output {
    tokio::time::timeout(Duration::from_secs(20), future).await.expect("bounded orphan recovery fixture")
}
fn lock_path(temp: &TempDatabase) -> std::path::PathBuf {
    let mut path = temp.path.clone().into_os_string(); path.push(".lock"); path.into()
}
fn smgw(root: &str) -> super::super::super::account_config::AccountConfig {
    let mut config = config(root); config.profile.incoming.as_mut().unwrap().receiver_kind = b"Sip.Request".to_vec(); config
}
fn count(db: &Database, sql: &str) -> i64 {
    db.with_conn(|conn| conn.query_row(sql, [], |row| row.get(0)).map_err(|_| "test_query_failed")).unwrap()
}
async fn finish(mut peer: super::super::super::runtime_backend::tests::Peer) {
    peer.swap(Some(STATUS_UNREGISTER), true).await;
    let message = next_non_poll(&mut peer).await; let request = decode_request(message.payload.expose_to_transport()).unwrap();
    assert_eq!(request.method, super::super::super::rpc_logout::LOGOUT_SESSION_METHOD.as_bytes());
    peer.reply(request.call_id, &[0, 1, 0, 0, 0, 0]).await; assert!(peer.receive().await.is_none());
}

#[tokio::test]
async fn exited_lock_holder_run_is_released_open_call_ended_sending_notice_uncertain_and_sms_kept() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let first_factory = Arc::new(QueuedFactory::default()); let (mut first, _, first_dropped) = peer(&cert, &first_factory);
    let first_manager = AccountManager::with_factory(temp.database.clone(), first_factory.clone());
    assert_eq!(first_manager.claim_process_ownership(&lock_path(&temp)).unwrap(), RecoveryReport::default());
    let line = first_manager.create(smgw(&cert.root), 100).await.unwrap().account.line_id;
    let (received, receipt) = oneshot::channel(); let (gone, exited) = oneshot::channel();
    let serving = tokio::spawn(async move { bounded(async move {
        first.authenticate().await; first.swap(Some(STATUS_REGISTER), true).await;
        first.incoming_with_fields(91, b"Sip.Request", b"fixture-survives-crash", Some(b"fixture-sender"), None).await;
        assert_eq!(next_non_poll(&mut first).await.kind, CallKind::Reply);
        received.send(()).unwrap(); exited.await.unwrap(); drop(first);
    }).await });
    bounded(first_manager.start(&line, 1, 100)).await.unwrap(); bounded(receipt).await.unwrap();
    let dead_nonce = current_nonce(&first_manager, &line);
    assert_eq!(count(&temp.database, "SELECT COUNT(*) FROM app_voice_runtime_leases WHERE holder IS NOT NULL"), 1);
    // The previous process exits without stop: its lease, raw envelope and lock
    // file descriptor are all that remain; nothing performed a clean release.
    drop(first_manager); gone.send(()).unwrap(); bounded(serving).await.unwrap(); assert!(first_dropped.load(Ordering::SeqCst));
    let call: i64 = temp.database.with_conn(|conn| { conn.execute("INSERT INTO call_history(line_id,direction,phone_number,duration,start_time,answered,app_call_token)
        VALUES(?1,'incoming','fixture-caller',0,'2026-01-01T00:00:00Z',1,'00000000000000000000000000000001')", [&line]).map_err(|_| "test_insert_failed")?;
        Ok(conn.last_insert_rowid()) }).unwrap();
    let sms: i64 = count(&temp.database, "SELECT id FROM sms_messages LIMIT 1");
    temp.database.with_conn(|conn| conn.execute("INSERT INTO app_voice_notification_jobs(sms_id,line_id,rule_id,channel_id,fingerprint,title,body,state,claim,attempts,not_before,expires_at,created_at)
        VALUES(?1,?2,'rule','channel',?3,'t','b','sending','dead-claim',1,0,4102444800,0)",
        rusqlite::params![sms, line, "a".repeat(64)]).map_err(|_| "test_insert_failed")).unwrap();
    let second_factory = Arc::new(QueuedFactory::default()); let (second, _, second_dropped) = peer(&cert, &second_factory);
    let reopened = Arc::new(Database::new(temp.path.clone()).unwrap());
    let manager = AccountManager::with_factory(reopened.clone(), second_factory.clone());
    assert_eq!(manager.get(&line, 100).await.unwrap().last_error, Some("app_account_runtime_owner_unconfirmed"));
    let report = manager.claim_process_ownership(&lock_path(&temp)).unwrap();
    assert_eq!(report, RecoveryReport { recovered_runs: 1, uncertain_notifications: 1, legacy_unconfirmed_leases: 0 });
    assert_eq!(count(&reopened, "SELECT COUNT(*) FROM app_voice_runtime_leases"), 0);
    assert!(reopened.with_conn(|conn| account_runs::require_live(conn, &line, 1, &dead_nonce)).is_err(), "dead nonce never admits writes again");
    let ended: Option<String> = reopened.with_conn(|conn| conn.query_row("SELECT end_time FROM call_history WHERE id=?1", [call], |row| row.get(0)).map_err(|_| "q")).unwrap();
    assert!(ended.is_some());
    assert_eq!(count(&reopened, "SELECT COUNT(*) FROM app_events WHERE event_type='call.ended' AND payload_json LIKE '%\"recovered\":true%'"), 1);
    assert_eq!(count(&reopened, "SELECT COUNT(*) FROM app_events WHERE event_type='app_voice.runtime_recovered'"), 1);
    assert_eq!(count(&reopened, "SELECT COUNT(*) FROM app_voice_notification_jobs WHERE state='uncertain' AND title='' AND body=''"), 1);
    assert_eq!(count(&reopened, "SELECT COUNT(*) FROM app_voice_inbox"), 0); assert_eq!(manager.messages(&line, 10).unwrap().len(), 1);
    let view = manager.get(&line, 100).await.unwrap(); assert!(!view.io_cleanup_required); assert!(view.last_error.is_none());
    assert_eq!(second_factory.build_count(), 0, "recovery never starts a session");
    let serving = tokio::spawn(async move { bounded(async move {
        let mut second = second; second.authenticate().await; second.swap(Some(STATUS_REGISTER), true).await; finish(second).await;
    }).await });
    bounded(manager.start(&line, 1, 100)).await.unwrap();
    bounded(async { loop { if manager.get(&line, 100).await.unwrap().phase == "hosted" { break; } tokio::time::sleep(Duration::from_millis(5)).await; } }).await;
    assert_ne!(current_nonce(&manager, &line), dead_nonce);
    assert!(!bounded(manager.stop(&line, 1, 100)).await.unwrap().io_cleanup_required); bounded(serving).await.unwrap();
    assert!(second_dropped.load(Ordering::SeqCst));
}
fn current_nonce(manager: &AccountManager, line: &str) -> String {
    let slot = lock(&manager.slots).get(line).unwrap().clone(); let nonce = lock(&slot.run).as_ref().unwrap().inbox_nonce.clone(); nonce
}

#[tokio::test]
async fn live_lock_owner_blocks_second_claim_and_untagged_legacy_lease_stays_unconfirmed() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let owner = AccountManager::with_factory(temp.database.clone(), Arc::new(QueuedFactory::default()));
    owner.claim_process_ownership(&lock_path(&temp)).unwrap();
    assert_eq!(owner.claim_process_ownership(&lock_path(&temp)).err(), Some("app_account_process_claim_present"));
    let line = owner.create(smgw(&cert.root), 100).await.unwrap().account.line_id;
    // A run tagged by a live foreign instance, plus an untagged legacy run.
    let other = owner.create(smgw(&cert.root), 100).await.unwrap().account.line_id;
    temp.database.with_conn(|conn| account_runs::register_runtime_held(conn, &line, 1, &local_nonce().unwrap(), Some(&"b".repeat(32)))).unwrap();
    temp.database.with_conn(|conn| account_runs::register_runtime(conn, &other, 1, &local_nonce().unwrap())).unwrap();
    let rival = AccountManager::with_factory(Arc::new(Database::new(temp.path.clone()).unwrap()), Arc::new(QueuedFactory::default()));
    assert_eq!(rival.claim_process_ownership(&lock_path(&temp)).err(), Some("app_account_process_lock_owned"));
    assert_eq!(count(&temp.database, "SELECT COUNT(*) FROM app_voice_runtime_leases"), 2, "a live lock holder's leases are never touched");
    assert!(temp.database.with_conn(|conn| account_runs::register_runtime_held(conn, &line, 1, &local_nonce().unwrap(), Some("not-a-nonce"))).is_err());
    drop(owner);
    let successor = AccountManager::with_factory(Arc::new(Database::new(temp.path.clone()).unwrap()), Arc::new(QueuedFactory::default()));
    let report = successor.claim_process_ownership(&lock_path(&temp)).unwrap();
    assert_eq!(report, RecoveryReport { recovered_runs: 1, uncertain_notifications: 0, legacy_unconfirmed_leases: 1 });
    assert!(successor.get(&line, 100).await.unwrap().last_error.is_none());
    assert_eq!(successor.get(&other, 100).await.unwrap().last_error, Some("app_account_runtime_owner_unconfirmed"),
        "untagged legacy holders are never inferred to have exited");
    assert_eq!(successor.start(&other, 1, 100).await.err(), Some("app_account_stop_before_replace"));
}

#[tokio::test]
async fn claim_after_a_local_start_is_refused_and_unclaimed_managers_never_recover() {
    let cert = Certificates::generate("DNS:fixture.invalid"); let temp = TempDatabase::new();
    let factory = Arc::new(QueuedFactory::default());
    let held = Arc::new(HeldConnector { entered: tokio::sync::Notify::new(), scope: Mutex::new(None) }); factory.add(held.clone());
    let manager = Arc::new(AccountManager::with_factory(temp.database.clone(), factory));
    let line = manager.create(smgw(&cert.root), 100).await.unwrap().account.line_id;
    let active = manager.clone(); let id = line.clone();
    let starting = tokio::spawn(async move { active.start(&id, 1, 100).await });
    bounded(held.entered.notified()).await;
    assert_eq!(manager.claim_process_ownership(&lock_path(&temp)).err(), Some("app_account_claim_after_start"));
    assert_eq!(count(&temp.database, "SELECT COUNT(*) FROM app_voice_runtime_leases WHERE holder IS NULL"), 1, "unclaimed manager tags nothing");
    starting.abort(); let _ = starting.await; manager.stop(&line, 1, 100).await.unwrap();
}
