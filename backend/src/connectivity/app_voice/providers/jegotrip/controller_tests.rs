use super::*;
use crate::connectivity::app_voice::providers::jegotrip::{
    native_events::{NativeNotification, NativeOperation}, tests::imported,
};
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Operation { LoginToken(String), Status(i32, String), Logout }

struct MockBackend {
    operations: Mutex<Vec<Operation>>,
    fail_unregister: bool,
}

impl MockBackend {
    fn new(fail_unregister: bool) -> Arc<Self> {
        Arc::new(Self { operations: Mutex::new(vec![]), fail_unregister })
    }
}

impl HostingBackend for MockBackend {
    fn login<'a>(&'a self, _: AttemptOwner, request: SdkLoginRequest<'a>) -> BackendFuture<'a> {
        Box::pin(async move {
            self.operations.lock().unwrap().push(Operation::LoginToken(request.phone_login_token.into()));
            Ok(())
        })
    }
    fn update_status<'a>(&'a self, _: AttemptOwner, status: i32, request: GatewayRegistration<'a>) -> BackendFuture<'a> {
        Box::pin(async move {
            self.operations.lock().unwrap().push(Operation::Status(status, request.sip_password.into()));
            if self.fail_unregister && status == STATUS_UNREGISTER { Err(BackendFailure::Disconnected) }
            else { Ok(()) }
        })
    }
    fn logout(&self, _: AttemptOwner) -> BackendFuture<'_> {
        Box::pin(async move {
            self.operations.lock().unwrap().push(Operation::Logout);
            Ok(())
        })
    }
}

fn notification(name: &str, operation: Option<NativeOperation>) -> NativeNotification<'_> {
    NativeNotification { name, operation, message: None, from: None, time: None }
}

async fn hosted(controller: &mut HostingController) -> AttemptOwner {
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.on_callback(&owner, Callback::SdkLogin { success: true }, 100).await.unwrap();
    controller.on_callback(&owner, Callback::HostingAck { success: true }, 100).await.unwrap();
    controller.on_callback(&owner, Callback::Channels { calls: true, messages: true }, 100).await.unwrap();
    owner
}

#[tokio::test]
async fn native_message_requires_hosting_channel_and_rejects_old_owner_after_stop() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    let message = || NativeNotification { name: "MtcSmgwMessageRecvNotification",
        operation: None, message: Some(&[0xff, 0x00]), from: None, time: None };
    assert!(controller.on_native_notification(&owner, message(), 100).await.unwrap().is_none());
    controller.on_callback(&owner, Callback::SdkLogin { success: true }, 100).await.unwrap();
    let ack = notification("MtcSmgwUpdateStatusOkNotification", Some(NativeOperation::HostingRegister));
    controller.on_native_notification(&owner, ack, 100).await.unwrap();
    assert!(controller.on_native_notification(&owner, message(), 100).await.unwrap().is_none());
    controller.on_callback(&owner, Callback::Channels { calls: false, messages: true }, 100).await.unwrap();
    let received = controller.on_native_notification(&owner, message(), 100).await.unwrap().unwrap();
    assert_eq!(received.body, [0xff, 0]);
    controller.stop().await.unwrap();
    assert!(controller.on_native_notification(&owner, message(), 100).await.unwrap().is_none());
}

#[tokio::test]
async fn send_success_notification_does_not_promote_pending_hosting() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.on_callback(&owner, Callback::SdkLogin { success: true }, 100).await.unwrap();
    let n = notification("MtcSmgwSendMessageOkNotification", None);
    controller.on_native_notification(&owner, n, 100).await.unwrap();
    assert_eq!(controller.phase(), Phase::HostingPending);
    assert!(!controller.incoming_message_ready());
}

