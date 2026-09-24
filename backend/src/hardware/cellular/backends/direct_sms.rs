//! Durable native SMS capture and submission. The physical operation lease
//! spans SIM validation, inbox commit and transport acknowledgement/cleanup.
//! No modem PDU or SIM identity is published on the public URC bus.
use super::{
    native::NativeDevice,
    protocol::{at_payload, csv, phone_number, CommandRequest, Tool},
    NativeError,
};
use crate::{
    connectivity::core::sms_codec,
    hardware::cellular::at_urc::{DirectBatch, DirectKind},
    platform::db::{native_sms_inbox::NativeSend, Database},
};
use std::{collections::BTreeMap, sync::Arc};

pub struct CapturedSms {
    pub sim_key: String,
    pub dropped: u64,
    pub unconfirmed_acks: usize,
    pub storage_scan_deferred: bool,
    pub capture_deferred: bool,
    pub stored_deletes_deferred: usize,
}

pub struct SubmittedSms {
    pub sms_id: i64,
    pub path: String,
    pub part_count: usize,
    pub submitted_parts: usize,
    pub confirmed: bool,
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(crate) fn recipient_key(number: &str) -> Result<String, NativeError> {
    let digits = number.strip_prefix('+').unwrap_or(number);
    if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(NativeError::Protocol(
            "native_sms_recipient_not_numeric".into(),
        ));
    }
    // Do not strip TON: national and international numbers are not implicitly
    // equivalent. A differing report stays unmatched instead of guessing.
    Ok(digest(number.as_bytes()))
}
fn imsi(output: &str) -> Result<&str, NativeError> {
    output
        .lines()
        .map(str::trim)
        .find(|v| (5..=15).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_digit()))
        .ok_or_else(|| NativeError::Unavailable("native_sms_sim_identity_unavailable".into()))
}
fn sim_key(device: &NativeDevice, imsi: &str) -> String {
    digest(
        format!(
            "native-sms\0{}\0{}\0{imsi}",
            device.spec.hardware_key, device.spec.uim_slot
        )
        .as_bytes(),
    )
}
fn private_request(
    device: &NativeDevice,
    tool: Tool,
    arguments: Vec<String>,
) -> Result<CommandRequest, NativeError> {
    let mut request = device.at_request("AT")?;
    request.tool = tool;
    request.arguments = arguments;
    Ok(request)
}
async fn bind_current_sim(device: &NativeDevice) -> Result<String, NativeError> {
    device.verify_primary_slot_locked().await?;
    let output = device.io.execute(&device.at_request("AT+CIMI")?).await?;
    let key = sim_key(device, imsi(&output)?);
    device
        .io
        .execute(&private_request(
            device,
            Tool::AtDirectBind,
            vec![key.clone()],
        )?)
        .await?;
    Ok(key)
}
fn ack_service(output: &str) -> Option<bool> {
    let fields = csv(at_payload(output, "+CSMS:")?).ok()?;
    if fields.len() != 4 || fields[1] != "1" {
        return None;
    }
    match fields[0].as_str() {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}
fn inbox_error(_: rusqlite::Error) -> NativeError {
    NativeError::Unavailable("native_sms_inbox_write_failed".into())
}

impl NativeDevice {
    pub(super) async fn initialize_durable_sms(self: &Arc<Self>) -> Result<(), NativeError> {
        self.require_sms_reception()?;
        self.verify_primary_slot().await?;
        let this = self.clone();
        tokio::spawn(async move {
            let _gate = this.operation.lock().await;
            this.ensure_available()?;
            bind_current_sim(&this).await?;
            this.io.execute(&this.at_request("AT+CMGF=0")?).await?;
            // Store normal MT SMS. Only unavoidable direct deliveries and
            // reports use the private inbox; never opt into direct-only MT.
            this.io
                .execute(&this.at_request("AT+CNMI=2,1,0,1,0")?)
                .await?;
            Ok(())
        })
        .await
        .map_err(|_| NativeError::CommandFailed("native_sms_initialize_task_failed"))?
    }

    pub async fn capture_sms(self: &Arc<Self>, db: Database) -> Result<CapturedSms, NativeError> {
        self.require_sms_reception()?;
        self.verify_primary_slot().await?;
        let this = self.clone();
        tokio::spawn(async move {
            let _gate = this.operation.lock().await;
            this.ensure_available()?;
            let key = bind_current_sim(&this).await?;
            let line = this.spec.line_id();
            // Reading service selection is not permission to change it. Some
            // modems do not implement CSMS; unknown must never cause CNMA.
            let service = this
                .io
                .execute(&this.at_request("AT+CSMS?")?)
                .await
                .ok()
                .and_then(|s| ack_service(&s));
            let output = this
                .io
                .execute(&private_request(
                    &this,
                    Tool::AtDirectPoll,
                    vec!["direct-pdus".into()],
                )?)
                .await?;
            let batch: DirectBatch = serde_json::from_str(&output)
                .map_err(|_| NativeError::Protocol("native_direct_batch_invalid".into()))?;
            if batch.pdus.len() > 16 {
                return Err(NativeError::Protocol("native_direct_batch_limit".into()));
            }
            let mut unconfirmed_acks = 0;
            let mut capture_deferred = false;
            for pdu in batch.pdus {
                if pdu.sim_key != key {
                    return Err(NativeError::OwnerConflict(
                        "native_direct_sim_scope_changed".into(),
                    ));
                }
                let fingerprint = digest(pdu.hex.to_ascii_uppercase().as_bytes());
                // This must remain before AtDirectCommit even for duplicates:
                // a retransmission can have a new token for an old durable row.
                if db
                    .stage_native_pdu(&line, &key, pdu.kind.inbox_kind(), &fingerprint, &pdu.hex)
                    .is_err()
                {
                    // Drain/promote already committed rows even when the
                    // inbox is full. Keep this token unacknowledged for retry.
                    capture_deferred = true;
                    break;
                }
                let ack = match service {
                    Some(true) => "1",
                    Some(false) => "0",
                    None => "unknown",
                };
                let request = private_request(
                    &this,
                    Tool::AtDirectCommit,
                    vec![pdu.token.to_string(), ack.into()],
                )?;
                let result = this
                    .io
                    .execute(&request)
                    .await
                    .unwrap_or_else(|_| "unconfirmed".into());
                let result = match result.as_str() {
                    "confirmed" | "not_required" | "ambiguous" | "service_unconfirmed" => {
                        result.as_str()
                    }
                    _ => "unconfirmed",
                };
                if !matches!(result, "confirmed" | "not_required") {
                    unconfirmed_acks += 1;
                }
                db.mark_native_pdu_ack(&line, &key, &fingerprint, result)
                    .map_err(inbox_error)?;
            }
            let stored_scan = async {
                this.ensure_available()?;
                this.io.execute(&this.at_request("AT+CMGF=0")?).await?;
                let output = this.io.execute(&this.at_request("AT+CMGL=4")?).await?;
                stored_pdus(&output)
            }
            .await;
            let storage_scan_deferred = stored_scan.is_err();
            // A modem without CMGL support must not starve durable direct
            // PDU replay. Storage failure does not authorize any deletion.
            let stored = stored_scan.unwrap_or_default();
            let mut committed = Vec::new();
            for (index, pdu) in stored {
                let Some(kind) = pdu_kind(&pdu) else { continue };
                let fingerprint = digest(pdu.as_bytes());
                if db
                    .stage_native_pdu(&line, &key, kind.inbox_kind(), &fingerprint, &pdu)
                    .is_err()
                {
                    capture_deferred = true;
                    break;
                }
                db.mark_native_pdu_ack(&line, &key, &fingerprint, "stored")
                    .map_err(inbox_error)?;
                committed.push((index, pdu));
            }
            // Always recheck before returning a scope for promotion, even if
            // there were only direct PDUs or the storage scan was unsupported.
            this.verify_primary_slot_locked().await?;
            let output = this.io.execute(&this.at_request("AT+CIMI")?).await?;
            if sim_key(&this, imsi(&output)?) != key {
                return Err(NativeError::OwnerConflict("native_sms_sim_changed".into()));
            }
            let mut stored_deletes_deferred = 0;
            if !committed.is_empty() {
                // A reused storage index must not delete a later message. The
                // re-read and exact comparison share the same physical lease.
                let current = match this.io.execute(&this.at_request("AT+CMGL=4")?).await {
                    Ok(output) => stored_pdus(&output)
                        .ok()
                        .map(|rows| rows.into_iter().collect::<BTreeMap<_, _>>()),
                    Err(_) => None,
                };
                for (index, pdu) in committed {
                    if current
                        .as_ref()
                        .is_some_and(|current| current.get(&index) == Some(&pdu))
                    {
                        if this
                            .io
                            .execute(&this.at_request(&format!("AT+CMGD={index}"))?)
                            .await
                            .is_err()
                        {
                            stored_deletes_deferred += 1;
                        }
                    } else {
                        stored_deletes_deferred += 1;
                    }
                }
            }
            Ok(CapturedSms {
                sim_key: key,
                dropped: batch.dropped,
                unconfirmed_acks,
                storage_scan_deferred,
                capture_deferred,
                stored_deletes_deferred,
            })
        })
        .await
        .map_err(|_| NativeError::CommandFailed("native_sms_capture_task_failed"))?
    }

    /// One shielded task owns the entire multipart submission and its ledger.
    /// After transmission starts, uncertain results are returned explicitly,
    /// not as an error that would authorize automatic fallback/resending.
    pub async fn send_sms_persisted(
        self: &Arc<Self>,
        db: Database,
        number: &str,
        text: &str,
    ) -> Result<SubmittedSms, NativeError> {
        phone_number(number)?;
        let recipient = recipient_key(number)?;
        self.verify_primary_slot().await?;
        let this = self.clone();
        let number = number.to_string();
        let text = text.to_string();
        tokio::spawn(async move {
            let _gate = this.operation.lock().await;
            this.ensure_available()?;
            let key = bind_current_sim(&this).await?;
            let response = this.io.execute(&this.at_request("AT+CSCA?")?).await?;
            let fields = csv(at_payload(&response, "+CSCA:")
                .ok_or_else(|| NativeError::Unavailable("native_smsc_unavailable".into()))?)?;
            let smsc = fields
                .first()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| NativeError::Unavailable("native_smsc_unavailable".into()))?;
            let pdus = sms_codec::build_modem_submit_pdus(&number, &text, smsc)
                .map_err(|_| NativeError::Protocol("native_sms_encoding_failed".into()))?;
            let part_count = pdus.len();
            let id = db
                .begin_native_sms_send(NativeSend {
                    line_id: &this.spec.line_id(),
                    sim_key: &key,
                    recipient_key: &recipient,
                    number: &number,
                    text: &text,
                    part_count,
                    started_at: chrono::Utc::now().timestamp(),
                })
                .map_err(inbox_error)?;
            let mut submitted_parts = 0;
            for (part, pdu) in pdus.into_iter().enumerate() {
                if this.ensure_available().is_err() {
                    break;
                }
                let length = pdu.len() - 1 - usize::from(pdu[0]);
                let hex = pdu.iter().map(|b| format!("{b:02X}")).collect();
                let request = private_request(&this, Tool::AtSms, vec![length.to_string(), hex])?;
                let started = chrono::Utc::now().timestamp();
                let result = this.io.execute(&request).await;
                let Ok(reference) = result.and_then(|v| {
                    v.parse::<u8>()
                        .map_err(|_| NativeError::Protocol("native_sms_reference_invalid".into()))
                }) else {
                    break;
                };
                if db
                    .remember_native_sms_part(
                        id,
                        part,
                        reference,
                        started,
                        chrono::Utc::now().timestamp(),
                    )
                    .is_err()
                {
                    break;
                }
                submitted_parts += 1;
            }
            let same_sim = match this.io.execute(&this.at_request("AT+CIMI")?).await {
                Ok(output) => imsi(&output).is_ok_and(|v| sim_key(&this, v) == key),
                Err(_) => false,
            };
            let confirmed = submitted_parts == part_count && same_sim;
            let persisted = db.finish_native_sms_send(id, confirmed).is_ok();
            Ok(SubmittedSms {
                sms_id: id,
                path: format!("{}:submitted-sms:{id}", this.spec.selector()),
                part_count,
                submitted_parts,
                confirmed: confirmed && persisted,
            })
        })
        .await
        .map_err(|_| NativeError::CommandFailed("native_sms_submission_task_failed"))?
    }
}

