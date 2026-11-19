//! Actions-only actual AccountManager controller wait, early resource admission
//! and original deadline/retirement. This is not SDK/SO/device execution.
use super::*;
use std::time::Instant;

#[tokio::test]
async fn send_account_controller_wait_is_charged_before_suspend_and_original_deadline_or_parent_retirement_admits_no_intent() {
    Box::pin(async {
        for retire in [false, true] {
            let cert = Certificates::generate("IP:127.0.0.1");
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await.unwrap();
            let port = listener.local_addr().unwrap().port(); let server = cert.server_config(&TLS13);
            let temp = TempDatabase::new(); let manager = AccountManager::new(temp.database.clone());
            let mut cfg = configuration(&cert.root, port, true);
            cfg.profile.rpc_timeout_ms = 2000;
            let account = manager.create(cfg, 100).await.unwrap().account; let line = account.line_id.clone();
            let (finish, finishing) = tokio::sync::oneshot::channel();
            let serving = tokio::spawn(Box::pin(async move { bounded(async move {
                let mut peer = CommonPeer::open(listener, server).await; login_host(&mut peer).await;
                finishing.await.unwrap();
                // The original strict unhosting/logout helper rejects any
                // SEND/auxiliary/received/user-read business frame or replay.
                finish_account(&mut peer).await;
            }).await }));
            bounded(manager.start(&line, account.revision, 100)).await.unwrap(); phase(&manager, &line, "hosted").await;
            assert_eq!(manager.im_send_resources_for_test(&line).unwrap(), (0, 0));
            let (held, holding_ready) = tokio::sync::oneshot::channel();
            let (release, releasing) = tokio::sync::oneshot::channel();
            let mut holding = Box::pin(manager.im_send_controller_pause_for_test(&line, held, releasing));
            tokio::select! {
                result = holding.as_mut() => panic!("original controller pause not released: {result:?}"),
                result = bounded(holding_ready) => result.unwrap(),
            }
            let started = Instant::now();
            let mut sending = Box::pin(manager.send_im_message_with_admission(&line, account.revision, "1_2", body(false), NONCE, true, true));
            tokio::select! {
                result = sending.as_mut() => panic!("original controller is still held: {result:?}"),
                _ = bounded(async { loop {
                    if manager.im_send_resources_for_test(&line).unwrap().0 == 1 { break; }
                    tokio::task::yield_now().await;
                }}) => {}
            }
            assert!(manager.im_send_resources_for_test(&line).unwrap().1 > 0);
            assert_eq!(rows(&temp.database, "app_voice_im_outbox", &line), 0, "the input reservation is not a SQL intent or native grant");
            if retire { manager.im_send_retire_gate_for_test(&line).unwrap(); }
            let error = bounded(sending).await.err().unwrap();
            assert!(error.not_admitted(), "only this pre-controller stage can promise no new intent");
            assert_eq!(error.code(), if retire { "app_account_im_send_scope_invalid" } else { "app_account_im_send_timeout" });
            if !retire {
                assert!(started.elapsed() >= Duration::from_secs(2));
                assert!(started.elapsed() < Duration::from_secs(3), "controller wait cannot renew the original two-second send deadline");
            }
            assert_eq!(manager.im_send_resources_for_test(&line).unwrap(), (0, 0));
            assert_eq!(rows(&temp.database, "app_voice_im_outbox", &line), 0);
            assert!(manager.im_send_history(&line, 128).unwrap().is_empty());
            release.send(()).unwrap(); bounded(holding).await.unwrap();
            assert_eq!(manager.get(&line, 100).await.unwrap().phase, "hosted");
            no_other_business(&manager, &temp.database, &line);
            finish.send(()).unwrap();
            assert!(!bounded(manager.stop(&line, account.revision, 100)).await.unwrap().io_cleanup_required);
            bounded(serving).await.unwrap(); released(&temp.database, &line);
            assert_eq!(rows(&temp.database, "app_voice_im_outbox", &line), 0);
        }
    }).await;
}
