use super::*;

#[derive(Default)]
struct FakeState {
    created: bool,
    name: String,
    calls: Vec<String>,
    reporting: [u8; 3],
    replacement_sim: bool,
    active_bearer: bool,
}
#[derive(Clone)]
struct Fake(Arc<Mutex<FakeState>>);
fn properties(id: i32, family: u32, apn: &str) -> Properties {
    HashMap::from([
        ("profile-id".into(), OwnedValue::from(id)),
        ("ip-type".into(), OwnedValue::from(family)),
        (
            "apn".into(),
            OwnedValue::try_from(Value::from(apn)).unwrap(),
        ),
        ("allowed-auth".into(), OwnedValue::from(1_u32)),
    ])
}
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem.Modem3gpp.ProfileManager")]
impl Fake {
    #[zbus(property)]
    fn index_field(&self) -> &str {
        "profile-id"
    }
    fn list(&self) -> Vec<Properties> {
        let s = self.0.lock().unwrap();
        let mut list = vec![
            properties(1, 4, ""),
            properties(2, 4, ""),
            properties(3, 4, "ims"),
        ];
        if s.created {
            let mut owned = properties(9, 1, "ims");
            owned.insert(
                "profile-name".into(),
                OwnedValue::try_from(Value::from(s.name.as_str())).unwrap(),
            );
            list.push(owned);
        }
        list
    }
    fn set(&self, requested: Properties) -> zbus::fdo::Result<Properties> {
        assert!(!requested.contains_key("profile-id"));
        assert_eq!(u32::try_from(&requested["ip-type"]).unwrap(), 1);
        assert_eq!(<&str>::try_from(&requested["apn"]).unwrap(), "ims");
        let mut s = self.0.lock().unwrap();
        assert!(!s.created);
        s.name = <&str>::try_from(&requested["profile-name"])
            .unwrap()
            .to_string();
        assert_eq!(s.name.len(), 16);
        assert!(s.name.starts_with("sa"));
        s.created = true;
        s.calls.push("Set-new".into());
        let mut result = properties(9, 1, "ims");
        result.insert(
            "profile-name".into(),
            OwnedValue::try_from(Value::from(s.name.as_str())).unwrap(),
        );
        Ok(result)
    }
    fn delete(&self, requested: Properties) -> zbus::fdo::Result<()> {
        assert_eq!(requested.len(), 1);
        assert_eq!(i32::try_from(&requested["profile-id"]).unwrap(), 9);
        let mut s = self.0.lock().unwrap();
        assert!(s.created);
        s.created = false;
        s.calls.push("Delete-owned".into());
        Ok(())
    }
}
struct FakeModem(Fake);
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem")]
impl FakeModem {
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
        if self.0 .0.lock().unwrap().active_bearer {
            vec![OwnedObjectPath::try_from("/org/freedesktop/ModemManager1/Bearer/77").unwrap()]
        } else {
            vec![]
        }
    }
    fn command(&self, command: &str, _timeout: u32) -> zbus::fdo::Result<String> {
        let mut s = self.0 .0.lock().unwrap();
        match command {
            "AT+CGDCONT=?" => Ok("+CGDCONT: (1-3,9-16),\"IP\",,,(0),(0)".into()),
            "AT+CGACT?" => Ok("+CGACT: 1,0\n+CGACT: 2,0\n+CGACT: 3,0".into()),
            "AT+CGDCONT=9,\"IP\",\"ims\"" => {
                assert!(!s.created);
                s.created = true;
                s.name = String::new();
                s.calls.push("AT-define-unused-9".into());
                Ok("OK".into())
            }
            "AT+CGDCONT?" => {
                let mut text="+CGDCONT: 1,\"IPV4V6\",\"\"\n+CGDCONT: 2,\"IPV4V6\",\"\"\n+CGDCONT: 3,\"IPV4V6\",\"ims\"".to_string();
                if s.created {
                    text.push_str("\n+CGDCONT: 9,\"IP\",\"ims\"");
                }
                Ok(text)
            }
            "AT$QCPDPIMSCFGE?" => Ok((1..=16)
                .map(|id| {
                    let f = if id == 9 { s.reporting } else { [0, 0, 0] };
                    format!("$QCPDPIMSCFGE: {id},{},{},{}", f[0], f[1], f[2])
                })
                .collect::<Vec<_>>()
                .join("\n")),
            "AT$QCPDPIMSCFGE=9,0,0,0" => {
                s.reporting = [0, 0, 0];
                s.calls.push("restore-reporting".into());
                Ok("OK".into())
            }
            _ => Err(zbus::fdo::Error::Failed("unexpected command".into())),
        }
    }
}
struct FakeSim(Fake);
#[zbus::interface(name = "org.freedesktop.ModemManager1.Sim")]
impl FakeSim {
    #[zbus(property)]
    fn sim_identifier(&self) -> &str {
        if self.0 .0.lock().unwrap().replacement_sim {
            "8900000000000000002"
        } else {
            "8900000000000000001"
        }
    }
}
struct FakeGpp;
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem.Modem3gpp")]
impl FakeGpp {
    #[zbus(property)]
    fn initial_eps_bearer_settings(&self) -> Properties {
        properties(1, 4, "internet")
    }
}
struct FakeVoice;
#[zbus::interface(name = "org.freedesktop.ModemManager1.Modem.Voice")]
impl FakeVoice {
    fn list_calls(&self) -> Vec<OwnedObjectPath> {
        vec![]
    }
}