#[tokio::test]
async fn missing_backend_never_submits_login_or_marks_ready() {
    let mut controller = HostingController::new("line-a".into(), None);
    controller.import(imported(), 100).unwrap();
    assert_eq!(controller.start(100).await.err(), Some(ControllerFailure::Backend(BackendFailure::Unavailable)));
    assert_eq!(controller.phase(), Phase::CredentialsImported);
    assert!(!controller.incoming_call_ready());
}

#[tokio::test]
async fn controller_submits_sdk_login_before_hosting_and_preserves_credentials() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    assert_eq!(*backend.operations.lock().unwrap(), vec![Operation::LoginToken("fixture-session-token".into())]);
    assert_eq!(controller.phase(), Phase::SdkLoginPending);
    assert!(!controller.incoming_message_ready());
    controller.on_callback(&owner, Callback::SdkLogin { success: true }, 100).await.unwrap();
    assert_eq!(controller.phase(), Phase::HostingPending);
    assert_eq!(backend.operations.lock().unwrap()[1], Operation::Status(4, "fixture-gateway-password".into()));
    controller.on_callback(&owner, Callback::HostingAck { success: true }, 100).await.unwrap();
    assert!(!controller.incoming_message_ready());
    controller.on_callback(&owner, Callback::Channels { calls: true, messages: true }, 100).await.unwrap();
    assert!(controller.incoming_call_ready());
    assert!(controller.incoming_message_ready());
    assert!(!controller.outbound_authorized());
}

#[tokio::test]
async fn duplicate_start_and_out_of_order_callback_do_not_submit_extra_operations() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    assert!(controller.start(100).await.is_err());
    assert!(!controller.on_callback(&owner, Callback::HostingAck { success: true }, 100).await.unwrap());
    assert_eq!(backend.operations.lock().unwrap().len(), 1);
    assert_eq!(controller.phase(), Phase::SdkLoginPending);
}

#[tokio::test]
async fn rejected_sdk_login_never_attempts_gateway_registration() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.on_callback(&owner, Callback::SdkLogin { success: false }, 100).await.unwrap();
    assert_eq!(controller.phase(), Phase::AuthenticationFailed);
    assert_eq!(backend.operations.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn failure_during_unregister_still_logs_out_and_retires_old_callbacks() {
    let backend = MockBackend::new(true);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.on_callback(&owner, Callback::SdkLogin { success: true }, 100).await.unwrap();
    assert_eq!(controller.stop().await.err(), Some(ControllerFailure::Backend(BackendFailure::Disconnected)));
    let operations = backend.operations.lock().unwrap();
    assert_eq!(operations[2], Operation::Status(3, "fixture-gateway-password".into()));
    assert_eq!(operations[3], Operation::Logout);
    assert_eq!(controller.phase(), Phase::Unconfigured);
    assert!(!controller.on_callback(&owner, Callback::HostingAck { success: true }, 100).await.unwrap());
}

#[tokio::test]
async fn missing_callback_has_a_bounded_deadline_and_late_success_cannot_revive_it() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.tick(101, Instant::now() + Duration::from_secs(31));
    assert_eq!(controller.phase(), Phase::Disconnected);
    assert!(!controller.on_callback(&owner, Callback::SdkLogin { success: true }, 101).await.unwrap());
}

#[tokio::test]
async fn credentials_cannot_be_replaced_while_sdk_login_is_pending() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend));
    controller.import(imported(), 100).unwrap();
    controller.start(100).await.unwrap();
    assert_eq!(controller.import(imported(), 100).err(), Some(ControllerFailure::Credentials("jegotrip_stop_before_replace")));
}

#[tokio::test]
async fn callback_from_another_line_is_rejected_without_touching_this_backend() {
    let backend = MockBackend::new(false);
    let mut a = HostingController::new("line-a".into(), Some(backend.clone()));
    let mut b = HostingController::new("line-b".into(), Some(MockBackend::new(false)));
    a.import(imported(), 100).unwrap(); b.import(imported(), 100).unwrap();
    let a_owner = a.start(100).await.unwrap();
    let b_owner = b.start(100).await.unwrap();
    assert!(!a.on_callback(&b_owner, Callback::SdkLogin { success: true }, 100).await.unwrap());
    assert!(a.on_callback(&a_owner, Callback::SdkLogin { success: true }, 100).await.unwrap());
}

