use super::*;

fn snapshot(sim_path: Option<&str>) -> ManagedObjects {
    let mut props = InterfaceProperties::new();
    props.insert("Device".into(), OwnedValue::try_from(Value::from("/sys/devices/fixture-modem")).unwrap());
    props.insert("Model".into(), OwnedValue::try_from(Value::from("passive modem")).unwrap());
    if let Some(sim_path) = sim_path {
        let path = OwnedObjectPath::try_from(sim_path).unwrap();
        props.insert("Sim".into(), OwnedValue::try_from(Value::from(path)).unwrap());
    }
    HashMap::from([(
        OwnedObjectPath::try_from("/org/freedesktop/ModemManager1/Modem/0").unwrap(),
        HashMap::from([(MM_MODEM.to_string(), props)]),
    )])
}

#[test]
fn missing_sim_keeps_physical_hardware_and_slot_identity() {
    let inventory = passive_modems_from_objects(&snapshot(Some("/")));
    assert_eq!(inventory.len(), 1);
    assert!(inventory[0].present);
    assert_eq!(inventory[0].sim_missing, Some(true));
    assert_eq!(inventory[0].line_id,
        super::super::bindings::physical_line_id("sysfs:devices/fixture-modem", 1));
    assert!(inventory[0].slot_stable);
}

#[test]
fn cached_sim_is_not_claimed_missing_or_actively_verified() {
    // Even if the snapshot has no corresponding SIM interface, never infer
    // removal from incomplete MM cache/identity. We make no SIM GetAll call.
    let cached = passive_modems_from_objects(&snapshot(Some("/org/freedesktop/ModemManager1/SIM/0")));
    assert_eq!(cached[0].sim_missing, Some(false));
    assert_eq!(cached[0].observation_source, "modemmanager_cache");
    let unknown = passive_modems_from_objects(&snapshot(None));
    assert_eq!(unknown[0].sim_missing, None);
    let absent = passive_modems_from_objects(&snapshot(Some("/")));
    assert_eq!(cached[0].line_id, absent[0].line_id);
    assert_eq!(unknown[0].line_id, absent[0].line_id);
}

#[test]
fn empty_snapshot_is_empty_not_a_synthetic_line() {
    assert!(passive_modems_from_objects(&ManagedObjects::new()).is_empty());
}
