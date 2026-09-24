//! Native modem SMS/USSD. Stored SMS is polled and only removed after the
//! shared application ingest has durably accepted it; no MM event source.

use super::{
    native::NativeDevice,
    protocol::{at_payload, csv, phone_number, Tool},
    NativeError,
};
use crate::connectivity::core::sms_codec;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone)]
pub struct NativeSms {
    pub path: String,
    pub number: String,
    pub content: String,
    pub timestamp: String,
    pub smsc: String,
    sim_imsi: String,
    cleanup_pending: bool,
    parts: Vec<(u32, String)>,
}

impl NativeDevice {
    fn require_sms_reception(&self) -> Result<(), NativeError> {
        if self.spec.sms_reception_enabled {
            Ok(())
        } else {
            Err(NativeError::Unsupported("native_sms_reception_disabled"))
        }
    }

    pub async fn ussd(self: &Arc<Self>, command: &str) -> Result<String, NativeError> {
        if !command.starts_with("AT+CUSD=") {
            return Err(NativeError::Protocol("native_ussd_command_invalid".into()));
        }
        let mut request = self.at_request(command)?;
        request.tool = Tool::AtUssd;
        self.command(request).await
    }

    pub async fn send_sms(
        self: &Arc<Self>,
        number: &str,
        text: &str,
    ) -> Result<String, NativeError> {
        self.verify_primary_slot().await?;
        phone_number(number)?;
        let smsc = self.at("AT+CSCA?").await?;
        let fields = csv(at_payload(&smsc, "+CSCA:")
            .ok_or_else(|| NativeError::Unavailable("native_smsc_unavailable".into()))?)?;
        let smsc = fields
            .first()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| NativeError::Unavailable("native_smsc_unavailable".into()))?;
        let pdus = sms_codec::build_modem_submit_pdus(number, text, smsc)
            .map_err(|_| NativeError::Protocol("native_sms_encoding_failed".into()))?;
        let mut requests = Vec::new();
        for pdu in pdus {
            let length = pdu
                .len()
                .checked_sub(1 + usize::from(pdu[0]))
                .ok_or_else(|| NativeError::Protocol("native_sms_encoding_failed".into()))?;
            let mut request = self.at_request("AT")?;
            request.tool = Tool::AtSms;
            request.arguments = vec![length.to_string(), hex(&pdu)];
            requests.push(request);
        }
        let references = self.commands(requests).await?;
        Ok(format!(
            "{}:submitted-sms:{}",
            self.spec.selector(),
            references.join("-")
        ))
    }

    /// Read event hints through the SAME physical command gate and AT reader.
    /// Reception admission must also be checked by the caller before polling.
    pub async fn poll_sms_events(
        self: &Arc<Self>,
    ) -> Result<crate::hardware::cellular::at_urc::UrcEvents, NativeError> {
        self.require_sms_reception()?;
        self.poll_events().await
    }

    /// Shared observation pump for enabled native lines. SMS consumers still
    /// need their separate reception/IMS admission before ingesting anything.
    pub async fn poll_events(
        self: &Arc<Self>,
    ) -> Result<crate::hardware::cellular::at_urc::UrcEvents, NativeError> {
        let mut request = self.at_request("AT")?;
        request.tool = Tool::AtPoll;
        request.arguments = vec!["poll-urcs".into()];
        let output = self.command(request).await?;
        let events: crate::hardware::cellular::at_urc::UrcEvents = serde_json::from_str(&output)
            .map_err(|_| NativeError::Protocol("native_at_events_invalid".into()))?;
        super::events::publish(self.spec.line_id(), events.clone());
        Ok(events)
    }

    pub async fn initialize_sms(self: &Arc<Self>) -> Result<(), NativeError> {
        self.require_sms_reception()?;
        self.verify_primary_slot().await?;
        self.commands(vec![
            self.at_request("AT+CMGF=0")?,
            // Store MT messages; a missed URC must be recoverable by polling.
            self.at_request("AT+CNMI=2,1,0,1,0")?,
        ])
        .await
        .map(|_| ())
    }

    async fn raw_messages(self: &Arc<Self>) -> Result<(String, Vec<(u32, String)>), NativeError> {
        self.require_sms_reception()?;
        self.verify_primary_slot().await?;
        let output = self
            .commands(vec![
                self.at_request("AT+CIMI")?,
                self.at_request("AT+CMGF=0")?,
                self.at_request("AT+CMGL=4")?,
            ])
            .await?;
        Ok((imsi_response(&output[0])?, parse_stored_pdus(&output[2])?))
    }

    pub async fn messages(self: &Arc<Self>) -> Result<Vec<NativeSms>, NativeError> {
        let (sim_imsi, pdus) = self.raw_messages().await?;
        let mut singles = Vec::new();
        let mut conflicts = std::collections::BTreeSet::new();
        let mut groups: BTreeMap<
            (String, u16, u8),
            BTreeMap<u8, (u32, String, sms_codec::MtSmsDeliver)>,
        > = BTreeMap::new();
        for (index, pdu) in pdus {
            let Some(bytes) = unhex(&pdu) else { continue };
            let Ok(delivery) = sms_codec::parse_modem_deliver_pdu(&bytes) else {
                continue;
            };
            if delivery.segment_total > 1 {
                let Some(reference) = delivery.segment_reference else {
                    continue;
                };
                let key = (
                    delivery.originator.clone(),
                    reference,
                    delivery.segment_total,
                );
                if conflicts.contains(&key) {
                    continue;
                }
                let entry = groups.entry(key.clone()).or_default();
                // Conflicting reused references are not permission to merge
                // unrelated messages. Leave them stored for explicit recovery.
                if entry.contains_key(&delivery.segment_sequence) {
                    conflicts.insert(key);
                    continue;
                }
                entry.insert(delivery.segment_sequence, (index, pdu, delivery));
            } else {
                singles.push(vec![(index, pdu, delivery)]);
            }
        }
        for (key, entries) in groups {
            if conflicts.contains(&key) {
                continue;
            }
            let total = key.2;
            if entries.len() == usize::from(total) && (1..=total).all(|i| entries.contains_key(&i))
            {
                singles.push(entries.into_values().collect());
            }
        }
        let mut messages = Vec::new();
        for parts in singles {
            let first = &parts[0].2;
            let digest_material = parts
                .iter()
                .map(|(_, p, _)| p.as_str())
                .collect::<Vec<_>>()
                .join(":");
            messages.push(NativeSms {
                path: format!(
                    "{}:sms:{:x}",
                    self.spec.selector(),
                    md5::compute(digest_material)
                ),
                number: first.originator.clone(),
                timestamp: first.service_center_timestamp.clone(),
                content: parts.iter().map(|(_, _, d)| d.text.as_str()).collect(),
                smsc: String::new(),
                sim_imsi: sim_imsi.clone(),
                cleanup_pending: false,
                parts: parts
                    .into_iter()
                    .map(|(index, pdu, _)| (index, pdu))
                    .collect(),
            });
        }
        let mut cache = self.sms_cache.lock().await;
        cache.retain(|_, sms| sms.cleanup_pending && sms.sim_imsi == sim_imsi);
        for sms in &messages {
            cache.entry(sms.path.clone()).or_insert_with(|| sms.clone());
        }
        Ok(cache.values().cloned().collect())
    }

    pub async fn message(self: &Arc<Self>, path: &str) -> Result<NativeSms, NativeError> {
        self.sms_cache
            .lock()
            .await
            .get(path)
            .cloned()
            .ok_or_else(|| NativeError::Unavailable("native_sms_not_in_current_scan".into()))
    }

    pub async fn delete_message(self: &Arc<Self>, path: &str) -> Result<(), NativeError> {
        self.require_sms_reception()?;
        let message = self.message(path).await?;
        let this = self.clone();
        tokio::spawn(async move {
            let _gate = this.operation.lock().await;
            if super::is_shutting_down() {
                return Err(NativeError::Unavailable(
                    "native_backend_shutting_down".into(),
                ));
            }
            // Verify SIM + stored content and delete under ONE physical lease.
            // eSIM switching/another native scan cannot interleave these steps.
            let imsi = this.io.execute(&this.at_request("AT+CIMI")?).await?;
            if imsi_response(&imsi)? != message.sim_imsi {
                this.sms_cache.lock().await.remove(&message.path);
                return Err(NativeError::OwnerConflict("native_sms_sim_changed".into()));
            }
            this.io.execute(&this.at_request("AT+CMGF=0")?).await?;
            let output = this.io.execute(&this.at_request("AT+CMGL=4")?).await?;
            let current = parse_stored_pdus(&output)?
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            for (index, pdu) in &message.parts {
                if current.get(index).is_some_and(|value| value != pdu) {
                    this.sms_cache.lock().await.remove(&message.path);
                    return Err(NativeError::OwnerConflict(
                        "native_sms_storage_generation_changed".into(),
                    ));
                }
            }
            if let Some(cached) = this.sms_cache.lock().await.get_mut(&message.path) {
                cached.cleanup_pending = true;
            }
            for (index, _) in &message.parts {
                if current.contains_key(index) {
                    this.io
                        .execute(&this.at_request(&format!("AT+CMGD={index}"))?)
                        .await?;
                }
                // A partial cleanup is retryable without re-deleting old indices.
                if let Some(cached) = this.sms_cache.lock().await.get_mut(&message.path) {
                    cached.parts.retain(|(part, _)| part != index);
                }
            }
            this.sms_cache.lock().await.remove(&message.path);
            Ok(())
        })
        .await
        .map_err(|_| NativeError::CommandFailed("native_sms_cleanup_task_failed"))?
    }
}

