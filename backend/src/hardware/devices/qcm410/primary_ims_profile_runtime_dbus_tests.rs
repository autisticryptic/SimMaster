//! Must be run under a private dbus-run-session with SYSTEM == SESSION.
use super::*;

#[derive(Default)]
struct State {
    created: bool,
    family: u32,
    reporting: [u8; 3],
    calls: Vec<String>,
    active: bool,
    changed_sim: bool,
    invalidate_after_definition: Option<Arc<AtomicBool>>,
}
#[derive(Clone)]
struct Profiles(Arc<Mutex<State>>);
fn profile_properties(id: i32, family: u32, apn: &str) -> Properties {
    HashMap::from([
        ("profile-id".into(), OwnedValue::from(id)),
        ("ip-type".into(), OwnedValue::from(family)),
        (
            "apn".into(),
            OwnedValue::try_from(Value::from(apn)).unwrap(),
        ),
    ])
}
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem.Modem3gpp.ProfileManager")]
impl Profiles {
    #[zbus(property)]
    fn index_field(&self) -> &str {
        "profile-id"
    }
    fn list(&self) -> Vec<Properties> {
        let state = self.0.lock().unwrap();
        let mut values = vec![profile_properties(3, 4, "ims")];
        if state.created {
            values.push(profile_properties(7, state.family, "ims"));
        }
        values
    }
    fn set(&self, _requested: Properties) -> zbus::fdo::Result<Properties> {
        panic!("runtime must never attempt QMI Set or a QMI-to-AT fallback")
    }
    fn delete(&self, requested: Properties) {
        assert_eq!(requested.len(), 1);
        assert_eq!(i32::try_from(&requested["profile-id"]).unwrap(), 7);
        let mut state = self.0.lock().unwrap();
        assert!(state.created);
        assert!(!state.active);
        state.created = false;
        state.calls.push("Delete:7".into());
    }
}
#[derive(Clone)]
struct Modem(Profiles);
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem")]
impl Modem {
    #[zbus(property)]
    fn primary_port(&self) -> &str {
        "wwan0qmi0"
    }
    #[zbus(property)]
    fn sim(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from("/org/freedesktop/ModemManager1/SIM/0").unwrap()
    }
    #[zbus(property)]
    fn primary_sim_slot(&self) -> u32 {
        1
    }
    #[zbus(property)]
    fn bearers(&self) -> Vec<OwnedObjectPath> {
        if self.0 .0.lock().unwrap().active {
            vec![OwnedObjectPath::try_from("/org/freedesktop/ModemManager1/Bearer/77").unwrap()]
        } else {
            vec![]
        }
    }
    fn command(&self, command: &str, _timeout: u32) -> zbus::fdo::Result<String> {
        let mut state = self.0 .0.lock().unwrap();
        match command {
            "AT+CGDCONT=?" => Ok("+CGDCONT: (3,7-16),\"IP\"\n+CGDCONT: (3,7-16),\"IPV6\"\n+CGDCONT: (3,7-16),\"IPV4V6\"".into()),
            "AT+CGACT?" => Ok("+CGACT: 3,0".into()),
            "AT+CGDCONT?" => {
                let mut value = "+CGDCONT: 3,\"IPV4V6\",\"ims\"".to_string();
                if state.created {
                    let family = match state.family { 1 => "IP", 2 => "IPV6", _ => "IPV4V6" };
                    value.push_str(&format!("\n+CGDCONT: 7,\"{family}\",\"ims\""));
                }
                Ok(value)
            }
            "AT$QCPDPIMSCFGE?" => Ok((1..=16).map(|id| {
                let f = if id == 7 { state.reporting } else { [0, 0, 0] };
                format!("$QCPDPIMSCFGE: {id},{},{},{}", f[0], f[1], f[2])
            }).collect::<Vec<_>>().join("\n")),
            "AT$QCPDPIMSCFGE=7,1,1,1" => {
                state.reporting = [1, 1, 1]; state.calls.push(command.into()); Ok("OK".into())
            }
            "AT$QCPDPIMSCFGE=7,0,0,0" => {
                state.reporting = [0, 0, 0]; state.calls.push(command.into()); Ok("OK".into())
            }
            _ if command.starts_with("AT+CGDCONT=7,") => {
                assert!(!state.created); assert!(!state.active);
                state.family = if command.contains("\"IPV4V6\"") { 4 } else if command.contains("\"IPV6\"") { 2 } else { 1 };
                state.created = true; state.calls.push(command.into());
                if let Some(current) = state.invalidate_after_definition.take() {
                    current.store(false, Ordering::Release);
                }
                Ok("OK".into())
            }
            _ => Err(zbus::fdo::Error::Failed("unexpected command".into())),
        }
    }
}
struct Sim(Profiles);
#[zbus::interface(name = "org.freedesktop.ModemManager1.Sim")]
impl Sim {
    #[zbus(property)]
    fn sim_identifier(&self) -> &str {
        if self.0 .0.lock().unwrap().changed_sim {
            "8900000000000000002"
        } else {
            "8900000000000000001"
        }
    }
}
struct Gpp;
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem.Modem3gpp")]
impl Gpp {
    #[zbus(property)]
    fn initial_eps_bearer_settings(&self) -> Properties {
        profile_properties(1, 4, "internet")
    }
}
struct Voice;
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem.Voice")]
impl Voice {
    fn list_calls(&self) -> Vec<OwnedObjectPath> {
        vec![]
    }
}
fn topology(_: &str) -> Result<String, String> {
    Ok("mock-control-topology".into())
}
async fn server() -> (Connection, Profiles, RuntimeIo) {
    let session = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("private D-Bus only");
    assert_eq!(std::env::var("DBUS_SYSTEM_BUS_ADDRESS").unwrap(), session);
    let profiles = Profiles(Arc::default());
    let path = format!("{MODEM_PREFIX}0");
    let connection = zbus::connection::Builder::system()
        .unwrap()
        .name(SERVICE)
        .unwrap()
        .serve_at(path.as_str(), profiles.clone())
        .unwrap()
        .serve_at(path.as_str(), Modem(profiles.clone()))
        .unwrap()
        .serve_at(path.as_str(), Gpp)
        .unwrap()
        .serve_at(path.as_str(), Voice)
        .unwrap()
        .serve_at(
            "/org/freedesktop/ModemManager1/SIM/0",
            Sim(profiles.clone()),
        )
        .unwrap()
        .serve_at("/org/freedesktop/ModemManager1", zbus::fdo::ObjectManager)
        .unwrap()
        .build()
        .await
        .unwrap();
    let bus = MmBus::new(PRIMARY, &path, "wwan0").await.unwrap();
    bus.pin_sim_binding().await.unwrap();
    (
        connection,
        profiles,
        RuntimeIo {
            inner: MmProfileIo {
                bus,
                method: CreationMethod::At,
                topology,
            },
            current: Arc::new(|| true),
            definition_dispatched: AtomicBool::new(false),
            reporting_dispatched: AtomicBool::new(false),
        },
    )
}
#[derive(Default)]
struct MemoryStore(Mutex<Option<Receipt>>);
impl Store for MemoryStore {
    fn save(&self, receipt: &Receipt) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(receipt.clone());
        Ok(())
    }
    fn remove(&self) -> Result<(), String> {
        self.0.lock().unwrap().take();
        Ok(())
    }
}

