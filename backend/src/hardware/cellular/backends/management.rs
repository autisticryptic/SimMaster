//! Native NAS RAT, operator and band preferences.
//! Message/TLV definitions follow the public libqmi NAS/DMS schemas. All
//! capability decisions are made from the device response, never SIM identity.

use super::{
    config::NativeProtocol,
    native::NativeDevice,
    protocol::{CommandRequest, Tool},
    NativeError,
};
use crate::api::models::{BandLockRequest, BandLockStatus, RadioMode, RadioModeResponse};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

type Fields = BTreeMap<u8, Vec<u8>>;

pub fn validate_fields(message: u16, fields: &[(u8, Vec<u8>)]) -> Result<(), NativeError> {
    if message != 0x33 {
        return if fields.is_empty() {
            Ok(())
        } else {
            Err(NativeError::Protocol(
                "native_qmi_query_fields_invalid".into(),
            ))
        };
    }
    let mut seen = BTreeSet::new();
    if fields.is_empty() || fields.len() > 8 {
        return Err(NativeError::Protocol(
            "native_qmi_preference_fields_invalid".into(),
        ));
    }
    for (kind, value) in fields {
        let length = match kind {
            0x11 => 2,
            0x15 => 8,
            0x16 => 5,
            0x17 | 0x1a => 1,
            0x24 => 32,
            0x2f | 0x30 => 64,
            _ => {
                return Err(NativeError::Protocol(
                    "native_qmi_preference_field_not_allowed".into(),
                ))
            }
        };
        if value.len() != length || !seen.insert(*kind) {
            return Err(NativeError::Protocol(
                "native_qmi_preference_field_invalid".into(),
            ));
        }
    }
    Ok(())
}

impl NativeDevice {
    async fn management(
        self: &Arc<Self>,
        action: &str,
        fields: Vec<(u8, Vec<u8>)>,
    ) -> Result<Fields, NativeError> {
        if self.spec.protocol != NativeProtocol::Qmi {
            return Err(NativeError::Unsupported(
                "native_management_requires_qmi_capability",
            ));
        }
        let request = CommandRequest {
            tool: Tool::QmiControl,
            device: self.spec.control_device.clone(),
            arguments: vec![
                action.into(),
                serde_json::to_string(&fields)
                    .map_err(|_| NativeError::Protocol("native_qmi_request_encoding".into()))?,
            ],
            timeout_seconds: 45,
        };
        let value = self.command(request).await?;
        let fields: Vec<(u8, Vec<u8>)> = serde_json::from_str(&value)
            .map_err(|_| NativeError::Protocol("native_qmi_response_invalid".into()))?;
        Ok(fields.into_iter().collect())
    }

    async fn rat_capabilities(self: &Arc<Self>) -> Result<u16, NativeError> {
        if self.spec.protocol != NativeProtocol::Qmi {
            return Err(NativeError::Unsupported(
                "native_rat_preference_not_supported_on_protocol",
            ));
        }
        let text = self.command(self.request("--dms-get-capabilities")).await?;
        let mask = [
            "cdma-1x",
            "cdma-1xevdo",
            "gsm",
            "umts",
            "lte",
            "td-scdma",
            "5gnr",
        ]
        .iter()
        .enumerate()
        .filter(|(_, name)| text.contains(&format!("'{name}'")))
        .fold(0u16, |mask, (bit, _)| mask | (1 << bit));
        if mask == 0 {
            return Err(NativeError::Unavailable(
                "native_rat_capabilities_unknown".into(),
            ));
        }
        Ok(mask)
    }

