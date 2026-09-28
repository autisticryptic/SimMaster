//! Executed only under the isolated Actions dbus-run-session filter.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use zbus::zvariant::OwnedObjectPath;

const MODEM: &str = "/org/freedesktop/ModemManager1/Modem/99";
const SIM: &str = "/org/freedesktop/ModemManager1/SIM/99";
const CARD: &str = "8900000000000000001";

struct State {
    registration: AtomicU32,
    radio: AtomicI32,
    slot: AtomicU32,
    wrong_port: AtomicBool,
    changed_sim: AtomicBool,
    change_during_registration: AtomicBool,
    missing_sim: AtomicBool,
    unavailable: AtomicBool,
    created: AtomicU32,
    started: AtomicU32,
    accepted: AtomicU32,
    deleted: AtomicU32,
    busy: AtomicBool,
    pause_create: AtomicBool,
    create_entered: tokio::sync::Notify,
    create_release: tokio::sync::Notify,
}
impl Default for State {
    fn default() -> Self {
        Self {
            registration: AtomicU32::new(1),
            radio: AtomicI32::new(8),
            slot: AtomicU32::new(1),
            wrong_port: AtomicBool::new(false),
            changed_sim: AtomicBool::new(false),
            change_during_registration: AtomicBool::new(false),
            missing_sim: AtomicBool::new(false),
            unavailable: AtomicBool::new(false),
            created: AtomicU32::new(0),
            started: AtomicU32::new(0),
            accepted: AtomicU32::new(0),
            deleted: AtomicU32::new(0),
            busy: AtomicBool::new(false),
            pause_create: AtomicBool::new(false),
            create_entered: tokio::sync::Notify::new(),
            create_release: tokio::sync::Notify::new(),
        }
    }
}
struct FakeModem(Arc<State>);
struct FakeSim(Arc<State>);
struct FakeGpp(Arc<State>);
struct FakeVoice(Arc<State>);
struct FakeCall(Arc<State>);
const CALL: &str = "/org/freedesktop/ModemManager1/Call/99";

