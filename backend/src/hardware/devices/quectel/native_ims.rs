//! Qualified EC20-CE-FAG read-only IMS status codec.
//!
//! Evidence: EC20CEFAGR08A03M4G firmware getter dispatch 0x82d553e4.
//! Only the ONE-argument query is allowed; adding flag arguments writes EFS.
//! Runtime model/revision strings select this schema, not cryptographic attestation.

use crate::hardware::cellular::native_ims::{
    NativeImsProbeError, NativeImsRegistration, NativeImsSample, NativeImsService,
    NATIVE_IMS_SCHEMA,
};

pub(crate) const MODEL_COMMAND: &str = "AT+CGMM";
pub(crate) const REVISION_COMMAND: &str = "AT+CGMR";
pub(crate) const STATUS_COMMAND: &str = "AT+QIMSCFG=\"ims_status\"";
pub(crate) const ICCID_COMMAND: &str = "AT+QCCID";
const MAX_REPLY: usize = 4096;

/// Known interleaved URCs are not evidence for the solicited response. Most
/// are already removed by AtSession; the two IMS URCs are not currently routed.
fn is_urc(line: &str) -> bool {
    matches!(line, "RING" | "NO CARRIER" | "BUSY" | "NO ANSWER" | "NO DIALTONE" | "NO DIAL TONE")
        || [
            "+QIMSREGSTAT:", "+QIMSSRVSTAT:", "+CMTI:", "+CDSI:",
            "+CLIP:", "+CRING:", "+CCWA:", "+COLP:", "+CUSD:",
            "+CREG:", "+CGREG:", "+CEREG:", "+C5GREG:", "+CPIN:", "+QIND:",
        ].iter().any(|prefix| line.starts_with(prefix))
}

/// NativeIo/SystemNativeIo returns AtSession's completed response INCLUDING
/// final OK. Reject bare OK, errors and missing completion, never infer success
/// merely from a status-looking line. Bound size before collecting fields.
fn response_rows<'a>(text: &'a str, command: &str) -> Result<Vec<&'a str>, NativeImsProbeError> {
    let bad = NativeImsProbeError::MalformedResponse;
    if text.len() > MAX_REPLY || !text.is_ascii() {
        return Err(bad);
    }
    let mut complete = false;
    let mut rows = Vec::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || is_urc(line) {
            continue;
        }
        if complete {
            return Err(bad);
        }
        if line == command {
            continue;
        }
        if line == "OK" {
            complete = true;
        } else if line == "ERROR" || line.starts_with("+CME ERROR") || line.starts_with("+CMS ERROR") {
            return Err(bad);
        } else {
            rows.push(line);
        }
    }
    if !complete || rows.is_empty() {
        return Err(bad);
    }
    Ok(rows)
}

pub(crate) fn qualify(model: &str, revision: &str) -> Result<(), NativeImsProbeError> {
    let model = response_rows(model, MODEL_COMMAND)
        .map_err(|_| NativeImsProbeError::FirmwareNotQualified)?;
    let revision = response_rows(revision, REVISION_COMMAND)
        .map_err(|_| NativeImsProbeError::FirmwareNotQualified)?;
    // Deliberately exact, not the broader Quectel family classifier. Other
    // models/builds need their own evidence and are not queried speculatively.
    let model_ok = model.len() == 1 && matches!(model[0], "EC20" | "EC20F" | "EC20CEFAG");
    let revision_ok = revision.len() == 1
        && matches!(revision[0], "EC20CEFAGR08A03M4G" | "Revision: EC20CEFAGR08A03M4G");
    if model_ok && revision_ok {
        Ok(())
    } else {
        Err(NativeImsProbeError::FirmwareNotQualified)
    }
}

pub(crate) fn parse_status(text: &str) -> Result<NativeImsSample, NativeImsProbeError> {
    let bad = NativeImsProbeError::MalformedResponse;
    let rows = response_rows(text, STATUS_COMMAND)?;
    if rows.len() != 1 {
        return Err(bad);
    }
    let fields = rows[0].strip_prefix("+QIMSCFG:").ok_or(bad)?.trim()
        .strip_prefix("\"ims_status\"").ok_or(bad)?.trim_start()
        .strip_prefix(',').ok_or(bad)?;
    let fields: Vec<&str> = fields.split(',').map(str::trim).collect();
    if fields.len() != 6 {
        return Err(bad);
    }
    let mut values = [0u8; 6];
    for (value, field) in values.iter_mut().zip(fields) {
        if field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad);
        }
        *value = field.parse::<u8>().map_err(|_| bad)?;
    }
    if values[0] > 1 || values[1] > 1 {
        return Err(bad);
    }
    let registration = match values[2] {
        // Absent optional TLV leaves initialized zero and still returns success.
        0 => NativeImsRegistration::UnknownOrUnreported,
        1 => NativeImsRegistration::Registering,
        2 => NativeImsRegistration::Registered,
        3 => NativeImsRegistration::Limited,
        _ => NativeImsRegistration::Unknown,
    };
    let service = |value| match value {
        0 => NativeImsService::UnknownOrUnreported,
        1 => NativeImsService::Limited,
        2 => NativeImsService::Available,
        _ => NativeImsService::Unknown,
    };
    Ok(NativeImsSample {
        schema: NATIVE_IMS_SCHEMA,
        registration_reporting: values[0] == 1,
        service_reporting: values[1] == 1,
        registration,
        sms: service(values[3]),
        voice: service(values[4]),
        ut: service(values[5]),
    })
}

/// ICCID remains private to the checked transaction. Validate shape before
/// using the application's established (19-digit) binding normalization.
pub(crate) fn normalized_iccid(value: &str) -> Option<String> {
    let value = value.trim();
    let digits = value.strip_suffix('F').or_else(|| value.strip_suffix('f')).unwrap_or(value);
    if !(19..=20).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(crate::platform::utils::normalize_iccid(digits))
}

pub(crate) fn parse_iccid(text: &str) -> Result<String, NativeImsProbeError> {
    let rows = response_rows(text, ICCID_COMMAND)
        .map_err(|_| NativeImsProbeError::BindingChanged)?;
    if rows.len() != 1 {
        return Err(NativeImsProbeError::BindingChanged);
    }
    rows[0].strip_prefix("+QCCID:").and_then(normalized_iccid)
        .ok_or(NativeImsProbeError::BindingChanged)
}

#[cfg(test)]
#[path = "native_ims_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "native_ims_serial_tests.rs"]
mod serial_tests;
