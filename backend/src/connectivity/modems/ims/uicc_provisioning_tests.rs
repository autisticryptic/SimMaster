use super::*;
use std::{cell::Cell, collections::VecDeque, future::ready};

fn card() -> UiccCardBinding {
    UiccCardBinding { endpoint: "synthetic-provisioning".into(), slot: 1,
        owner: "owner".into(), iccid: "synthetic-card".into(), imsi: "001010000000001".into() }
}
fn snapshot() -> UiccImsProvisioning {
    UiccImsProvisioning::new(UiccApplications {
        usim_aid: Some(USIM_AID_PREFIX.to_vec()), isim_aid: Some(qmi_uim::isim::ISIM_AID_PREFIX.to_vec()),
    }, Some(IsimImsMaterial { impi: Some("private@ims.example.test".into()), domain: Some("ims.example.test".into()),
        impus: vec!["sip:public@ims.example.test".into()], pcscf: vec!["pcscf.example.test".into()] })).unwrap()
}

#[tokio::test]
async fn stable_read_is_bounded_and_bracketed_by_three_owner_observations() {
    let expected = card(); let reads = Cell::new(0); let observations = Cell::new(0);
    let value = read_bound_with(&expected, || { observations.set(observations.get()+1); ready(Ok(expected.clone())) },
        || { reads.set(reads.get()+1); ready(Ok(snapshot())) }).await.unwrap();
    assert_eq!(value, snapshot()); assert_eq!(reads.get(), 2); assert_eq!(observations.get(), 3);
}

#[tokio::test]
async fn each_same_card_material_change_is_detected_including_app_addition_or_removal() {
    let expected = card(); let old = snapshot();
    super::super::register_test_binding("provisioning-material", expected.clone());
    let mut changed = Vec::new();
    let mut value = old.clone(); value.material.as_mut().unwrap().impi = Some("other@ims.example.test".into()); changed.push(value);
    let mut value = old.clone(); value.material.as_mut().unwrap().domain = Some("other.example.test".into()); changed.push(value);
    let mut value = old.clone(); value.material.as_mut().unwrap().impus = vec!["sip:other@ims.example.test".into()]; changed.push(value);
    let mut value = old.clone(); value.material.as_mut().unwrap().pcscf = vec!["new.example.test".into()]; changed.push(value);
    let mut value = old.clone(); value.applications.isim_aid.as_mut().unwrap().push(1); changed.push(value);
    changed.push(UiccImsProvisioning::new(UiccApplications { usim_aid: Some(USIM_AID_PREFIX.to_vec()), isim_aid: None }, None).unwrap());
    for value in changed {
        seed_for_test("provisioning-material", &expected, value.clone());
        assert_eq!(old.verify_bound(&expected, "provisioning-material", "unused").await, Err("ims_uicc_provisioning_changed"));
        assert_eq!(value.verify_bound(&expected, "provisioning-material", "unused").await, Ok(()));
    }
}

#[tokio::test]
async fn torn_reads_do_not_loop_or_rebase_the_registered_snapshot() {
    let expected = card(); let old = snapshot(); let mut changed = old.clone();
    changed.material.as_mut().unwrap().domain = Some("changed.example.test".into());
    let mut values = VecDeque::from([old.clone(), changed]);
    let error = read_bound_with(&expected, || ready(Ok(expected.clone())), || ready(Ok(values.pop_front().unwrap()))).await.unwrap_err();
    assert_eq!(error, "ims_uicc_provisioning_unstable"); assert!(values.is_empty()); assert_eq!(old, snapshot());
}

#[tokio::test]
async fn a_read_error_is_not_unconfigured_and_owner_loss_wins_over_partial_data() {
    let expected = card(); let reads = Cell::new(0);
    assert_eq!(read_bound_with(&expected, || ready(Ok(expected.clone())), || {
        reads.set(reads.get()+1); ready(Err("isim_apdu_exchange_failed"))
    }).await, Err("isim_apdu_exchange_failed"));
    assert_eq!(reads.get(), 1);
    let mut replacement = expected.clone(); replacement.iccid = "new-card".into();
    let mut observations = VecDeque::from([expected.clone(), replacement]);
    assert_eq!(read_bound_with(&expected, || ready(Ok(observations.pop_front().unwrap())),
        || ready(Err("isim_apdu_exchange_failed"))).await, Err("ims_uicc_binding_changed"));
}

#[tokio::test]
async fn same_reader_and_same_efs_cannot_hide_physical_subscription_swap() {
    let expected = card(); let mut replacement = expected.clone(); replacement.owner = "new-owner".into();
    let mut observations = VecDeque::from([expected.clone(), expected.clone(), replacement]);
    assert_eq!(read_bound_with(&expected, || ready(Ok(observations.pop_front().unwrap())), || ready(Ok(snapshot()))).await,
        Err("ims_uicc_binding_changed"));
}

#[test]
fn optional_partial_material_is_valid_but_inconsistent_or_unsafe_state_is_rejected() {
    let old = snapshot();
    let partial = IsimImsMaterial { impi: None, ..old.material.clone().unwrap() };
    assert!(UiccImsProvisioning::new(old.applications.clone(), Some(partial)).is_ok());
    assert_eq!(UiccImsProvisioning::new(old.applications.clone(), None), Err("ims_uicc_provisioning_invalid"));
    assert_eq!(UiccImsProvisioning::new(UiccApplications::default(), old.material.clone()), Err("ims_uicc_provisioning_invalid"));
    let mut unsafe_value = old.material.unwrap(); unsafe_value.impi = Some("bad\r\nheader".into());
    assert!(UiccImsProvisioning::new(old.applications, Some(unsafe_value)).is_err());
}

#[test]
fn debug_never_exposes_aids_ims_identities_or_proxy_names() {
    let text = format!("{:?}", snapshot());
    for secret in ["private", "public", "ims.example", "pcscf", "160", "135"] { assert!(!text.contains(secret)); }
}
