//! Socket-pair protocol tests only; no proxy/device/global native fleet.
use super::*;
use crate::hardware::{
    cellular::backends::{
        config::{NativeDeviceConfig, NativeProtocol},
        io::NativeIo,
        native::NativeDevice,
        protocol::CommandRequest,
        NativeError,
    },
    devices::transport::TransportFuture,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct MemoryIo {
    receipts: Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
}
impl NativeIo for MemoryIo {
    fn execute<'a>(
        &'a self,
        _: &'a CommandRequest,
    ) -> TransportFuture<'a, Result<String, NativeError>> {
        Box::pin(async { panic!("no hardware IO in ledger fixture") })
    }
    fn save_receipt(&self, key: &str, bytes: &[u8], create: bool) -> Result<(), NativeError> {
        let mut rows = self.receipts.lock().unwrap();
        assert!(!create || !rows.contains_key(key));
        rows.insert(key.into(), bytes.into());
        Ok(())
    }
    fn clear_receipt(&self, key: &str) -> Result<(), NativeError> {
        assert!(self.receipts.lock().unwrap().remove(key).is_some());
        Ok(())
    }
}
fn pair() -> (
    QmiProxyConnection,
    UnixStream,
    Arc<NativeDevice>,
    Arc<MemoryIo>,
) {
    let (stream, peer) = UnixStream::pair().unwrap();
    for socket in [&stream, &peer] {
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
    }
    let io = Arc::new(MemoryIo::default());
    let device = NativeDevice::new(
        NativeDeviceConfig {
            hardware_key: "uim-ledger-fixture".into(),
            sysfs_anchor: "/sys/devices/fixture".into(),
            protocol: NativeProtocol::Qmi,
            control_device: "/dev/fixture".into(),
            at_device: None,
            sms_reception_enabled: false,
            uim_slot: 1,
            ims: None,
            data: None,
        },
        io.clone(),
    );
    let connection = QmiProxyConnection {
        stream,
        next_ctl_transaction: 1,
        next_service_transaction: 1,
        native_device: Some(device.clone()),
        native_channel: None,
    };
    (connection, peer, device, io)
}
fn reply(peer: &mut UnixStream, request: QmiMessage, mut fields: Vec<QmiTlv>, failure: bool) {
    fields.push(tlv(
        TLV_RESULT,
        if failure {
            vec![1, 0, 7, 0]
        } else {
            vec![0, 0, 0, 0]
        },
    ));
    peer.write_all(
        &encode_qmi_message(&QmiMessage {
            tlvs: fields,
            ..request
        })
        .unwrap(),
    )
    .unwrap();
}
#[test]
fn native_ledger_spans_ctl_allocation_channel_close_and_ctl_release() {
    for fail_release in [false, true] {
        let (mut conn, mut peer, device, io) = pair();
        let server_io = io.clone();
        let server = std::thread::spawn(move || {
            let request = read_qmi_message(&mut peer).unwrap();
            assert_eq!(request.message_id, QMI_CTL_ALLOCATE_CID);
            assert_eq!(
                server_io.receipts.lock().unwrap().len(),
                1,
                "intent must precede CTL allocation"
            );
            reply(
                &mut peer,
                request,
                vec![tlv(TLV_CTL_ALLOCATION_INFO, vec![QMUX_UIM_SERVICE, 7])],
                false,
            );
            let request = read_qmi_message(&mut peer).unwrap();
            assert_eq!(request.message_id, QMI_UIM_OPEN_LOGICAL_CHANNEL);
            reply(
                &mut peer,
                request,
                vec![tlv(TLV_UIM_OPEN_CHANNEL_ID, vec![2])],
                false,
            );
            let request = read_qmi_message(&mut peer).unwrap();
            assert_eq!(request.message_id, QMI_UIM_LOGICAL_CHANNEL);
            reply(&mut peer, request, vec![], false);
            let request = read_qmi_message(&mut peer).unwrap();
            assert_eq!(request.message_id, QMI_CTL_RELEASE_CID);
            assert_eq!(
                server_io.receipts.lock().unwrap().len(),
                1,
                "channel close must retain the CTL receipt"
            );
            reply(&mut peer, request, vec![], fail_release);
        });
        let client = conn.allocate_uim_cid().unwrap();
        let mut aid = USIM_AID_PREFIX.to_vec();
        aid.push(1);
        let channel = conn.open_logical_channel(client, 1, &aid).unwrap();
        conn.close_logical_channel(client, 1, channel.channel_id)
            .unwrap();
        assert_eq!(
            device.sim_channel_status().owner.unwrap().client_id,
            Some(7)
        );
        assert_eq!(conn.release_uim_cid(client).is_err(), fail_release);
        server.join().unwrap();
        drop(conn);
        assert_eq!(!io.receipts.lock().unwrap().is_empty(), fail_release);
        assert_eq!(
            device.sim_channel_status().reconciliation_required,
            fail_release
        );
    }
}
#[test]
fn explicit_ctl_allocation_rejection_releases_only_the_pending_intent() {
    let (mut conn, mut peer, device, io) = pair();
    let server = std::thread::spawn(move || {
        let request = read_qmi_message(&mut peer).unwrap();
        reply(&mut peer, request, vec![], true);
    });
    assert!(conn.allocate_uim_cid().is_err());
    server.join().unwrap();
    drop(conn);
    assert!(io.receipts.lock().unwrap().is_empty());
    assert!(!device.sim_channel_status().reconciliation_required);
}
