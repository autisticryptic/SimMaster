use super::*;

fn notification(name: &str) -> NativeNotification<'_> {
    NativeNotification { name, operation: None, message: None, from: None, time: None }
}

#[test]
fn hosting_notification_requires_captured_register_operation_not_invented_result_field() {
    assert!(decode(notification("MtcSmgwUpdateStatusOkNotification")).unwrap().is_none());
    let mut n = notification("MtcSmgwUpdateStatusOkNotification");
    n.operation = Some(NativeOperation::HostingRegister);
    assert!(matches!(decode(n).unwrap(),
        Some(NativeEvent::Callback(Callback::HostingAck { success: true }))));
    let mut n = notification("MtcSmgwUpdateStatusFailNotification");
    n.operation = Some(NativeOperation::HostingRegister);
    assert!(matches!(decode(n).unwrap(),
        Some(NativeEvent::Callback(Callback::HostingAck { success: false }))));
}

#[test]
fn unregister_or_login_operation_cannot_acknowledge_register() {
    for name in ["MtcSmgwUpdateStatusOkNotification", "MtcSmgwUpdateStatusFailNotification"] {
        for operation in [None, Some(NativeOperation::HostingUnregister), Some(NativeOperation::SdkLogin)] {
            let mut n = notification(name);
            n.operation = operation;
            assert!(decode(n).unwrap().is_none());
        }
    }
}

#[test]
fn login_completions_require_the_captured_login_operation() {
    for (name, expected) in [("MtcCliServerLoginOkNotification", true),
        ("MtcCliServerLoginDidFailNotification", false)] {
        assert!(decode(notification(name)).unwrap().is_none());
        let mut n = notification(name);
        n.operation = Some(NativeOperation::HostingRegister);
        assert!(decode(n).unwrap().is_none());
        let mut n = notification(name);
        n.operation = Some(NativeOperation::SdkLogin);
        assert!(matches!(decode(n).unwrap(), Some(NativeEvent::Callback(Callback::SdkLogin { success }))
            if success == expected));
    }
}

#[test]
fn message_payload_is_preserved_as_bytes_including_non_utf8() {
    let mut n = notification("MtcSmgwMessageRecvNotification");
    n.message = Some(&[0xff, 0, 0xe7]);
    n.from = Some(b"fixture-sender");
    n.time = Some(b"fixture-time");
    let Some(NativeEvent::IncomingMessage(message)) = decode(n).unwrap() else { panic!("message expected") };
    assert_eq!(message.body, vec![0xff, 0, 0xe7]);
    assert_eq!(message.from.as_deref(), Some(b"fixture-sender".as_slice()));
    assert_eq!(message.time.as_deref(), Some(b"fixture-time".as_slice()));
    assert!(!format!("{message:?}").contains("fixture"));
}

#[test]
fn empty_body_is_a_present_message_not_a_missing_field() {
    let mut n = notification("MtcSmgwMessageRecvNotification");
    n.message = Some(b"");
    let Some(NativeEvent::IncomingMessage(message)) = decode(n).unwrap() else { panic!("message expected") };
    assert!(message.body.is_empty());
}

#[test]
fn notification_debug_does_not_dump_phone_number_or_sms_contents() {
    let mut n = notification("MtcSmgwMessageRecvNotification");
    n.message = Some(b"fixture-secret-content");
    n.from = Some(b"fixture-secret-sender");
    assert!(!format!("{n:?}").contains("fixture"));
}

#[test]
fn missing_body_and_oversized_message_are_explicit_errors() {
    assert_eq!(decode(notification("MtcSmgwMessageRecvNotification")).err(), Some("jegotrip_native_message_missing"));
    let bytes = vec![0; 65_537];
    let mut n = notification("MtcSmgwMessageRecvNotification");
    n.message = Some(&bytes);
    assert_eq!(decode(n).err(), Some("jegotrip_native_message_too_large"));
}

#[test]
fn sender_and_time_fields_have_the_same_bounded_copy_policy() {
    let bytes = vec![0; 65_537];
    for field in [0, 1] {
        let mut n = notification("MtcSmgwMessageRecvNotification");
        n.message = Some(b"body");
        if field == 0 { n.from = Some(&bytes); } else { n.time = Some(&bytes); }
        assert_eq!(decode(n).err(), Some("jegotrip_native_message_too_large"));
    }
}

#[test]
fn send_success_or_status2_success_never_acknowledge_hosting_status1() {
    for name in ["MtcSmgwSendMessageOkNotification", "MtcSmgwUpdateStatus2OkNotification",
        "MtcSgwDeliGetInfoOkNotification", "MtcCliReconnectOkNotification", "unknown"] {
        let mut n = notification(name);
        n.operation = Some(NativeOperation::HostingRegister);
        assert!(decode(n).unwrap().is_none());
    }
}

#[test]
fn reconnecting_or_logout_revokes_readiness() {
    for name in ["MtcCliReconnectingNotification", "MtcCliServerDidLogoutNotification", "MtcCliServerLogoutedNotification"] {
        assert!(matches!(decode(notification(name)).unwrap(),
            Some(NativeEvent::Callback(Callback::Failure(BackendFailure::Disconnected)))));
    }
}