#[tokio::test]
async fn notification_names_alone_cannot_confuse_unregister_with_register() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.on_native_notification(&owner,
        notification("MtcCliServerLoginOkNotification", Some(NativeOperation::SdkLogin)), 100).await.unwrap();
    for operation in [None, Some(NativeOperation::SdkLogin), Some(NativeOperation::HostingUnregister)] {
        controller.on_native_notification(&owner,
            notification("MtcSmgwUpdateStatusOkNotification", operation), 100).await.unwrap();
        assert_eq!(controller.phase(), Phase::HostingPending);
    }
    controller.on_native_notification(&owner,
        notification("MtcSmgwUpdateStatusOkNotification", Some(NativeOperation::HostingRegister)), 100).await.unwrap();
    assert_eq!(controller.phase(), Phase::Hosted);
    assert!(!controller.incoming_message_ready());
    assert_eq!(backend.operations.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn native_ack_from_stopped_attempt_cannot_complete_restarted_hosting() {
    let mut controller = HostingController::new("line-a".into(), Some(MockBackend::new(false)));
    let old = hosted(&mut controller).await;
    controller.stop().await.unwrap();
    controller.import(imported(), 100).unwrap();
    let current = controller.start(100).await.unwrap();
    controller.on_callback(&current, Callback::SdkLogin { success: true }, 100).await.unwrap();
    controller.on_native_notification(&old,
        notification("MtcSmgwUpdateStatusOkNotification", Some(NativeOperation::HostingRegister)), 100).await.unwrap();
    assert_eq!(controller.phase(), Phase::HostingPending);
}

#[tokio::test]
async fn recreated_same_line_cannot_accept_previous_controller_owner() {
    let mut old = HostingController::new("line-a".into(), Some(MockBackend::new(false)));
    old.import(imported(), 100).unwrap();
    let old_owner = old.start(100).await.unwrap();
    let backend = MockBackend::new(false);
    let mut current = HostingController::new("line-a".into(), Some(backend.clone()));
    current.import(imported(), 100).unwrap();
    let current_owner = current.start(100).await.unwrap();
    assert_ne!(old_owner, current_owner);
    assert!(!current.on_callback(&old_owner, Callback::SdkLogin { success: true }, 100).await.unwrap());
    assert_eq!(backend.operations.lock().unwrap().len(), 1);
    assert_eq!(current.phase(), Phase::SdkLoginPending);
}

#[tokio::test]
async fn stale_or_unready_malformed_message_is_ignored_before_decoding() {
    let mut controller = HostingController::new("line-a".into(), Some(MockBackend::new(false)));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    assert!(controller.on_native_notification(&owner,
        notification("MtcSmgwMessageRecvNotification", None), 100).await.unwrap().is_none());
    controller.stop().await.unwrap();
    assert!(controller.on_native_notification(&owner,
        notification("MtcSmgwMessageRecvNotification", None), 100).await.unwrap().is_none());
    let current = hosted(&mut controller).await;
    assert_eq!(controller.on_native_notification(&current,
        notification("MtcSmgwMessageRecvNotification", None), 100).await.err(),
        Some(ControllerFailure::Notification("jegotrip_native_message_missing")));
    assert!(controller.incoming_message_ready());
}

#[tokio::test]
async fn expiry_at_callback_delivery_prevents_hosting_without_scheduler_tick() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    assert!(!controller.on_callback(&owner, Callback::SdkLogin { success: true }, 200).await.unwrap());
    assert_eq!(controller.phase(), Phase::CredentialExpired);
    assert_eq!(backend.operations.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn expiry_at_message_delivery_revokes_ready_without_scheduler_tick() {
    let mut controller = HostingController::new("line-a".into(), Some(MockBackend::new(false)));
    let owner = hosted(&mut controller).await;
    let mut message = notification("MtcSmgwMessageRecvNotification", None);
    message.message = Some(b"fixture-message");
    assert!(controller.on_native_notification(&owner, message, 200).await.unwrap().is_none());
    assert_eq!(controller.phase(), Phase::CredentialExpired);
    assert!(!controller.incoming_call_ready());
    assert!(!controller.incoming_message_ready());
}

#[tokio::test]
async fn expiry_wins_over_callback_timeout_and_repeated_ticks_do_not_change_it() {
    let mut controller = HostingController::new("line-a".into(), Some(MockBackend::new(false)));
    controller.import(imported(), 100).unwrap();
    controller.start(100).await.unwrap();
    controller.tick(200, Instant::now() + Duration::from_secs(31));
    let owner = controller.session.current_owner();
    controller.tick(201, Instant::now() + Duration::from_secs(60));
    assert_eq!(controller.phase(), Phase::CredentialExpired);
    assert_eq!(controller.session.current_owner(), owner);
}

#[tokio::test]
async fn callback_deadline_is_checked_even_when_scheduler_has_not_ticked() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.pending_deadline = Some(Instant::now());
    assert!(!controller.on_callback(&owner, Callback::SdkLogin { success: true }, 101).await.unwrap());
    assert_eq!(controller.phase(), Phase::Disconnected);
    assert_eq!(backend.operations.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn correlated_rpc_reply_drives_hosting_ack_but_not_message_readiness() {
    use crate::connectivity::app_voice::providers::jegotrip::rpc_pending::HostingRpcTracker;
    let mut controller = HostingController::new("line-a".into(), Some(MockBackend::new(false)));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.on_callback(&owner, Callback::SdkLogin { success: true }, 100).await.unwrap();
    let mut tracker = HostingRpcTracker::new();
    let connection = tracker.connection_scope();
    let registration = GatewayRegistration { authname_prefix: "prefix", channel_id: "channel",
        delete_int_code: "1", sip_number: "number", sip_password: "password" };
    let (id, _) = tracker.prepare_update_status(owner.clone(), "actual-sdk-uri", STATUS_REGISTER,
        &registration, &[]).unwrap();
    let mut bytes = vec![0];
    bytes.extend_from_slice(&id.to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&6u32.to_be_bytes());
    bytes.extend_from_slice(&[0, 1, 0, 0, 0, 0]);
    let completed = tracker.on_reply(&connection, &bytes).unwrap().unwrap();
    assert!(controller.on_callback(&completed.owner, completed.callback, 100).await.unwrap());
    assert_eq!(controller.phase(), Phase::Hosted);
    assert!(!controller.incoming_message_ready());
    assert!(tracker.on_reply(&connection, &bytes).unwrap().is_none());
    controller.on_callback(&owner, Callback::Channels { calls: false, messages: true }, 100).await.unwrap();
    let mut n = notification("MtcSmgwMessageRecvNotification", None);
    n.message = Some(&[0, 0xff, 0]);
    let received = controller.on_native_notification(&owner, n, 100).await.unwrap().unwrap();
    assert_eq!(received.body, [0, 0xff, 0]);
    assert!(!controller.outbound_authorized());
}

#[tokio::test]
async fn native_reconnect_success_alone_cannot_restore_hosting_or_channels() {
    let mut controller = HostingController::new("line-a".into(), Some(MockBackend::new(false)));
    let owner = hosted(&mut controller).await;
    controller.on_native_notification(&owner,
        notification("MtcCliReconnectingNotification", None), 100).await.unwrap();
    assert_eq!(controller.phase(), Phase::Disconnected);
    controller.on_native_notification(&owner,
        notification("MtcCliReconnectOkNotification", None), 100).await.unwrap();
    assert_eq!(controller.phase(), Phase::Disconnected);
    assert!(!controller.incoming_call_ready());
    assert!(!controller.incoming_message_ready());
}
