//! Real worker request correlation with synthetic execution. Actions-only;
//! neither a Linux namespace nor a kernel XFRM operation is performed.
use super::*;
use crate::services::ue_worker::UeWorkerHandle;
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};

type Batches = Arc<Mutex<Vec<Vec<NetConfigOp>>>>;

fn worker() -> UeWorkerHandle {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let line = format!("xfrm-binding-{}", NEXT.fetch_add(1, Ordering::SeqCst));
    UeWorkerHandle::for_line(&line, crate::platform::netns::NetnsName::for_line(
        crate::platform::netns::DEFAULT_NAMESPACE_PREFIX, &line))
}
fn plan(transport: XfrmTransport) -> XfrmInstallPlan {
    let ue = SecAgree { spi_c: 0x1111, spi_s: 0x2222, port_c: 6000, port_s: 6001 };
    let peer = SecAgree { spi_c: 0x3333, spi_s: 0x4444, port_c: 7000, port_s: 7001 };
    build_install_plan_for_transport("192.0.2.2".parse().unwrap(), "192.0.2.1".parse().unwrap(),
        &ue, &peer, &[0x11; 16], &[0x22; 16], XfrmAlgs { auth: "hmac(sha1)", auth_trunc_bits: 96, enc: "cbc(aes)" }, transport).unwrap()
}
async fn recording(worker: &UeWorkerHandle) -> Batches {
    let batches = Arc::new(Mutex::new(vec![]));
    let recorded = batches.clone();
    worker.enable_test_net_config_handler(move |ops| {
        let recorded = recorded.clone();
        async move { recorded.lock().unwrap().push(ops); Ok(()) }
    }).await;
    batches
}
fn check_commands(batch: &[NetConfigOp], action: &str, best_effort: bool, count: usize) {
    assert_eq!(batch.len(), count);
    for op in batch {
        let NetConfigOp::Xfrm { args, best_effort: actual } = op else { panic!("non-XFRM operation") };
        assert_eq!(args.first().map(String::as_str), Some("xfrm"));
        assert_eq!(args.get(2).map(String::as_str), Some(action));
        assert!(!args.iter().any(|arg| arg == "flush"));
        assert_eq!(*actual, best_effort);
        if action == "delete" { assert!(!args.iter().any(|arg| arg == "tmpl" || arg == "auth-trunc" || arg == "enc")); }
    }
}

#[tokio::test]
async fn captured_binding_installs_and_removes_only_its_udp_or_tcp_plan() {
    for transport in [XfrmTransport::Udp, XfrmTransport::Tcp] {
        let worker = worker(); let batches = recording(&worker).await; let binding = worker.bind();
        let plan = plan(transport);
        install_plan_in_worker(&plan, &binding).await.unwrap();
        uninstall_plan_in_worker(&plan, &binding).await;
        let batches = batches.lock().unwrap(); assert_eq!(batches.len(), 2);
        let count = plan.states.len() + plan.policies.len();
        check_commands(&batches[0], "add", false, count);
        check_commands(&batches[1], "delete", true, count);
        // Policy deletion is reversed and exact; no deletion template/key can
        // turn a failed registration into a broad state/policy flush.
        for (operation, policy) in batches[1].iter().zip(plan.policies.iter().rev()) {
            let NetConfigOp::Xfrm { args, .. } = operation else { unreachable!() };
            let mut expected = policy.clone(); expected[2] = "delete".into();
            if let Some(at) = expected.iter().position(|value| value == "tmpl") { expected.truncate(at); }
            assert_eq!(*args, expected);
        }
    }
}