    pub async fn radio_mode(self: &Arc<Self>) -> Result<RadioModeResponse, NativeError> {
        let supported = self.rat_capabilities().await?;
        let fields = self.management("get-preferences", Vec::new()).await?;
        let mask = fields
            .get(&0x11)
            .filter(|v| v.len() == 2)
            .map(|v| u16::from_le_bytes([v[0], v[1]]))
            .ok_or_else(|| NativeError::Unavailable("native_rat_preference_unknown".into()))?;
        let mut modes = vec!["auto".into()];
        if supported & (1 << 4) != 0 {
            modes.push("lte".into());
        }
        if supported & (1 << 6) != 0 {
            modes.push("nr".into());
        }
        Ok(RadioModeResponse {
            mode: match mask {
                16 => "lte",
                64 => "nr",
                _ => "auto",
            }
            .into(),
            technology_preference: self.network().await?.technology,
            supported_modes: modes,
        })
    }

    pub async fn set_radio_mode(self: &Arc<Self>, mode: RadioMode) -> Result<(), NativeError> {
        let supported = self.rat_capabilities().await?;
        let wanted = match mode {
            RadioMode::Auto => supported,
            RadioMode::LteOnly => 1 << 4,
            RadioMode::NrOnly => 1 << 6,
        };
        if wanted & supported != wanted {
            return Err(NativeError::Unsupported(
                "native_rat_not_supported_by_device",
            ));
        }
        self.management(
            "set-preferences",
            vec![(0x11, wanted.to_le_bytes().to_vec()), (0x17, vec![1])],
        )
        .await
        .map(|_| ())
    }

    pub async fn select_qmi_operator(self: &Arc<Self>, plmn: &str) -> Result<(), NativeError> {
        let mut value = vec![u8::from(!plmn.is_empty())];
        let (mcc, mnc) = if plmn.is_empty() {
            (0u16, 0u16)
        } else {
            if !super::protocol::valid_plmn(plmn) {
                return Err(NativeError::Protocol("native_operator_plmn_invalid".into()));
            }
            (
                plmn[..3]
                    .parse::<u16>()
                    .map_err(|_| NativeError::Protocol("native_mcc_invalid".into()))?,
                plmn[3..]
                    .parse::<u16>()
                    .map_err(|_| NativeError::Protocol("native_mnc_invalid".into()))?,
            )
        };
        value.extend(mcc.to_le_bytes());
        value.extend(mnc.to_le_bytes());
        let mut fields = vec![(0x16, value), (0x17, vec![1])];
        if !plmn.is_empty() {
            fields.push((0x1a, vec![u8::from(plmn.len() == 6)]));
        }
        self.management("set-preferences", fields).await.map(|_| ())
    }

    pub async fn bands(self: &Arc<Self>) -> Result<BandLockStatus, NativeError> {
        let supported = self.management("band-capabilities", Vec::new()).await?;
        let current = self.management("get-preferences", Vec::new()).await?;
        band_status(&supported, &current)
    }

    pub async fn set_bands(self: &Arc<Self>, request: &BandLockRequest) -> Result<(), NativeError> {
        let capabilities = self.management("band-capabilities", Vec::new()).await?;
        let current = self.management("get-preferences", Vec::new()).await?;
        let fields = band_write_fields(&capabilities, &current, request)?;
        self.management("set-preferences", fields).await.map(|_| ())
    }
}

fn mask_bands(bytes: &[u8]) -> BTreeSet<u32> {
    bytes
        .iter()
        .enumerate()
        .flat_map(|(i, b)| {
            (0..8)
                .filter(move |n| b & (1 << n) != 0)
                .map(move |n| (i * 8 + n + 1) as u32)
        })
        .collect()
}

fn array_bands(bytes: &[u8]) -> Result<BTreeSet<u32>, NativeError> {
    if bytes.len() < 2 {
        return Err(NativeError::Protocol(
            "native_band_capabilities_invalid".into(),
        ));
    }
    let count = usize::from(u16::from_le_bytes([bytes[0], bytes[1]]));
    if bytes.len() != 2 + 2 * count || count > 512 {
        return Err(NativeError::Protocol(
            "native_band_capabilities_invalid".into(),
        ));
    }
    Ok(bytes[2..]
        .chunks_exact(2)
        .map(|v| u16::from_le_bytes([v[0], v[1]]) as u32)
        .filter(|n| *n > 0)
        .collect())
}

