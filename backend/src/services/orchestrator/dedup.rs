//! Cross-transport SMS dedup fingerprinting.
//!
//! A single inbound message can surface on more than one leg (e.g. an IMS leg
//! and the CS listener both see it during a handover window). To store it
//! exactly once we compute a **stable content fingerprint** and claim it in the
//! DB before inserting (see `Database::claim_sms_dedup`). The claim is race-free
//! (`INSERT OR IGNORE` on a UNIQUE column), so concurrent legs cannot both win.
//!
//! The fingerprint deliberately mirrors the fields
//! `MtSmsDeliver::is_duplicate_delivery` compares (originator, text, SCTS, and
//! the concatenation triplet), so single-leg dedup and cross-leg dedup agree.
//! We hash the text rather than embedding it, so the fingerprint never carries
//! plaintext message content.

/// Inputs to a message fingerprint. Field names match the real
/// `MtSmsDeliver` fields (note the `segment_` prefix — not bare
/// `sequence`/`total`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageFingerprintInput<'a> {
    /// SMS-DELIVER service centre timestamp (SCTS).
    pub service_center_timestamp: &'a str,
    /// Originating address.
    pub originator: &'a str,
    /// Message text (hashed, never embedded verbatim).
    pub text: &'a str,
    /// Concatenation reference (`None` for single-part).
    pub segment_reference: Option<u16>,
    /// 1-based segment sequence (`1` for single-part).
    pub segment_sequence: u8,
    /// Total segment count (`1` for single-part).
    pub segment_total: u8,
}

/// Compute a stable, plaintext-free fingerprint string for an inbound message.
///
/// The format is versioned (`smsfp1:`) so the scheme can evolve without
/// colliding with historical rows. Two deliveries that
/// `MtSmsDeliver::is_duplicate_delivery` would consider equal produce the same
/// fingerprint here.
pub fn message_fingerprint(input: &MessageFingerprintInput<'_>) -> String {
    let text_hash = format!("{:x}", md5::compute(input.text.as_bytes()));
    let seg_ref = match input.segment_reference {
        Some(r) => format!("{r:04x}"),
        None => "none".to_string(),
    };
    format!(
        "smsfp1:{}:{}:{}:{}:{}:{}",
        input.originator,
        input.service_center_timestamp,
        text_hash,
        seg_ref,
        input.segment_sequence,
        input.segment_total,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base<'a>() -> MessageFingerprintInput<'a> {
        MessageFingerprintInput {
            service_center_timestamp: "2026-07-14 12:00:00",
            originator: "+8613800138000",
            text: "hello world",
            segment_reference: None,
            segment_sequence: 1,
            segment_total: 1,
        }
    }

    #[test]
    fn identical_messages_share_a_fingerprint() {
        assert_eq!(message_fingerprint(&base()), message_fingerprint(&base()));
    }

    #[test]
    fn fingerprint_carries_no_plaintext() {
        let fp = message_fingerprint(&base());
        assert!(
            !fp.contains("hello world"),
            "fingerprint must not embed plaintext: {fp}"
        );
    }

    #[test]
    fn different_text_changes_fingerprint() {
        let a = message_fingerprint(&base());
        let mut other = base();
        other.text = "different";
        assert_ne!(a, message_fingerprint(&other));
    }

    #[test]
    fn different_originator_changes_fingerprint() {
        let a = message_fingerprint(&base());
        let mut other = base();
        other.originator = "+8613900139000";
        assert_ne!(a, message_fingerprint(&other));
    }

    #[test]
    fn different_scts_changes_fingerprint() {
        let a = message_fingerprint(&base());
        let mut other = base();
        other.service_center_timestamp = "2026-07-14 12:00:01";
        assert_ne!(a, message_fingerprint(&other));
    }

    #[test]
    fn segments_of_same_message_have_distinct_fingerprints() {
        let mut seg1 = base();
        seg1.segment_reference = Some(0x2a);
        seg1.segment_sequence = 1;
        seg1.segment_total = 2;
        let mut seg2 = seg1.clone();
        seg2.segment_sequence = 2;
        assert_ne!(message_fingerprint(&seg1), message_fingerprint(&seg2));
    }

    #[test]
    fn single_and_multipart_do_not_collide() {
        let single = base();
        let mut multi = base();
        multi.segment_reference = Some(0);
        assert_ne!(message_fingerprint(&single), message_fingerprint(&multi));
    }
}
