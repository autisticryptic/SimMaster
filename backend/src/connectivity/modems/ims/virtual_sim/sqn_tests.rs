//! Pure SQN rules; no card or database. Actions-only.
use super::*;

#[test]
fn bytes_round_trip_in_network_order() {
    let sqn = Sqn::from_bytes(&[0xff, 0x9b, 0xb4, 0xd0, 0xb6, 0x07]);
    assert_eq!(sqn.to_bytes(), [0xff, 0x9b, 0xb4, 0xd0, 0xb6, 0x07]);
    assert_eq!(sqn.value(), 0xff9bb4d0b607);
    assert_eq!(Sqn::from_value(0).to_bytes(), [0u8; 6]);
}

#[test]
fn only_a_strictly_newer_network_sqn_is_accepted() {
    let stored = Sqn::from_value(100);
    assert!(stored.accepts(Sqn::from_value(101)));
    assert!(!stored.accepts(Sqn::from_value(100)), "replay must be rejected");
    assert!(!stored.accepts(Sqn::from_value(99)));
    let mut stored = stored;
    stored.advance(Sqn::from_value(99));
    assert_eq!(stored.value(), 100, "advance never moves backwards");
    stored.advance(Sqn::from_value(105));
    assert_eq!(stored.value(), 105);
}

#[test]
fn auts_uses_the_next_value_and_wraps_inside_48_bits() {
    assert_eq!(Sqn::from_value(41).next().value(), 42);
    let max = Sqn::from_value(MASK48);
    assert_eq!(max.next().value(), 0);
    assert_eq!(Sqn::from_value(u64::MAX).value(), MASK48, "value is masked");
}

#[test]
fn ordering_follows_numeric_value() {
    let mut values = [Sqn::from_value(5), Sqn::from_value(1), Sqn::from_value(3)];
    values.sort();
    assert_eq!(values.map(|v| v.value()), [1, 3, 5]);
}