fn imsi_response(output: &str) -> Result<String, NativeError> {
    output
        .lines()
        .map(str::trim)
        .find(|s| (5..=15).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_string)
        .ok_or_else(|| NativeError::Unavailable("native_sms_sim_identity_unavailable".into()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 || text.len() > 1024 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

fn parse_stored_pdus(output: &str) -> Result<Vec<(u32, String)>, NativeError> {
    let mut pending = None;
    let mut pdus = Vec::new();
    for line in output.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix("+CMGL:") {
            let fields = csv(header.trim())?;
            pending = fields.first().and_then(|s| s.parse::<u32>().ok());
        } else if !line.is_empty() && unhex(line).is_some() {
            if let Some(index) = pending.take() {
                pdus.push((index, line.to_ascii_uppercase()));
            }
        }
    }
    Ok(pdus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sms_encoder_uses_real_modem_pdu_framing_and_handles_multipart_unicode() {
        let pdus =
            sms_codec::build_modem_submit_pdus("+12345678", &"测试".repeat(80), "+12345").unwrap();
        assert!(pdus.len() > 1);
        for pdu in pdus {
            let tpdu = &pdu[1 + usize::from(pdu[0])..];
            assert_eq!(tpdu[0] & 0x03, 1);
            assert_ne!(tpdu[0] & 0x40, 0);
            assert_eq!(unhex(&hex(&pdu)).unwrap(), pdu);
        }
    }

    #[test]
    fn stored_message_parser_does_not_treat_urcs_or_status_lines_as_pdus() {
        let result = parse_stored_pdus("+CMGL: 7,0,,4\r\n+CEREG: 1\r\n00112233\r\nOK\r\n").unwrap();
        assert_eq!(result, vec![(7, "00112233".into())]);
        assert!(unhex("00;AT").is_none());
    }

    struct EventsIo(std::sync::Mutex<Vec<super::super::protocol::CommandRequest>>);
    impl super::super::io::NativeIo for EventsIo {
        fn execute<'a>(
            &'a self,
            request: &'a super::super::protocol::CommandRequest,
        ) -> crate::hardware::devices::transport::TransportFuture<'a, Result<String, NativeError>>
        {
            Box::pin(async move {
                self.0.lock().unwrap().push(request.clone());
                let events = crate::hardware::cellular::at_urc::UrcEvents {
                    sms_stored: true,
                    ..Default::default()
                };
                Ok(serde_json::to_string(&events).unwrap())
            })
        }
    }

    #[tokio::test]
    async fn sms_event_poll_uses_only_the_explicit_at_endpoint_and_passive_tool() {
        use super::super::config::{NativeDeviceConfig, NativeProtocol};
        let io = Arc::new(EventsIo(Default::default()));
        let device = NativeDevice::new(
            NativeDeviceConfig {
                hardware_key: "fixture-events".into(),
                sysfs_anchor: "/sys/devices/fixture".into(),
                protocol: NativeProtocol::Qmi,
                control_device: "/dev/fixture-qmi".into(),
                at_device: Some("/dev/fixture-at".into()),
                sms_reception_enabled: true,
                uim_slot: 1,
                ims: None,
                data: None,
            },
            io.clone(),
        );
        assert!(device.poll_sms_events().await.unwrap().sms_stored);
        let requests = io.0.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].tool, Tool::AtPoll);
        assert_eq!(requests[0].device, "/dev/fixture-at");
        assert_eq!(requests[0].arguments, vec!["poll-urcs"]);
    }

    struct ChangedSimIo(std::sync::Mutex<Vec<String>>);
    impl super::super::io::NativeIo for ChangedSimIo {
        fn execute<'a>(
            &'a self,
            request: &'a super::super::protocol::CommandRequest,
        ) -> crate::hardware::devices::transport::TransportFuture<'a, Result<String, NativeError>>
        {
            Box::pin(async move {
                self.0.lock().unwrap().push(request.arguments[0].clone());
                Ok("001012222222222\r\nOK".into())
            })
        }
    }
    #[tokio::test]
    async fn sms_cleanup_refuses_a_replaced_sim_before_any_delete() {
        use super::super::config::{NativeDeviceConfig, NativeProtocol};
        let io = Arc::new(ChangedSimIo(Default::default()));
        let device = NativeDevice::new(
            NativeDeviceConfig {
                hardware_key: "fixture-sms".into(),
                sysfs_anchor: "/sys/devices/fixture".into(),
                protocol: NativeProtocol::At,
                control_device: "/dev/fixture".into(),
                at_device: None,
                sms_reception_enabled: true,
                uim_slot: 1,
                ims: None,
                data: None,
            },
            io.clone(),
        );
        let path = format!("{}:sms:fixture", device.spec.selector());
        device.sms_cache.lock().await.insert(
            path.clone(),
            NativeSms {
                path: path.clone(),
                number: "12345".into(),
                content: "fixture".into(),
                timestamp: "2026-01-01T00:00:00Z".into(),
                smsc: String::new(),
                sim_imsi: "001011111111111".into(),
                cleanup_pending: false,
                parts: vec![(1, "001122".into())],
            },
        );
        assert_eq!(
            device.delete_message(&path).await.unwrap_err(),
            NativeError::OwnerConflict("native_sms_sim_changed".into())
        );
        assert_eq!(*io.0.lock().unwrap(), vec!["AT+CIMI"]);
    }

    #[tokio::test]
    async fn disabled_native_sms_reception_never_initializes_scans_or_deletes() {
        use super::super::config::{NativeDeviceConfig, NativeProtocol};
        let io = Arc::new(ChangedSimIo(Default::default()));
        let device = NativeDevice::new(
            NativeDeviceConfig {
                hardware_key: "fixture-readonly-at".into(),
                sysfs_anchor: "/sys/devices/fixture".into(),
                protocol: NativeProtocol::At,
                control_device: "/dev/fixture".into(),
                at_device: None,
                sms_reception_enabled: false,
                uim_slot: 1,
                ims: None,
                data: None,
            },
            io.clone(),
        );
        assert!(matches!(
            device.initialize_sms().await,
            Err(NativeError::Unsupported("native_sms_reception_disabled"))
        ));
        assert!(matches!(
            device.poll_sms_events().await,
            Err(NativeError::Unsupported("native_sms_reception_disabled"))
        ));
        assert!(matches!(
            device.messages().await,
            Err(NativeError::Unsupported("native_sms_reception_disabled"))
        ));
        assert!(matches!(
            device.delete_message("unused").await,
            Err(NativeError::Unsupported("native_sms_reception_disabled"))
        ));
        assert!(io.0.lock().unwrap().is_empty());
    }
}