#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem.Voice")]
impl FakeVoice {
    fn list_calls(&self) -> Vec<OwnedObjectPath> {
        if self.0.busy.load(Ordering::SeqCst) || self.0.created.load(Ordering::SeqCst) > 0 {
            vec![OwnedObjectPath::try_from(CALL).unwrap()]
        } else {
            vec![]
        }
    }
    async fn create_call(&self, _properties: Properties) -> OwnedObjectPath {
        self.0.created.fetch_add(1, Ordering::SeqCst);
        if self.0.pause_create.load(Ordering::SeqCst) {
            self.0.create_entered.notify_one();
            self.0.create_release.notified().await;
        }
        OwnedObjectPath::try_from(CALL).unwrap()
    }
    fn delete_call(&self, _path: OwnedObjectPath) {
        self.0.deleted.fetch_add(1, Ordering::SeqCst);
    }
}
#[zbus::interface(name = "org.freedesktop.ModemManager1.Call")]
impl FakeCall {
    #[zbus(property)]
    fn state(&self) -> i32 {
        if self.0.busy.load(Ordering::SeqCst) {
            4
        } else {
            7
        }
    }
    fn start(&self) {
        self.0.started.fetch_add(1, Ordering::SeqCst);
    }
    fn accept(&self) {
        self.0.accepted.fetch_add(1, Ordering::SeqCst);
    }
}
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem")]
impl FakeModem {
    #[zbus(property)]
    fn sim(&self) -> OwnedObjectPath {
        OwnedObjectPath::try_from(if self.0.missing_sim.load(Ordering::SeqCst) {
            "/"
        } else {
            SIM
        })
        .unwrap()
    }
    #[zbus(property)]
    fn primary_port(&self) -> &str {
        if self.0.wrong_port.load(Ordering::SeqCst) {
            "wrong0"
        } else {
            "wwan0qmi0"
        }
    }
    #[zbus(property)]
    fn primary_sim_slot(&self) -> u32 {
        self.0.slot.load(Ordering::SeqCst)
    }
    #[zbus(property)]
    fn state(&self) -> i32 {
        self.0.radio.load(Ordering::SeqCst)
    }
}
#[zbus::interface(name = "org.freedesktop.ModemManager1.Sim")]
impl FakeSim {
    #[zbus(property)]
    fn sim_identifier(&self) -> &str {
        if self.0.changed_sim.load(Ordering::SeqCst) {
            "8900000000000000002"
        } else {
            CARD
        }
    }
}
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem.Modem3gpp")]
impl FakeGpp {
    #[zbus(property)]
    fn registration_state(&self) -> zbus::fdo::Result<u32> {
        if self.0.unavailable.load(Ordering::SeqCst) {
            return Err(zbus::fdo::Error::Failed("fixture_unavailable".into()));
        }
        if self.0.change_during_registration.load(Ordering::SeqCst) {
            self.0.changed_sim.store(true, Ordering::SeqCst);
        }
        Ok(self.0.registration.load(Ordering::SeqCst))
    }
}
fn binding() -> ModemBinding {
    ModemBinding {
        line_id: "line-mm-home-test".into(),
        modem_path: MODEM.into(),
        sim_path: Some(SIM.into()),
        sim_iccid: CARD.into(),
        primary_port: "wwan0qmi0".into(),
        uim_slot: 1,
        present: true,
        ..Default::default()
    }
}
async fn fixture() -> (Connection, ModemManagerObservations, Arc<State>) {
    let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("isolated bus required");
    assert_eq!(
        std::env::var("DBUS_SYSTEM_BUS_ADDRESS").ok().as_deref(),
        Some(address.as_str())
    );
    let state = Arc::new(State::default());
    let server = zbus::connection::Builder::system()
        .unwrap()
        .name(MM_SERVICE)
        .unwrap()
        .serve_at(MODEM, FakeModem(Arc::clone(&state)))
        .unwrap()
        .serve_at(MODEM, FakeGpp(Arc::clone(&state)))
        .unwrap()
        .serve_at(SIM, FakeSim(Arc::clone(&state)))
        .unwrap()
        .serve_at(MODEM, FakeVoice(Arc::clone(&state)))
        .unwrap()
        .serve_at(CALL, FakeCall(Arc::clone(&state)))
        .unwrap()
        .build()
        .await
        .unwrap();
    let client = ModemManagerObservations::new(Arc::new(Connection::system().await.unwrap()));
    (server, client, state)
}
#[tokio::test]
async fn checked_mm_dial_does_not_reuse_an_existing_call_or_dispatch_after_serial_wait() {
    let (server, observer, state) = fixture().await;
    state.busy.store(true, Ordering::SeqCst);
    assert!(modem_manager::make_call_on_modem_checked(
        &observer.connection,
        MODEM,
        "+12025550100",
        || std::future::ready(Ok(()))
    )
    .await
    .is_err());
    assert_eq!(state.created.load(Ordering::SeqCst), 0);
    state.busy.store(false, Ordering::SeqCst);
    let serial = super::super::serial::acquire_for(MODEM).await;
    let allowed = Arc::new(AtomicBool::new(true));
    let gate = Arc::clone(&allowed);
    let client = Arc::clone(&observer.connection);
    let queued = tokio::spawn(async move {
        modem_manager::make_call_on_modem_checked(&client, MODEM, "+12025550100", || {
            std::future::ready(if gate.load(Ordering::SeqCst) {
                Ok(())
            } else {
                Err("voice_registered_home_required".into())
            })
        })
        .await
    });
    tokio::task::yield_now().await;
    allowed.store(false, Ordering::SeqCst);
    drop(serial);
    assert!(queued.await.unwrap().is_err());
    assert_eq!(state.created.load(Ordering::SeqCst), 0);
    server.release_name(MM_SERVICE).await.unwrap();
}

