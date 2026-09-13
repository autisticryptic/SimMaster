//! Native SIM access lease and AT logical-channel adapter.
//!
//! The existing QMI UIM codecs stay authoritative. Blocking SIM entry points
//! call this from their existing blocking worker, so the device gate covers
//! the complete open/APDU/close sequence, not isolated AT/QMI commands.

use super::{
    config::NativeProtocol,
    native::NativeDevice,
    protocol::{at_payload, csv},
};
use crate::connectivity::modems::ims::vowifi::qmi_uim::{
    self, UimApduResponse, UsimAkaApduResult, UsimEpdgConfig, UsimIdentity,
};
use std::sync::Arc;
use tokio::sync::OwnedMutexGuard;

pub struct SimLease {
    device: Arc<NativeDevice>,
    _guard: OwnedMutexGuard<()>,
}

impl SimLease {
    pub fn for_endpoint(path: &str, slot: u8) -> Result<Option<Self>, &'static str> {
        let Some(fleet) = super::active_native() else {
            return Ok(None);
        };
        let device = fleet
            .by_control_device(path)
            .map_err(|_| "native_sim_device_not_owned")?;
        if device.spec.uim_slot != slot {
            return Err("native_sim_slot_mismatch");
        }
        if device.spec.protocol != NativeProtocol::Qmi && slot != 1 {
            return Err("native_at_sim_slot_selection_requires_driver");
        }
        // These APIs were already blocking QMI transactions before native
        // routing. They must remain on spawn_blocking, never a Tokio worker.
        let guard = device.operation.clone().blocking_lock_owned();
        if super::is_shutting_down() {
            return Err("native_backend_shutting_down");
        }
        tokio::runtime::Handle::try_current()
            .map_err(|_| "native_sim_runtime_unavailable")?
            .block_on(device.io.verify_owner(path))
            .map_err(|_| "native_sim_owner_unavailable")?;
        Ok(Some(Self {
            device,
            _guard: guard,
        }))
    }

    pub fn uses_at(&self) -> bool {
        self.device.spec.protocol != NativeProtocol::Qmi
    }

    fn at(&self, command: &str) -> Result<String, &'static str> {
        let request = self
            .device
            .at_request(command)
            .map_err(|_| "native_sim_at_unavailable")?;
        tokio::runtime::Handle::try_current()
            .map_err(|_| "native_sim_runtime_unavailable")?
            .block_on(self.device.io.execute(&request))
            .map_err(|_| "native_sim_at_exchange_failed")
    }

    fn with_channel<T>(
        &self,
        aid: &[u8],
        run: impl FnOnce(&mut AtChannel<'_>) -> Result<T, &'static str>,
    ) -> Result<T, &'static str> {
        if aid.is_empty() || aid.len() > 32 {
            return Err("native_sim_aid_invalid");
        }
        let output = self.at(&format!("AT+CCHO=\"{}\"", hex(aid)))?;
        let channel = at_payload(&output, "+CCHO:")
            .or_else(|| {
                output
                    .lines()
                    .map(str::trim)
                    .find(|s| s.parse::<u32>().is_ok())
            })
            .and_then(|s| s.parse::<u32>().ok())
            .filter(|id| *id > 0)
            .ok_or("native_sim_channel_open_unconfirmed")?;
        let mut channel = AtChannel {
            lease: self,
            channel,
            closed: false,
        };
        let result = run(&mut channel);
        let close = channel.close();
        match result {
            Ok(value) => close.map(|_| value),
            Err(error) => {
                let _ = close;
                Err(error)
            }
        }
    }

    pub fn verify(&self, aid: &[u8]) -> Result<(), &'static str> {
        self.with_channel(aid, |_| Ok(()))
    }

    pub fn authenticate(
        &self,
        aid: &[u8],
        rand: &[u8],
        autn: &[u8],
    ) -> Result<UsimAkaApduResult, &'static str> {
        let apdu = qmi_uim::build_usim_authenticate_apdu(rand, autn)
            .map_err(|_| "native_sim_aka_input_invalid")?;
        self.with_channel(aid, |channel| {
            let response = channel.exchange(&apdu)?;
            qmi_uim::parse_usim_authenticate_response_reason(&response)
        })
    }

    pub fn identity(&self, aid: &[u8]) -> Result<UsimIdentity, &'static str> {
        self.with_channel(aid, |channel| {
            channel.select(0x6f07)?;
            let imsi = channel.exchange(&[0, 0xb0, 0, 0, 9])?;
            if (imsi.sw1, imsi.sw2) != (0x90, 0) {
                return Err("native_sim_imsi_read_rejected");
            }
            let imsi =
                qmi_uim::decode_ef_imsi(&imsi.data).map_err(|_| "native_sim_imsi_decode_failed")?;
            let mnc_length = channel
                .select(0x6fad)
                .ok()
                .and_then(|_| channel.exchange(&[0, 0xb0, 0, 0, 4]).ok())
                .filter(|r| (r.sw1, r.sw2) == (0x90, 0))
                .and_then(|r| qmi_uim::parse_ef_ad_mnc_length(&r.data));
            Ok(UsimIdentity { imsi, mnc_length })
        })
    }

    pub fn epdg(&self, aid: &[u8]) -> Result<UsimEpdgConfig, &'static str> {
        self.with_channel(aid, |channel| {
            let home_identifiers = channel
                .read_file(qmi_uim::EF_EPDG_ID)
                .ok()
                .flatten()
                .and_then(|b| qmi_uim::parse_ef_epdg_id(&b).ok())
                .unwrap_or_default();
            let selection = channel
                .read_file(qmi_uim::EF_EPDG_SELECTION)
                .ok()
                .flatten()
                .and_then(|b| qmi_uim::parse_ef_epdg_selection(&b).ok())
                .unwrap_or_default();
            Ok(UsimEpdgConfig {
                home_identifiers,
                selection,
            })
        })
    }
}

