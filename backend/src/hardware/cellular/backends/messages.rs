//! Native modem SMS/USSD. Stored SMS is polled and only removed after the
//! shared application ingest has durably accepted it; no MM event source.

use super::{
    native::NativeDevice,
    protocol::{at_payload, csv, phone_number, Tool},
    NativeError,
};
use crate::connectivity::core::sms_codec;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone)]
pub struct NativeSms {
    pub path: String,
    pub number: String,
    pub content: String,
    pub timestamp: String,
    pub smsc: String,
    parts: Vec<(u32, String)>,
}

impl NativeDevice {
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

    pub async fn initialize_sms(self: &Arc<Self>) -> Result<(), NativeError> {
        self.verify_primary_slot().await?;
        self.commands(vec![
            self.at_request("AT+CMGF=0")?,
            // Store MT messages; a missed URC must be recoverable by polling.
            self.at_request("AT+CNMI=2,1,0,1,0")?,
        ])
        .await
        .map(|_| ())
    }

    async fn raw_messages(self: &Arc<Self>) -> Result<Vec<(u32, String)>, NativeError> {
        self.verify_primary_slot().await?;
        let output = self
            .commands(vec![
                self.at_request("AT+CMGF=0")?,
                self.at_request("AT+CMGL=4")?,
            ])
            .await?;
        parse_stored_pdus(&output[1])
    }

    pub async fn messages(self: &Arc<Self>) -> Result<Vec<NativeSms>, NativeError> {
        let pdus = self.raw_messages().await?;
        let mut singles = Vec::new();
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
                let entry = groups
                    .entry((
                        delivery.originator.clone(),
                        reference,
                        delivery.segment_total,
                    ))
                    .or_default();
                // Conflicting reused references are not permission to merge
                // unrelated messages. Leave them stored for explicit recovery.
                if entry.contains_key(&delivery.segment_sequence) {
                    entry.clear();
                    continue;
                }
                entry.insert(delivery.segment_sequence, (index, pdu, delivery));
            } else {
                singles.push(vec![(index, pdu, delivery)]);
            }
        }
        for ((_, _, total), entries) in groups {
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
                parts: parts
                    .into_iter()
                    .map(|(index, pdu, _)| (index, pdu))
                    .collect(),
            });
        }
        let mut cache = self.sms_cache.lock().await;
        cache.clear();
        for sms in &messages {
            cache.insert(sms.path.clone(), sms.clone());
        }
        Ok(messages)
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
        let message = self.message(path).await?;
        let current = self
            .raw_messages()
            .await?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut requests = Vec::new();
        for (index, pdu) in &message.parts {
            if current.get(index) != Some(pdu) {
                return Err(NativeError::OwnerConflict(
                    "native_sms_storage_generation_changed".into(),
                ));
            }
            requests.push(self.at_request(&format!("AT+CMGD={index}"))?);
        }
        self.commands(requests).await?;
        self.sms_cache.lock().await.remove(path);
        Ok(())
    }
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
}
