//! This line's own telephone number, as learned from an IMS registration.
//!
//! # Why this exists
//!
//! On a data-only line the SIM does not tell us the subscriber's number:
//! `EF-MSISDN` is commonly unprogrammed, so `AT+CNUM` comes back empty and
//! ModemManager's `own-numbers` property is unset. USSD would need a circuit
//! this bearer does not provide.
//!
//! The one place the number *is* observable is the registrar's answer. A
//! successful REGISTER carries `P-Associated-URI` (TS 24.229 §5.1.1.2), and an
//! operator returns both the IMSI-derived IMPU and the MSISDN-associated one —
//! `<tel:+60174231067>` in the observed Maxis answer. That is the number.
//!
//! Both access legs learn it independently and neither can reach the database
//! from where it learns it (the VoWiFi leg has no `Database` handle at all), so
//! they publish here instead. The API layer reads this when assembling SIM info
//! and persists it into the ordinary own-number cache, which is what every other
//! reader — the UI, notification templates, device status — already consults.
//!
//! Numbers here are *observed facts*, never a user's manual entry: a manual
//! value in the cache is authoritative and must not be overwritten by this.

use std::{
    collections::HashMap,
    sync::{OnceLock, RwLock},
};

/// Source label written into the own-number cache for values learned this way,
/// distinguishing them from `manual`, `dbus` and `protocol`.
pub const IMS_NUMBER_SOURCE: &str = "ims_associated_uri";

static OBSERVED: OnceLock<RwLock<HashMap<String, Vec<String>>>> = OnceLock::new();

fn observed() -> &'static RwLock<HashMap<String, Vec<String>>> {
    OBSERVED.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Publish the telephone identities a registrar returned for one line.
///
/// Empty input is ignored rather than stored: a refresh that returns no
/// telephone identity must not erase a number an earlier registration proved.
/// A non-empty set replaces the previous one, because the registrar's current
/// answer is the authority on what this line's numbers are.
pub fn record(line_id: &str, numbers: Vec<String>) {
    if numbers.is_empty() {
        return;
    }
    let line_id = line_id.trim();
    if line_id.is_empty() {
        return;
    }
    let mut guard = observed()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(line_id.to_string(), numbers);
}

/// Telephone identities observed for one line, or empty when none yet.
pub fn for_line(line_id: &str) -> Vec<String> {
    observed()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(line_id.trim())
        .cloned()
        .unwrap_or_default()
}

/// Drop what was observed for one line. Used when a line is unregistered or its
/// SIM changes, so a stale number cannot outlive the registration that proved it.
pub fn clear(line_id: &str) {
    let mut guard = observed()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.remove(line_id.trim());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each test uses its own line id: the registry is process-global.
    fn line(name: &str) -> String {
        format!("line-own-numbers-{name}")
    }

    #[test]
    fn recorded_numbers_are_readable_for_that_line_only() {
        let a = line("readable-a");
        let b = line("readable-b");
        record(&a, vec!["+60174231067".to_string()]);
        assert_eq!(for_line(&a), vec!["+60174231067".to_string()]);
        assert!(
            for_line(&b).is_empty(),
            "one line's number must not leak into another"
        );
        clear(&a);
    }

    #[test]
    fn an_empty_refresh_never_erases_a_proven_number() {
        // A later REGISTER that returns no telephone identity is not evidence
        // that the line has no number, so it must not clear one.
        let id = line("empty-refresh");
        record(&id, vec!["+60174231067".to_string()]);
        record(&id, Vec::new());
        assert_eq!(for_line(&id), vec!["+60174231067".to_string()]);
        clear(&id);
    }

    #[test]
    fn a_new_answer_replaces_the_previous_set() {
        let id = line("replace");
        record(&id, vec!["+60174231067".to_string()]);
        record(
            &id,
            vec!["+60111111111".to_string(), "+60222222222".to_string()],
        );
        assert_eq!(
            for_line(&id),
            vec!["+60111111111".to_string(), "+60222222222".to_string()]
        );
        clear(&id);
    }

    #[test]
    fn clear_removes_the_entry() {
        let id = line("clear");
        record(&id, vec!["+60174231067".to_string()]);
        clear(&id);
        assert!(for_line(&id).is_empty());
    }

    #[test]
    fn blank_line_ids_are_rejected_rather_than_stored_under_an_empty_key() {
        record("   ", vec!["+60174231067".to_string()]);
        assert!(for_line("").is_empty());
        assert!(for_line("   ").is_empty());
    }

    #[test]
    fn lookup_tolerates_untrimmed_ids() {
        let id = line("untrimmed");
        record(&format!("  {id}  "), vec!["+60174231067".to_string()]);
        assert_eq!(for_line(&id), vec!["+60174231067".to_string()]);
        clear(&id);
    }
}