fn capabilities(fields: &Fields) -> Result<(BTreeSet<u32>, BTreeSet<u32>), NativeError> {
    let lte = if let Some(bytes) = fields.get(&0x12) {
        array_bands(bytes)?
    } else {
        fields
            .get(&0x10)
            .filter(|b| b.len() == 8)
            .map(|b| mask_bands(b))
            .unwrap_or_default()
    };
    let nr = fields
        .get(&0x13)
        .map(|b| array_bands(b))
        .transpose()?
        .unwrap_or_default();
    if lte.is_empty() && nr.is_empty() {
        return Err(NativeError::Unsupported(
            "native_band_capabilities_unavailable",
        ));
    }
    Ok((lte, nr))
}

fn selected(
    fields: &Fields,
    lte: &BTreeSet<u32>,
    nr: &BTreeSet<u32>,
) -> Result<(BTreeSet<u32>, BTreeSet<u32>), NativeError> {
    let lte_current = if lte.is_empty() {
        BTreeSet::new()
    } else {
        let value = fields
            .get(&0x23)
            .filter(|v| v.len() == 32)
            .or_else(|| fields.get(&0x15).filter(|v| v.len() == 8))
            .ok_or_else(|| NativeError::Unavailable("native_lte_band_preference_unknown".into()))?;
        mask_bands(value).intersection(lte).copied().collect()
    };
    let nr_current = if nr.is_empty() {
        BTreeSet::new()
    } else {
        let sa = fields
            .get(&0x2c)
            .filter(|v| v.len() == 64)
            .map(|v| mask_bands(v));
        let nsa = fields
            .get(&0x2d)
            .filter(|v| v.len() == 64)
            .map(|v| mask_bands(v));
        if sa.is_some() && nsa.is_some() && sa != nsa {
            return Err(NativeError::Unavailable(
                "native_nr_sa_nsa_preferences_differ".into(),
            ));
        }
        sa.or(nsa)
            .ok_or_else(|| NativeError::Unavailable("native_nr_band_preference_unknown".into()))?
            .intersection(nr)
            .copied()
            .collect()
    };
    Ok((lte_current, nr_current))
}

fn split_bands(bands: &BTreeSet<u32>, nr: bool) -> (Vec<u32>, Vec<u32>) {
    bands.iter().copied().partition(|band| {
        if nr {
            !([
                34, 38, 39, 40, 41, 46, 47, 48, 50, 51, 53, 54, 77, 78, 79, 90, 96, 101, 102, 104,
            ]
            .contains(band)
                || *band >= 257)
        } else {
            !(33..=54).contains(band)
        }
    })
}

fn band_status(supported: &Fields, current: &Fields) -> Result<BandLockStatus, NativeError> {
    let (lte, nr) = capabilities(supported)?;
    let (current_lte, current_nr) = selected(current, &lte, &nr)?;
    let (supported_lte_fdd_bands, supported_lte_tdd_bands) = split_bands(&lte, false);
    let (supported_nr_fdd_bands, supported_nr_tdd_bands) = split_bands(&nr, true);
    let (lte_fdd_bands, lte_tdd_bands) = split_bands(&current_lte, false);
    let (nr_fdd_bands, nr_tdd_bands) = split_bands(&current_nr, true);
    Ok(BandLockStatus {
        locked: current_lte != lte || current_nr != nr,
        supported_lte_fdd_bands,
        supported_lte_tdd_bands,
        supported_nr_fdd_bands,
        supported_nr_tdd_bands,
        lte_fdd_bands,
        lte_tdd_bands,
        nr_fdd_bands,
        nr_tdd_bands,
    })
}