#[tokio::test]
async fn previously_checked_binding_cannot_install_or_delete_after_respawn() {
    let worker = worker(); let batches = recording(&worker).await; let captured = worker.bind();
    assert!(captured.is_current()); // The old outer call-site check passed.
    worker.advance_test_generation().await;
    let plan = plan(XfrmTransport::Udp);
    let error = install_plan_in_worker(&plan, &captured).await.unwrap_err();
    assert_eq!(error.code(), code::RUNTIME_UE_WORKER_GENERATION_CHANGED);
    uninstall_plan_in_worker(&plan, &captured).await;
    assert!(batches.lock().unwrap().is_empty(), "no operation may reach the replacement process");
    // A separately admitted new registration can use the current binding.
    let fresh = worker.bind();
    install_plan_in_worker(&plan, &fresh).await.unwrap();
    uninstall_plan_in_worker(&plan, &fresh).await;
    assert_eq!(batches.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn generation_change_between_preflight_and_enqueue_blocks_both_operations() {
    for install in [true, false] {
        let worker = worker(); let batches = recording(&worker).await; let captured = worker.bind();
        let held = captured.lock_current_generation().await.unwrap();
        let (queued, queued_rx) = tokio::sync::oneshot::channel();
        let advancing_worker = worker.clone();
        let advance = tokio::spawn(async move {
            let _ = queued.send(());
            advancing_worker.advance_test_generation().await;
        });
        queued_rx.await.unwrap(); // The first task now waits for the lifecycle lock.
        let old = captured.clone();
        let (started, started_rx) = tokio::sync::oneshot::channel();
        let operation = tokio::spawn(async move {
            let plan = plan(XfrmTransport::Udp);
            assert!(old.is_current());
            let _ = started.send(());
            if install { install_plan_in_worker(&plan, &old).await.map_err(|error| error.code()) }
            else { uninstall_plan_in_worker(&plan, &old).await; Ok(()) }
        });
        started_rx.await.unwrap();
        // Tokio's FIFO lifecycle mutex advances the process before the queued
        // config request. Only the captured-generation enqueue check can stop it.
        drop(held);
        advance.await.unwrap();
        let result = operation.await.unwrap();
        if install { assert_eq!(result, Err(code::RUNTIME_UE_WORKER_GENERATION_CHANGED)); }
        else { assert_eq!(result, Ok(())); }
        assert!(batches.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn late_install_response_cannot_succeed_or_roll_back_on_a_new_generation() {
    for response_ok in [true, false] {
        let worker = worker(); let captured = worker.bind();
        let batches: Batches = Arc::new(Mutex::new(vec![]));
        let recorded = batches.clone(); let changing = worker.clone();
        worker.enable_test_net_config_handler(move |ops| {
            let recorded = recorded.clone(); let changing = changing.clone();
            async move {
                recorded.lock().unwrap().push(ops);
                changing.advance_test_generation().await;
                if response_ok { Ok(()) } else { Err("old-partial-install".into()) }
            }
        }).await;
        let plan = plan(XfrmTransport::Tcp);
        assert_eq!(install_plan_in_worker(&plan, &captured).await.unwrap_err().code(), code::RUNTIME_UE_WORKER_GENERATION_CHANGED);
        uninstall_plan_in_worker(&plan, &captured).await;
        let batches = batches.lock().unwrap();
        assert_eq!(batches.len(), 1, "neither rollback nor a repeated install may target the new epoch");
        check_commands(&batches[0], "add", false, plan.states.len() + plan.policies.len());
    }
}

#[tokio::test]
async fn partial_install_rolls_back_same_binding_without_masking_original_failure() {
    let worker = worker(); let captured = worker.bind(); let batches: Batches = Arc::new(Mutex::new(vec![]));
    let recorded = batches.clone();
    worker.enable_test_net_config_handler(move |ops| {
        let recorded = recorded.clone();
        async move {
            let count = { let mut batches = recorded.lock().unwrap(); batches.push(ops); batches.len() };
            if count == 1 { Err("synthetic-original-install-failure".into()) }
            else { Err("synthetic-cleanup-failure".into()) }
        }
    }).await;
    let plan = plan(XfrmTransport::Udp);
    let error = install_plan_in_worker(&plan, &captured).await.unwrap_err();
    assert_eq!(error.code(), code::COMMAND_FAILED);
    assert!(error.to_string().contains("synthetic-original-install-failure"));
    assert!(!error.to_string().contains("synthetic-cleanup-failure"));
    let batches = batches.lock().unwrap(); assert_eq!(batches.len(), 2);
    check_commands(&batches[1], "delete", true, plan.states.len() + plan.policies.len());
}