#[tokio::test]
async fn runtime_profile_private_bus_attempts_keep_owner_sim_but_reset_interface_and_guard_dispatch(
) {
    let (_server, profiles, io) = server().await;
    let current = Arc::new(AtomicBool::new(true));
    let predicate: Current = {
        let current = Arc::clone(&current);
        Arc::new(move || current.load(Ordering::Acquire))
    };
    let first = io
        .inner
        .bus
        .for_profile_attempt(Arc::clone(&predicate))
        .unwrap();
    first.selected_interface.set("wwan2".into()).unwrap();
    let second = io
        .inner
        .bus
        .for_profile_attempt(Arc::clone(&predicate))
        .unwrap();
    assert!(second.selected_interface.get().is_none());
    assert_eq!(second.data_interface(), "wwan0");
    assert_eq!(first.owner, second.owner);
    assert_eq!(first.bus_id, second.bus_id);
    assert!(first.sim_binding.get() == second.sim_binding.get());
    current.store(false, Ordering::Release);
    let request = PrimaryImsRequest {
        device: PRIMARY,
        modem: &second.modem,
        interface: "wwan0",
        apn: "ims",
        profile_id: Some(7),
        family: MmIpFamily::Ipv4,
        allow_roaming: true,
        expected_sim: Some(("8900000000000000001", 1)),
    };
    assert_eq!(
        second.create(&request).await.unwrap_err(),
        "qca410_primary_mm_setup_cancelled"
    );
    assert_eq!(
        second
            .connect("/org/freedesktop/ModemManager1/Bearer/77")
            .await
            .unwrap_err(),
        "qca410_primary_mm_setup_cancelled"
    );
    assert!(profiles.0.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn runtime_profile_private_bus_retirement_requires_unique_owner_absence_not_just_replacement()
{
    let (original_server, _profiles, io) = server().await;
    let store = MemoryStore::default();
    let before = io.snapshot().await.unwrap();
    let plan = fingerprint(&("ims", 1_u32, &before)).unwrap();
    let mut receipt = acquire_with(&io, &store, "ims", 1, &plan).await.unwrap();
    original_server.release_name(SERVICE).await.unwrap();
    let replacement = zbus::connection::Builder::system()
        .unwrap()
        .name(SERVICE)
        .unwrap()
        .build()
        .await
        .unwrap();
    let bus = MmBus::new(PRIMARY, &format!("{MODEM_PREFIX}0"), "wwan0")
        .await
        .unwrap();
    let fresh = MmProfileIo {
        bus,
        method: CreationMethod::At,
        topology,
    };
    assert_eq!(
        super::retirement::old_owner_absent(&fresh, &receipt)
            .await
            .unwrap_err(),
        "mm_ims_profile_retirement_old_owner_still_present"
    );
    // The original unique name remains alive despite losing the service name.
    assert_ne!(original_server.unique_name(), replacement.unique_name());
    receipt.before.owner = ":1.999999999".into();
    super::retirement::old_owner_absent(&fresh, &receipt)
        .await
        .unwrap();
    replacement.release_name(SERVICE).await.unwrap();
    assert!(super::retirement::old_owner_absent(&fresh, &receipt)
        .await
        .is_err());
}

#[tokio::test]
async fn runtime_profile_private_bus_exact_family_reporting_and_cleanup() {
    let (_server, profiles, io) = server().await;
    for family in [4_u32, 2, 1] {
        let store = MemoryStore::default();
        let before = io.snapshot().await.unwrap();
        let receipt = acquire_with(
            &io,
            &store,
            "ims",
            family,
            &fingerprint(&("ims", family, &before)).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(receipt.method, CreationMethod::At);
        assert_eq!(receipt.owned.as_ref().unwrap().family, family);
        arm(&io, &store, receipt).await.unwrap();
        let receipt = store.0.lock().unwrap().clone().unwrap();
        profiles.0.lock().unwrap().active = true;
        assert!(release_with(&io.inner, &store, receipt.clone())
            .await
            .is_err());
        assert!(profiles.0.lock().unwrap().created);
        profiles.0.lock().unwrap().active = false;
        release_with(&io.inner, &store, receipt).await.unwrap();
        assert_eq!(io.snapshot().await.unwrap(), before);
    }
    assert_eq!(
        profiles
            .0
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|c| *c == "Delete:7")
            .count(),
        3
    );
}

#[tokio::test]
async fn runtime_profile_private_bus_generation_rechecked_inside_serial_wait() {
    let (_server, profiles, mut io) = server().await;
    let live = Arc::new(AtomicBool::new(true));
    io.current = {
        let live = Arc::clone(&live);
        Arc::new(move || live.load(Ordering::Acquire))
    };
    let modem = io.inner.bus.modem.clone();
    let (entered, entry) = oneshot::channel();
    let (release, released) = oneshot::channel();
    let blocker = tokio::spawn(async move {
        serial::with_serial_for(&modem, async {
            entered.send(()).unwrap();
            released.await.unwrap();
        })
        .await;
    });
    entry.await.unwrap();
    let pending = tokio::spawn(async move { io.command("AT+CGDCONT=7,\"IPV4V6\",\"ims\"").await });
    tokio::task::yield_now().await;
    live.store(false, Ordering::Release);
    release.send(()).unwrap();
    blocker.await.unwrap();
    assert!(pending.await.unwrap().is_err());
    assert!(profiles.0.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn runtime_profile_private_bus_fresh_discovery_requires_old_modem_absence_and_stable_sim() {
    let (server, profiles, io) = server().await;
    let store = MemoryStore::default();
    let before = io.snapshot().await.unwrap();
    let receipt = acquire_with(
        &io,
        &store,
        "ims",
        4,
        &fingerprint(&("ims", 4_u32, &before)).unwrap(),
    )
    .await
    .unwrap();
    let next = format!("{MODEM_PREFIX}8");
    server
        .object_server()
        .at(next.as_str(), profiles.clone())
        .await
        .unwrap();
    server
        .object_server()
        .at(next.as_str(), Modem(profiles.clone()))
        .await
        .unwrap();
    server.object_server().at(next.as_str(), Gpp).await.unwrap();
    server
        .object_server()
        .at(next.as_str(), Voice)
        .await
        .unwrap();
    assert!(identity_io_with(&receipt, topology).await.is_err()); // two possible modems
    let old = receipt.before.modem.as_str();
    server
        .object_server()
        .remove::<Profiles, _>(old)
        .await
        .unwrap();
    server
        .object_server()
        .remove::<Modem, _>(old)
        .await
        .unwrap();
    server.object_server().remove::<Gpp, _>(old).await.unwrap();
    server
        .object_server()
        .remove::<Voice, _>(old)
        .await
        .unwrap();
    let rebound = identity_io_with(&receipt, topology).await.unwrap();
    assert_eq!(rebound.bus.modem, next);
    profiles.0.lock().unwrap().changed_sim = true;
    assert!(identity_io_with(&receipt, topology).await.is_err());
    profiles.0.lock().unwrap().changed_sim = false;
    let receipt = reconcile_profile_modem(&rebound, &store, receipt)
        .await
        .unwrap();
    release_with(&rebound, &store, receipt).await.unwrap();
    assert!(!profiles.0.lock().unwrap().created);
}

#[tokio::test]
async fn runtime_profile_private_bus_late_generation_loss_finishes_profile_readback() {
    let (_server, profiles, mut io) = server().await;
    let current = Arc::new(AtomicBool::new(true));
    profiles.0.lock().unwrap().invalidate_after_definition = Some(Arc::clone(&current));
    io.current = {
        let current = Arc::clone(&current);
        Arc::new(move || current.load(Ordering::Acquire))
    };
    let before = io.snapshot().await.unwrap();
    let store = MemoryStore::default();
    let receipt = acquire_with(
        &io,
        &store,
        "ims",
        4,
        &fingerprint(&("ims", 4_u32, &before)).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(receipt.phase, Phase::Owned);
    assert!(check_current(&io.current).is_err());
    assert!(io.command("AT$QCPDPIMSCFGE=7,1,1,1").await.is_err());
    release_with(&io.inner, &store, receipt).await.unwrap();
    assert_eq!(
        profiles.0.lock().unwrap().calls,
        ["AT+CGDCONT=7,\"IPV4V6\",\"ims\"", "Delete:7"]
    );
}

#[tokio::test]
async fn runtime_profile_private_bus_original_owner_is_not_redirected() {
    let (original, profiles, io) = server().await;
    original.release_name(SERVICE).await.unwrap();
    let (_replacement, replacement_profiles, _) = server().await;
    assert!(io.command("AT+CGDCONT=7,\"IPV4V6\",\"ims\"").await.is_err());
    assert!(profiles.0.lock().unwrap().calls.is_empty());
    assert!(replacement_profiles.0.lock().unwrap().calls.is_empty());
}