fn encode_mask(bands: &BTreeSet<u32>, length: usize) -> Result<Vec<u8>, NativeError> {
    let mut result = vec![0; length];
    for band in bands {
        if *band == 0 || *band > (8 * length) as u32 {
            return Err(NativeError::Unsupported(
                "native_band_mask_range_unsupported",
            ));
        }
        let bit = (*band - 1) as usize;
        result[bit / 8] |= 1 << (bit % 8);
    }
    Ok(result)
}

fn band_write_fields(
    caps: &Fields,
    current: &Fields,
    request: &BandLockRequest,
) -> Result<Vec<(u8, Vec<u8>)>, NativeError> {
    let (lte, nr) = capabilities(caps)?;
    let requested_lte = request
        .lte_fdd_bands
        .iter()
        .chain(&request.lte_tdd_bands)
        .copied()
        .collect::<BTreeSet<_>>();
    let requested_nr = request
        .nr_fdd_bands
        .iter()
        .chain(&request.nr_tdd_bands)
        .copied()
        .collect::<BTreeSet<_>>();
    if !requested_lte.is_subset(&lte) || !requested_nr.is_subset(&nr) {
        return Err(NativeError::Unsupported(
            "native_requested_band_not_supported",
        ));
    }
    let wanted_lte = if requested_lte.is_empty() {
        &lte
    } else {
        &requested_lte
    };
    let wanted_nr = if requested_nr.is_empty() {
        &nr
    } else {
        &requested_nr
    };
    let mut fields = vec![(0x17, vec![1])];
    if !lte.is_empty() {
        if current.get(&0x23).is_some_and(|v| v.len() == 32) {
            fields.push((0x24, encode_mask(wanted_lte, 32)?));
        } else if current.get(&0x15).is_some_and(|v| v.len() == 8) {
            fields.push((0x15, encode_mask(wanted_lte, 8)?));
        } else {
            return Err(NativeError::Unavailable(
                "native_lte_band_preference_unknown".into(),
            ));
        }
    }
    if !nr.is_empty() {
        let mut found = false;
        for (read, write) in [(0x2c, 0x2f), (0x2d, 0x30)] {
            if current.get(&read).is_some_and(|v| v.len() == 64) {
                fields.push((write, encode_mask(wanted_nr, 64)?));
                found = true;
            }
        }
        if !found {
            return Err(NativeError::Unavailable(
                "native_nr_band_preference_unknown".into(),
            ));
        }
    }
    validate_fields(0x33, &fields)?;
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_writes_are_typed_and_do_not_include_radio_enable_or_roaming_policy() {
        assert!(validate_fields(0x33, &[(0x10, vec![1])]).is_err());
        assert!(validate_fields(0x33, &[(0x14, vec![0, 0])]).is_err());
        assert!(validate_fields(0x33, &[(0x11, vec![16, 0])]).is_ok());
        assert!(validate_fields(0x34, &[(0x11, vec![16, 0])]).is_err());
    }
    #[test]
    fn band_masks_preserve_upper_bands_and_reject_unsupported_selections() {
        let bands = BTreeSet::from([1, 3, 41, 66]);
        assert_eq!(mask_bands(&encode_mask(&bands, 32).unwrap()), bands);
        assert!(encode_mask(&bands, 8).is_err());
        let caps = BTreeMap::from([(0x12, vec![4, 0, 1, 0, 3, 0, 41, 0, 66, 0])]);
        let current = BTreeMap::from([(0x23, encode_mask(&bands, 32).unwrap())]);
        assert!(!band_status(&caps, &current).unwrap().locked);
        let request = BandLockRequest {
            lte_fdd_bands: vec![66],
            ..Default::default()
        };
        let fields = band_write_fields(&caps, &current, &request).unwrap();
        assert_eq!(
            mask_bands(&fields.iter().find(|(kind, _)| *kind == 0x24).unwrap().1),
            BTreeSet::from([66])
        );
        let request = BandLockRequest {
            lte_fdd_bands: vec![99],
            ..Default::default()
        };
        assert!(band_write_fields(&caps, &current, &request).is_err());
    }
}