struct AtChannel<'a> {
    lease: &'a SimLease,
    channel: u32,
    closed: bool,
}
impl AtChannel<'_> {
    fn close(&mut self) -> Result<(), &'static str> {
        self.closed = true; // No automatic duplicate close on an ambiguous result.
        self.lease
            .at(&format!("AT+CCHC={}", self.channel))
            .map(|_| ())
    }

    fn exchange(&mut self, apdu: &[u8]) -> Result<UimApduResponse, &'static str> {
        let mut command = apdu.to_vec();
        let mut data = Vec::new();
        for _ in 0..8 {
            let body = hex(&command);
            let text = self.lease.at(&format!(
                "AT+CGLA={},{},\"{}\"",
                self.channel,
                body.len(),
                body
            ))?;
            let response = parse_cgla(&text)?;
            if response.sw1 == 0x6c {
                *command.last_mut().ok_or("native_sim_apdu_invalid")? = response.sw2;
                continue;
            }
            data.extend_from_slice(&response.data);
            if data.len() > 8192 {
                return Err("native_sim_response_too_large");
            }
            if matches!(response.sw1, 0x61 | 0x9f) {
                command = qmi_uim::build_get_response_apdu(response.sw2);
                continue;
            }
            return Ok(UimApduResponse {
                data,
                sw1: response.sw1,
                sw2: response.sw2,
            });
        }
        Err("native_sim_apdu_followup_limit")
    }

    fn select(&mut self, file: u16) -> Result<UimApduResponse, &'static str> {
        let response = self.exchange(&[0, 0xa4, 0, 4, 2, (file >> 8) as u8, file as u8, 0])?;
        if (response.sw1, response.sw2) != (0x90, 0) {
            return Err("native_sim_file_select_rejected");
        }
        Ok(response)
    }

    fn read_file(&mut self, file: u16) -> Result<Option<Vec<u8>>, &'static str> {
        let selected = match self.select(file) {
            Ok(value) => value,
            Err(_) => return Ok(None),
        };
        let size = qmi_uim::parse_fcp_file_size(&selected.data)
            .filter(|n| *n <= 4096)
            .ok_or("native_sim_file_size_unknown")?;
        let mut data = Vec::with_capacity(size);
        while data.len() < size {
            let offset = data.len();
            let length = (size - offset).min(255);
            let response =
                self.exchange(&[0, 0xb0, (offset >> 8) as u8, offset as u8, length as u8])?;
            if (response.sw1, response.sw2) != (0x90, 0) || response.data.len() != length {
                return Err("native_sim_file_read_incomplete");
            }
            data.extend(response.data);
        }
        Ok(Some(data))
    }
}
impl Drop for AtChannel<'_> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.close();
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

fn parse_cgla(text: &str) -> Result<UimApduResponse, &'static str> {
    let fields = csv(at_payload(text, "+CGLA:").ok_or("native_sim_cgla_missing")?)
        .map_err(|_| "native_sim_cgla_invalid")?;
    let value = fields.get(1).ok_or("native_sim_cgla_invalid")?;
    if fields[0].parse::<usize>().ok() != Some(value.len())
        || value.len() < 4
        || value.len() > 16384
        || value.len() % 2 != 0
        || !value.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err("native_sim_cgla_invalid");
    }
    let mut bytes = (0..value.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&value[i..i + 2], 16).map_err(|_| "native_sim_cgla_invalid"))
        .collect::<Result<Vec<_>, _>>()?;
    let sw2 = bytes.pop().ok_or("native_sim_cgla_invalid")?;
    let sw1 = bytes.pop().ok_or("native_sim_cgla_invalid")?;
    Ok(UimApduResponse {
        data: bytes,
        sw1,
        sw2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn at_apdu_parser_validates_lengths_and_preserves_status() {
        let response = parse_cgla("+CGLA: 8,\"01029000\"").unwrap();
        assert_eq!(response.data, vec![1, 2]);
        assert_eq!((response.sw1, response.sw2), (0x90, 0));
        assert!(parse_cgla("+CGLA: 4,\"01029000\"").is_err());
        assert!(parse_cgla("+CGLA: 4,\"ZZZZ\"").is_err());
    }
}
