use super::*;
use crate::connectivity::app_voice::providers::jegotrip::tests::imported;
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
    controller.on_callback(&owner, Callback::SdkLogin { success: true }).await.unwrap();
    assert_eq!(controller.phase(), Phase::HostingPending);
    assert_eq!(backend.operations.lock().unwrap()[1], Operation::Status(4, "fixture-gateway-password".into()));
    controller.on_callback(&owner, Callback::HostingAck { success: true }).await.unwrap();
    assert!(!controller.incoming_message_ready());
    controller.on_callback(&owner, Callback::Channels { calls: true, messages: true }).await.unwrap();
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
    assert!(!controller.on_callback(&owner, Callback::HostingAck { success: true }).await.unwrap());
    assert_eq!(backend.operations.lock().unwrap().len(), 1);
    assert_eq!(controller.phase(), Phase::SdkLoginPending);
}

#[tokio::test]
async fn rejected_sdk_login_never_attempts_gateway_registration() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.on_callback(&owner, Callback::SdkLogin { success: false }).await.unwrap();
    assert_eq!(controller.phase(), Phase::AuthenticationFailed);
    assert_eq!(backend.operations.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn failure_during_unregister_still_logs_out_and_retires_old_callbacks() {
    let backend = MockBackend::new(true);
    let mut controller = HostingController::new("line-a".into(), Some(backend.clone()));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.on_callback(&owner, Callback::SdkLogin { success: true }).await.unwrap();
    assert_eq!(controller.stop().await.err(), Some(ControllerFailure::Backend(BackendFailure::Disconnected)));
    let operations = backend.operations.lock().unwrap();
    assert_eq!(operations[2], Operation::Status(3, "fixture-gateway-password".into()));
    assert_eq!(operations[3], Operation::Logout);
    assert_eq!(controller.phase(), Phase::Unconfigured);
    assert!(!controller.on_callback(&owner, Callback::HostingAck { success: true }).await.unwrap());
}

#[tokio::test]
async fn missing_callback_has_a_bounded_deadline_and_late_success_cannot_revive_it() {
    let backend = MockBackend::new(false);
    let mut controller = HostingController::new("line-a".into(), Some(backend));
    controller.import(imported(), 100).unwrap();
    let owner = controller.start(100).await.unwrap();
    controller.tick(101, Instant::now() + Duration::from_secs(31));
    assert_eq!(controller.phase(), Phase::Disconnected);
    assert!(!controller.on_callback(&owner, Callback::SdkLogin { success: true }).await.unwrap());
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
    assert!(!a.on_callback(&b_owner, Callback::SdkLogin { success: true }).await.unwrap());
    assert!(a.on_callback(&a_owner, Callback::SdkLogin { success: true }).await.unwrap());
}