#[tokio::test]
async fn checked_mm_create_owner_replacement_never_redirects_start_accept_or_delete() {
    let (server, observer, state) = fixture().await;
    state.pause_create.store(true, Ordering::SeqCst);
    let client = Arc::clone(&observer.connection);
    let task = tokio::spawn(async move {
        modem_manager::make_call_on_modem_checked(&client, MODEM, "+12025550100", || {
            std::future::ready(Ok(()))
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), state.create_entered.notified())
        .await
        .unwrap();
    server.release_name(MM_SERVICE).await.unwrap();
    let replacement_state = Arc::new(State::default());
    let replacement = zbus::connection::Builder::system()
        .unwrap()
        .name(MM_SERVICE)
        .unwrap()
        .serve_at(MODEM, FakeVoice(Arc::clone(&replacement_state)))
        .unwrap()
        .serve_at(CALL, FakeCall(Arc::clone(&replacement_state)))
        .unwrap()
        .build()
        .await
        .unwrap();
    state.create_release.notify_one();
    assert!(task.await.unwrap().is_err());
    assert_eq!(state.started.load(Ordering::SeqCst), 0);
    assert_eq!(
        state.deleted.load(Ordering::SeqCst),
        1,
        "cleanup stayed with original unique owner"
    );
    assert_eq!(replacement_state.started.load(Ordering::SeqCst), 0);
    assert_eq!(replacement_state.accepted.load(Ordering::SeqCst), 0);
    assert_eq!(replacement_state.deleted.load(Ordering::SeqCst), 0);
    replacement.release_name(MM_SERVICE).await.unwrap();
}

#[tokio::test]
async fn checked_mm_accept_refuses_changed_policy_and_unowned_at_index() {
    let (server, observer, state) = fixture().await;
    state.created.store(1, Ordering::SeqCst);
    assert!(
        modem_manager::answer_call_on_modem_checked(&observer.connection, MODEM, CALL, || {
            std::future::ready(Err("voice_registered_home_required".into()))
        })
        .await
        .is_err()
    );
    assert_eq!(state.accepted.load(Ordering::SeqCst), 0);
    server.release_name(MM_SERVICE).await.unwrap();
}

#[tokio::test]
async fn home_voice_observation_refuses_roaming_unknown_and_sms_only_on_private_bus() {
    let (server, observer, state) = fixture().await;
    for registration in [0, 1, 2, 5, 6, 7, 8, 9, 10] {
        state.registration.store(registration, Ordering::SeqCst);
        assert_eq!(
            observer.registered_home_voice(&binding()).await.unwrap(),
            matches!(registration, 1 | 9)
        );
    }
    state.registration.store(1, Ordering::SeqCst);
    state.unavailable.store(true, Ordering::SeqCst);
    assert!(!observer
        .registered_home_voice(&binding())
        .await
        .unwrap_or(false));
    server.release_name(MM_SERVICE).await.unwrap();
}
#[tokio::test]
async fn home_voice_observation_binds_sim_port_slot_and_radio_before_publication() {
    let (server, observer, state) = fixture().await;
    assert!(observer.registered_home_voice(&binding()).await.unwrap());
    for slot in [0, 1, 2, 256] {
        state.slot.store(slot, Ordering::SeqCst);
        assert_eq!(
            observer.registered_home_voice(&binding()).await.unwrap(),
            slot <= 1
        );
    }
    state.slot.store(1, Ordering::SeqCst);
    for flag in [
        &state.wrong_port,
        &state.changed_sim,
        &state.missing_sim,
        &state.change_during_registration,
    ] {
        flag.store(true, Ordering::SeqCst);
        assert!(!observer.registered_home_voice(&binding()).await.unwrap());
        flag.store(false, Ordering::SeqCst);
        state.changed_sim.store(false, Ordering::SeqCst);
    }
    state.radio.store(7, Ordering::SeqCst);
    assert!(!observer.registered_home_voice(&binding()).await.unwrap());
    server.release_name(MM_SERVICE).await.unwrap();
}
#[tokio::test]
async fn home_voice_observation_does_not_borrow_a_different_owner_or_missing_identity() {
    let (server, observer, _) = fixture().await;
    let mut unknown = binding();
    unknown.sim_iccid.clear();
    assert!(!observer.registered_home_voice(&unknown).await.unwrap());
    let manager = zbus::fdo::DBusProxy::new(&observer.connection)
        .await
        .unwrap();
    let old_owner = manager
        .get_name_owner(MM_SERVICE.try_into().unwrap())
        .await
        .unwrap();
    server.release_name(MM_SERVICE).await.unwrap();
    assert!(observer.registered_home_voice(&binding()).await.is_err());
    // An unexported owner cannot supply default-false roaming as home evidence.
    let replacement = zbus::connection::Builder::system()
        .unwrap()
        .name(MM_SERVICE)
        .unwrap()
        .build()
        .await
        .unwrap();
    let new_owner = manager
        .get_name_owner(MM_SERVICE.try_into().unwrap())
        .await
        .unwrap();
    assert_ne!(old_owner, new_owner);
    assert!(observer.registered_home_voice(&binding()).await.is_err());
    replacement.release_name(MM_SERVICE).await.unwrap();
}