pub(crate) fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty()
        || text.len() > 1024
        || text.len() % 2 != 0
        || !text.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}
fn pdu_kind(pdu: &str) -> Option<DirectKind> {
    let bytes = unhex(pdu)?;
    match bytes.get(1 + usize::from(bytes[0]))? & 3 {
        0 => Some(DirectKind::Deliver),
        2 => Some(DirectKind::StatusReport),
        _ => None,
    }
}
fn stored_pdus(output: &str) -> Result<Vec<(u32, String)>, NativeError> {
    let invalid = || NativeError::Protocol("native_stored_pdu_framing_invalid".into());
    let mut pending = None;
    let mut result = Vec::new();
    let mut indices = std::collections::BTreeSet::new();
    for line in output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(header) = line.strip_prefix("+CMGL:") {
            if pending.is_some() {
                return Err(invalid());
            }
            let fields = csv(header)?;
            let index = fields
                .first()
                .and_then(|v| v.parse::<u32>().ok())
                .ok_or_else(invalid)?;
            let length = fields
                .last()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|v| (1..=255).contains(v))
                .ok_or_else(invalid)?;
            if fields.len() < 4 || !indices.insert(index) {
                return Err(invalid());
            }
            pending = Some((index, length));
        } else if let Some((index, length)) = pending.take() {
            let bytes = unhex(line).ok_or_else(invalid)?;
            if bytes.len() != 1 + usize::from(bytes[0]) + length {
                return Err(invalid());
            }
            result.push((index, line.to_ascii_uppercase()));
        } else if line != "OK" {
            return Err(invalid());
        }
    }
    if pending.is_some() {
        return Err(invalid());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::{
        config::{NativeDeviceConfig, NativeProtocol},
        io::NativeIo,
    };
    use super::*;
    use crate::hardware::{cellular::at_urc::DirectPdu, devices::transport::TransportFuture};
    struct Fixture {
        db: Database,
        requests: std::sync::Mutex<Vec<CommandRequest>>,
        sim: std::sync::Mutex<String>,
        fail_stage: bool,
    }
    impl NativeIo for Fixture {
        fn execute<'a>(
            &'a self,
            request: &'a CommandRequest,
        ) -> TransportFuture<'a, Result<String, NativeError>> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(request.clone());
                match request.tool {
                    Tool::AtDirectBind => {
                        *self.sim.lock().unwrap() = request.arguments[0].clone();
                        Ok("bound".into())
                    }
                    Tool::AtDirectPoll => {
                        let batch = DirectBatch {
                            pdus: vec![DirectPdu {
                                token: 1,
                                kind: DirectKind::Deliver,
                                sim_key: self.sim.lock().unwrap().clone(),
                                hex: "000004812143000862101000000000020041".into(),
                            }],
                            dropped: 0,
                        };
                        Ok(serde_json::to_string(&batch).unwrap())
                    }
                    Tool::AtDirectCommit => {
                        assert!(!self.fail_stage);
                        let key = self.sim.lock().unwrap().clone();
                        assert_eq!(
                            self.db
                                .pending_native_pdus(&spec().line_id(), &key)
                                .unwrap()
                                .len(),
                            1
                        );
                        Ok("confirmed".into())
                    }
                    Tool::AtSms => Ok("7".into()),
                    _ => match request.arguments[0].as_str() {
                        "AT+CIMI" => Ok("001011111111111\r\nOK".into()),
                        "AT+CSMS?" => Ok("+CSMS: 1,1,1,1\r\nOK".into()),
                        "AT+CSCA?" => Ok("+CSCA: \"+12345\",145\r\nOK".into()),
                        _ => Ok("OK".into()),
                    },
                }
            })
        }
    }
    fn spec() -> NativeDeviceConfig {
        NativeDeviceConfig {
            hardware_key: "fixture-direct".into(),
            sysfs_anchor: "/sys/devices/fixture".into(),
            protocol: NativeProtocol::At,
            control_device: "/dev/fixture-at".into(),
            at_device: None,
            sms_reception_enabled: true,
            uim_slot: 1,
            ims: None,
            data: None,
        }
    }
    #[tokio::test]
    async fn direct_commit_only_occurs_after_durable_staging() {
        let db = Database::new(":memory:".into()).unwrap();
        let io = Arc::new(Fixture {
            db: db.clone(),
            requests: Default::default(),
            sim: Default::default(),
            fail_stage: false,
        });
        let device = NativeDevice::new(spec(), io.clone());
        let captured = device.capture_sms(db.clone()).await.unwrap();
        assert_eq!(captured.unconfirmed_acks, 0);
        assert_eq!(
            db.pending_native_pdus(&spec().line_id(), &captured.sim_key)
                .unwrap()
                .len(),
            1
        );
        assert!(io
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.tool == Tool::AtDirectCommit));
    }
    #[tokio::test]
    async fn full_inbox_never_authorizes_cnma_or_storage_delete() {
        let db = Database::new(":memory:".into()).unwrap();
        let io = Arc::new(Fixture {
            db: db.clone(),
            requests: Default::default(),
            sim: Default::default(),
            fail_stage: true,
        });
        let device = NativeDevice::new(spec(), io.clone());
        let key = sim_key(&device, "001011111111111");
        for n in 0..256 {
            db.stage_native_pdu(
                &spec().line_id(),
                &key,
                "deliver",
                &format!("{n:064x}"),
                "00",
            )
            .unwrap();
        }
        let captured = device.capture_sms(db).await.unwrap();
        assert!(captured.capture_deferred);
        assert!(!io
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r.tool == Tool::AtDirectCommit
                || r.arguments.iter().any(|a| a.starts_with("AT+CMGD"))));
    }
    #[tokio::test]
    async fn sending_records_expected_parts_and_actual_modem_reference() {
        let db = Database::new(":memory:".into()).unwrap();
        let io = Arc::new(Fixture {
            db: db.clone(),
            requests: Default::default(),
            sim: Default::default(),
            fail_stage: false,
        });
        let device = NativeDevice::new(spec(), io);
        let result = device
            .send_sms_persisted(db, "+1234", "fixture")
            .await
            .unwrap();
        assert!(result.confirmed);
        assert_eq!(result.submitted_parts, 1);
    }
    enum Fault {
        Storage,
        Delete,
        SimChanged,
        SlotChanged,
    }
    struct FaultIo {
        inner: Fixture,
        fault: Fault,
        queries: std::sync::atomic::AtomicUsize,
    }
    impl NativeIo for FaultIo {
        fn execute<'a>(
            &'a self,
            request: &'a CommandRequest,
        ) -> TransportFuture<'a, Result<String, NativeError>> {
            Box::pin(async move {
                use std::sync::atomic::Ordering;
                if matches!(self.fault, Fault::SlotChanged)
                    && request
                        .arguments
                        .iter()
                        .any(|a| a == "--uim-get-card-status")
                {
                    let slot = if self.queries.fetch_add(1, Ordering::SeqCst) == 0 {
                        1
                    } else {
                        2
                    };
                    return Ok(format!("Primary GW: slot '{slot}', application '0'"));
                }
                if request.arguments.first().map(String::as_str) == Some("AT+CIMI")
                    && matches!(self.fault, Fault::SimChanged)
                    && self.queries.fetch_add(1, Ordering::SeqCst) > 0
                {
                    return Ok("001012222222222\r\nOK".into());
                }
                if request.arguments.first().map(String::as_str) == Some("AT+CMGL=4") {
                    if matches!(self.fault, Fault::Storage) {
                        return Err(NativeError::Unsupported("fixture_no_cmgl"));
                    }
                    if matches!(self.fault, Fault::Delete) {
                        return Ok(
                            "+CMGL: 1,0,,17\r\n000004812143000862101000000000020041\r\nOK".into(),
                        );
                    }
                }
                if matches!(self.fault, Fault::Delete)
                    && request.arguments.iter().any(|a| a.starts_with("AT+CMGD="))
                {
                    return Err(NativeError::CommandFailed("fixture_delete_failed"));
                }
                self.inner.execute(request).await
            })
        }
    }
    fn fault_device(db: &Database, fault: Fault) -> Arc<NativeDevice> {
        let mut config = spec();
        if matches!(fault, Fault::SlotChanged) {
            config.protocol = NativeProtocol::Qmi;
            config.at_device = Some("/dev/fixture-at".into());
        }
        NativeDevice::new(
            config,
            Arc::new(FaultIo {
                inner: Fixture {
                    db: db.clone(),
                    requests: Default::default(),
                    sim: Default::default(),
                    fail_stage: false,
                },
                fault,
                queries: Default::default(),
            }),
        )
    }
    #[tokio::test]
    async fn unsupported_storage_and_failed_delete_do_not_starve_durable_delivery() {
        for fault in [Fault::Storage, Fault::Delete] {
            let db = Database::new(":memory:".into()).unwrap();
            let device = fault_device(&db, fault);
            let capture = device.capture_sms(db.clone()).await.unwrap();
            assert!(capture.storage_scan_deferred || capture.stored_deletes_deferred == 1);
            let messages = crate::services::messaging::native_sms::consume_pending(
                &db,
                &spec().line_id(),
                &capture.sim_key,
                true,
            )
            .unwrap();
            assert_eq!(messages.len(), 1);
            assert_eq!(messages[0].content, "A");
        }
    }
    #[tokio::test]
    async fn slot_or_sim_change_does_not_authorize_promotion_under_a_new_scope() {
        let db = Database::new(":memory:".into()).unwrap();
        let device = fault_device(&db, Fault::SlotChanged);
        assert_eq!(
            device.capture_sms(db.clone()).await.err(),
            Some(NativeError::OwnerConflict(
                "native_primary_sim_slot_unconfirmed_or_mismatched".into()
            ))
        );
        let device = fault_device(&db, Fault::SimChanged);
        let key = sim_key(&device, "001011111111111");
        assert_eq!(
            device.capture_sms(db.clone()).await.err(),
            Some(NativeError::OwnerConflict("native_sms_sim_changed".into()))
        );
        assert_eq!(
            db.pending_native_pdus(&spec().line_id(), &key)
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn csms_selection_is_read_not_inferred_from_cnmi() {
        assert_eq!(ack_service("+CSMS: 1,1,1,1\r\nOK"), Some(true));
        assert_eq!(ack_service("+CSMS: 0,1,1,1"), Some(false));
        for value in ["OK", "+CSMS: 1,0,1,1", "+CSMS: 9,1,1,1", "+CSMS: 1,1"] {
            assert_eq!(ack_service(value), None);
        }
    }
    #[test]
    fn stored_parser_rejects_broken_headers_duplicate_indices_and_wrong_lengths() {
        assert_eq!(
            stored_pdus("+CMGL: 1,0,,4\r\n0000112233\r\nOK").unwrap(),
            vec![(1, "0000112233".into())]
        );
        for output in [
            "+CMGL: 1,0,,4\r\nOK",
            "+CMGL: 1,0,,5\r\n0000112233",
            "+CMGL: 1,0,,4\r\n+CMGL: 2,0,,4",
            "+CMGL: 1,0,,4\r\n0000112233\r\n+CMGL: 1,0,,4\r\n0000112233",
        ] {
            assert!(stored_pdus(output).is_err());
        }
        assert_ne!(
            recipient_key("1234").unwrap(),
            recipient_key("+1234").unwrap()
        );
    }
}