async fn server() -> (Connection, Fake, MmProfileIo) {
    let session = std::env::var("DBUS_SESSION_BUS_ADDRESS").expect("private D-Bus only");
    assert_eq!(std::env::var("DBUS_SYSTEM_BUS_ADDRESS").unwrap(), session);
    let fake = Fake(Arc::default());
    let path = "/org/freedesktop/ModemManager1/Modem/0";
    let conn = zbus::connection::Builder::system()
        .unwrap()
        .name(SERVICE)
        .unwrap()
        .serve_at(path, fake.clone())
        .unwrap()
        .serve_at(path, FakeModem(fake.clone()))
        .unwrap()
        .serve_at(path, FakeGpp)
        .unwrap()
        .serve_at(path, FakeVoice)
        .unwrap()
        .serve_at(
            "/org/freedesktop/ModemManager1/SIM/0",
            FakeSim(fake.clone()),
        )
        .unwrap()
        .build()
        .await
        .unwrap();
    let bus = MmBus::new("/dev/wwan0qmi0", path, "wwan0").await.unwrap();
    bus.pin_sim_binding().await.unwrap();
    (
        conn,
        fake,
        MmProfileIo {
            bus,
            method: CreationMethod::Qmi,
        },
    )
}

#[derive(Default)]
struct MemoryStore(Mutex<Option<Receipt>>);
impl Store for MemoryStore {
    fn save(&self, r: &Receipt) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(r.clone());
        Ok(())
    }
    fn remove(&self) -> Result<(), String> {
        self.0.lock().unwrap().take();
        Ok(())
    }
}

#[tokio::test]
async fn temporary_profile_private_bus_uses_indexless_set_and_exact_owned_delete() {
    let (_server, fake, io) = server().await;
    let store = MemoryStore::default();
    let before = io.snapshot().await.unwrap();
    let token = fingerprint(&("ims", 1_u32, &before)).unwrap();
    let receipt = acquire_with(&io, &store, "ims", 1, &token).await.unwrap();
    assert_eq!(receipt.owned.as_ref().unwrap().id, 9);
    fake.0.lock().unwrap().reporting = [1, 1, 1];
    release_with(&io, &store, receipt).await.unwrap();
    assert_eq!(io.snapshot().await.unwrap(), before);
    assert_eq!(
        *fake.0.lock().unwrap().calls,
        ["Set-new", "restore-reporting", "Delete-owned"]
    );
    assert!(store.0.lock().unwrap().is_none());
}

#[tokio::test]
async fn temporary_profile_private_bus_at_path_creates_only_unused_capability_selected_cid() {
    let (_server, fake, mut io) = server().await;
    io.method = CreationMethod::At;
    let store = MemoryStore::default();
    let before = io.snapshot().await.unwrap();
    let token = fingerprint(&("ims", 1_u32, &before)).unwrap();
    let receipt = acquire_with(&io, &store, "ims", 1, &token).await.unwrap();
    assert_eq!(receipt.method, CreationMethod::At);
    assert_eq!(receipt.owned.as_ref().unwrap().id, 9);
    release_with(&io, &store, receipt).await.unwrap();
    assert_eq!(io.snapshot().await.unwrap(), before);
    assert_eq!(
        fake.0.lock().unwrap().calls,
        ["AT-define-unused-9", "Delete-owned"]
    );
}

#[tokio::test]
async fn temporary_profile_private_bus_rejects_sim_change_and_active_bearers() {
    let (_server, fake, io) = server().await;
    fake.0.lock().unwrap().active_bearer = true;
    assert!(io.snapshot().await.unwrap_err().contains("bearers_present"));
    fake.0.lock().unwrap().active_bearer = false;
    let store = MemoryStore::default();
    let before = io.snapshot().await.unwrap();
    let receipt = acquire_with(
        &io,
        &store,
        "ims",
        1,
        &fingerprint(&("ims", 1_u32, &before)).unwrap(),
    )
    .await
    .unwrap();
    fake.0.lock().unwrap().replacement_sim = true;
    assert!(release_with(&io, &store, receipt).await.is_err());
    assert_eq!(fake.0.lock().unwrap().calls, ["Set-new"]);
    assert!(store.0.lock().unwrap().is_some());
}
