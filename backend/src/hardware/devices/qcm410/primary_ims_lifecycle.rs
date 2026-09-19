//! Ownership of MM-backed IMS bearers, including process termination.
//!
//! Mutations target a D-Bus UNIQUE owner, not the reusable MM well-known name.
//! A durable /run record is written before Connect or a namespace move. Only
//! records created by this driver may be reclaimed after an application crash.

use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, OpenOptions},
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tokio::time::Instant;
use zbus::{
    fdo::DBusProxy,
    proxy::CacheProperties,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
    Connection, Proxy,
};

use crate::{
    hardware::{
        cellular::{cgcontrdp::CgcontrdpSettings, serial},
        devices::transport::{ImsBearerError, ImsBearerErrorKind, ImsPcscfDiscovery},
    },
    platform::netns::{self, NetnsName},
};

use super::{
    netdev::{self, NetdevConfig},
    primary_ims_pcscf::{self, session_changed, unavailable},
    primary_ims_session::{safe_error, PrimaryImsRequest, OWNER_MISSING},
    primary_ims_settings::{self, MmIpFamily},
};

const SERVICE: &str = "org.freedesktop.ModemManager1";
const MODEM: &str = "org.freedesktop.ModemManager1.Modem";
const BEARER: &str = "org.freedesktop.ModemManager1.Bearer";
const STATE_DIR: &str = "/run/simadmin/primary-ims-owned";
const MAX_RECORD_BYTES: u64 = 8192;

static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicUsize = AtomicUsize::new(0);
static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

type LeaseKey = (String, String, String);
type Leases = BTreeMap<LeaseKey, Arc<OwnedLease>>;
static LEASES: OnceLock<Mutex<Leases>> = OnceLock::new();

#[derive(Default)]
struct NetworkActivity {
    closing: AtomicBool,
    active: AtomicUsize,
}

pub(super) struct NetworkGuard(Arc<NetworkActivity>);

impl NetworkActivity {
    fn enter(self: &Arc<Self>) -> Result<NetworkGuard, String> {
        if self.closing.load(Ordering::Acquire) || is_shutting_down() {
            return Err("qca410_primary_mm_session_closing".to_string());
        }
        self.active.fetch_add(1, Ordering::AcqRel);
        let guard = NetworkGuard(Arc::clone(self));
        if self.closing.load(Ordering::Acquire) || is_shutting_down() {
            return Err("qca410_primary_mm_session_closing".to_string());
        }
        Ok(guard)
    }
}

impl Drop for NetworkGuard {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

fn leases() -> &'static Mutex<Leases> {
    LEASES.get_or_init(Mutex::default)
}

pub(super) fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Acquire)
}

pub(super) fn begin_shutdown() {
    SHUTTING_DOWN.store(true, Ordering::Release);
}

pub(super) struct PendingSetup;

impl PendingSetup {
    pub fn new() -> Self {
        PENDING.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for PendingSetup {
    fn drop(&mut self) {
        PENDING.fetch_sub(1, Ordering::AcqRel);
    }
}

fn bus_error(error: impl std::fmt::Display) -> String {
    let text = error.to_string();
    let lower = text.to_ascii_lowercase();
    if [
        "serviceunknown",
        "namehasnoowner",
        "unknownobject",
        "unknown object",
        "org.freedesktop.modemmanager1.error.core.notfound",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
    {
        OWNER_MISSING.to_string()
    } else {
        format!("qca410_primary_mm_dbus_failed:{}", safe_error(&text))
    }
}

async fn timed<T>(
    seconds: u64,
    future: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::time::timeout(Duration::from_secs(seconds), future)
        .await
        .map_err(|_| "qca410_primary_mm_command_timeout".to_string())?
}

pub(super) struct MmBus {
    connection: Connection,
    pub bus_id: String,
    pub owner: String,
    pub modem: String,
    pub device: String,
    pub interface: String,
}

pub(super) struct BearerStatus {
    pub connected: bool,
    pub interface: String,
    pub apn: String,
}

impl MmBus {
    pub async fn new(device: &str, modem: &str, interface: &str) -> Result<Arc<Self>, String> {
        timed(10, async {
            let connection = Connection::system().await.map_err(bus_error)?;
            let manager = DBusProxy::new(&connection).await.map_err(bus_error)?;
            let bus_id = manager.get_id().await.map_err(bus_error)?.to_string();
            let owner = manager
                .get_name_owner(SERVICE.try_into().expect("valid MM service name"))
                .await
                .map_err(bus_error)?
                .to_string();
            Ok(Arc::new(Self {
                connection,
                bus_id,
                owner,
                modem: modem.to_string(),
                device: device.to_string(),
                interface: interface.to_string(),
            }))
        })
        .await
    }

    async fn proxy<'a>(
        &'a self,
        path: &'a str,
        interface: &'static str,
    ) -> Result<Proxy<'a>, String> {
        zbus::proxy::Builder::new(&self.connection)
            .destination(self.owner.as_str())
            .map_err(bus_error)?
            .path(path)
            .map_err(bus_error)?
            .interface(interface)
            .map_err(bus_error)?
            .cache_properties(CacheProperties::No)
            .build()
            .await
            .map_err(bus_error)
    }

    pub async fn owner_is_current(&self) -> Result<bool, String> {
        timed(5, async {
            let manager = DBusProxy::new(&self.connection).await.map_err(bus_error)?;
            match manager
                .get_name_owner(SERVICE.try_into().expect("valid MM service name"))
                .await
            {
                Ok(owner) => Ok(owner.as_str() == self.owner),
                Err(error) if bus_error(&error) == OWNER_MISSING => Ok(false),
                Err(error) => Err(bus_error(error)),
            }
        })
        .await
    }

    pub async fn primary_port(&self) -> Result<String, String> {
        timed(10, async {
            self.proxy(&self.modem, MODEM)
                .await?
                .get_property::<String>("PrimaryPort")
                .await
                .map_err(bus_error)
        })
        .await
    }

    pub async fn bearers(&self) -> Result<Vec<String>, String> {
        timed(10, async {
            let paths = self
                .proxy(&self.modem, MODEM)
                .await?
                .get_property::<Vec<OwnedObjectPath>>("Bearers")
                .await
                .map_err(bus_error)?;
            Ok(paths.into_iter().map(|path| path.to_string()).collect())
        })
        .await
    }

    pub async fn create(&self, request: &PrimaryImsRequest<'_>) -> Result<String, String> {
        timed(15, async {
            let properties = create_properties(request)?;
            let path: OwnedObjectPath = self
                .proxy(&self.modem, MODEM)
                .await?
                .call("CreateBearer", &(properties,))
                .await
                .map_err(bus_error)?;
            Ok(path.to_string())
        })
        .await
    }

    pub async fn status(&self, bearer: &str) -> Result<BearerStatus, String> {
        timed(10, async {
            let proxy = self.proxy(bearer, BEARER).await?;
            let connected = proxy
                .get_property::<bool>("Connected")
                .await
                .map_err(bus_error)?;
            let interface = proxy
                .get_property::<String>("Interface")
                .await
                .map_err(bus_error)?;
            let properties = proxy
                .get_property::<HashMap<String, OwnedValue>>("Properties")
                .await
                .map_err(bus_error)?;
            let apn = properties
                .get("apn")
                .and_then(|value| <&str>::try_from(value).ok())
                .unwrap_or_default()
                .to_string();
            Ok(BearerStatus {
                connected,
                interface,
                apn,
            })
        })
        .await
    }

    /// Read only this unique-owner bearer, not a modem-wide context or another
    /// application's WDS client. GetAll keeps status and IP dictionaries in one
    /// response; owner/status are checked again before consuming the result.
    pub async fn ip_settings(
        &self,
        bearer: &str,
        apn: &str,
        family: MmIpFamily,
    ) -> Result<Option<CgcontrdpSettings>, String> {
        timed(10, async {
            if !self.owner_is_current().await? {
                return Err(OWNER_MISSING.to_string());
            }
            let properties: primary_ims_settings::Properties = self
                .proxy(bearer, "org.freedesktop.DBus.Properties")
                .await?
                .call("GetAll", &(BEARER,))
                .await
                .map_err(bus_error)?;
            let settings = primary_ims_settings::parse(&properties, &self.interface, apn, family)?;
            let status = self.status(bearer).await?;
            primary_ims_settings::validate_binding(
                status.connected,
                &status.interface,
                &status.apn,
                &self.interface,
                apn,
            )?;
            if !self.owner_is_current().await? {
                return Err(OWNER_MISSING.to_string());
            }
            Ok(settings)
        })
        .await
    }

    /// Supplementary AT observations are bound to the session's original
    /// unique owner, never to a later lookup of a reusable modem selector.
    pub async fn discover_pcscf(
        self: &Arc<Self>,
        bearer: &str,
        apn: &str,
        family: MmIpFamily,
        profile_id: Option<u32>,
        expected: &CgcontrdpSettings,
        guard: &NetworkGuard,
    ) -> Result<ImsPcscfDiscovery, ImsBearerError> {
        let deadline = Instant::now() + primary_ims_pcscf::BUDGET;
        tokio::time::timeout_at(deadline, async {
            let result = primary_ims_pcscf::discover_with(
                expected,
                profile_id,
                apn,
                primary_ims_pcscf::READ_DELAY,
                || self.pcscf_binding_snapshot(bearer, apn, family, profile_id),
                |command| async move { self.pcscf_at_read(&command, guard, deadline).await },
            )
            .await;
            // Every recoverable exit (including AT/parse/context errors)
            // must prove the bearer is still the same before DNS/config
            // fallback can use its old addressing. Cached liveness alone
            // cannot detect profile, grant or exclusive-interface changes.
            if result
                .as_ref()
                .is_err_and(|error| error.kind == ImsBearerErrorKind::PcscfUnavailable)
                && self
                    .pcscf_binding_snapshot(bearer, apn, family, profile_id)
                    .await?
                    != *expected
            {
                return Err(session_changed("ip_config_changed"));
            }
            result
        })
        .await
        .map_err(|_| session_changed("read_timeout_unverified"))?
    }

    async fn pcscf_owner_check(&self) -> Result<(), ImsBearerError> {
        if !self
            .owner_is_current()
            .await
            .map_err(|_| session_changed("owner_unavailable"))?
        {
            return Err(session_changed("owner_changed"));
        }
        let port = self
            .primary_port()
            .await
            .map_err(|_| session_changed("endpoint_unavailable"))?;
        if self.device.strip_prefix("/dev/") != Some(port.as_str()) {
            return Err(session_changed("endpoint_changed"));
        }
        Ok(())
    }

    async fn pcscf_binding_snapshot(
        &self,
        bearer: &str,
        apn: &str,
        family: MmIpFamily,
        profile_id: Option<u32>,
    ) -> Result<CgcontrdpSettings, ImsBearerError> {
        self.pcscf_owner_check().await?;
        let result = timed(10, async {
            if !self.bearers().await?.iter().any(|path| path == bearer)
                || !self.may_clean_interface(bearer).await?
            {
                return Err("bearer_not_exclusive".to_string());
            }
            let properties: primary_ims_settings::Properties = self
                .proxy(bearer, "org.freedesktop.DBus.Properties")
                .await?
                .call("GetAll", &(BEARER,))
                .await
                .map_err(bus_error)?;
            let actual_profile = primary_ims_settings::profile_id(&properties)?;
            if profile_id.is_some() && actual_profile != profile_id {
                return Err("profile_pin_changed".to_string());
            }
            let settings = primary_ims_settings::parse(&properties, &self.interface, apn, family)?
                .ok_or_else(|| "ip_config_unavailable".to_string())?;
            let status = self.status(bearer).await?;
            primary_ims_settings::validate_binding(
                status.connected,
                &status.interface,
                &status.apn,
                &self.interface,
                apn,
            )?;
            Ok(settings)
        })
        .await
        // Do not export raw properties or a possibly sensitive D-Bus error.
        .map_err(|_| session_changed("bearer_snapshot_invalid"))?;
        self.pcscf_owner_check().await?;
        Ok(result)
    }

    async fn pcscf_at_read(
        self: &Arc<Self>,
        command: &str,
        guard: &NetworkGuard,
        deadline: Instant,
    ) -> Result<String, ImsBearerError> {
        let context_read = command.strip_prefix("AT+CGCONTRDP=").is_some_and(|value| {
            value
                .parse::<u8>()
                .ok()
                .is_some_and(|cid| (1..=16).contains(&cid) && cid.to_string() == value)
        });
        if !matches!(command, "AT+CGACT?" | "AT+CGDCONT?") && !context_read {
            return Err(unavailable("command_not_read_only"));
        }
        let guard = guard
            .0
            .enter()
            .map_err(|_| session_changed("lease_closing"))?;
        let bus = Arc::clone(self);
        let command = command.to_string();
        retained_serial_read(self.modem.clone(), guard, deadline, async move {
            bus.pcscf_owner_check().await?;
            if Instant::now() >= deadline {
                return Err(session_changed("read_timeout_unverified"));
            }
            let result = timed(5, async {
                bus.proxy(&bus.modem, MODEM)
                    .await?
                    .call::<_, _, String>("Command", &(command.as_str(), 4_u32))
                    .await
                    .map_err(bus_error)
            })
            .await;
            // Even a failed AT reply must not conceal handover during the call.
            bus.pcscf_owner_check().await?;
            result.map_err(|error| {
                if error == "qca410_primary_mm_command_timeout" {
                    session_changed("at_read_timeout_unverified")
                } else {
                    unavailable("at_read_failed")
                }
            })
        })
        .await
    }

    pub async fn connect(&self, bearer: &str) -> Result<(), String> {
        timed(65, async {
            self.proxy(bearer, BEARER)
                .await?
                .call::<_, _, ()>("Connect", &())
                .await
                .map_err(bus_error)
        })
        .await
    }

    async fn disconnect(&self, bearer: &str) -> Result<(), String> {
        timed(10, async {
            self.proxy(bearer, BEARER)
                .await?
                .call::<_, _, ()>("Disconnect", &())
                .await
                .map_err(bus_error)
        })
        .await
    }

    pub async fn delete(&self, bearer: &str) -> Result<(), String> {
        timed(10, async {
            let path = OwnedObjectPath::try_from(bearer.to_string()).map_err(bus_error)?;
            self.proxy(&self.modem, MODEM)
                .await?
                .call::<_, _, ()>("DeleteBearer", &(path,))
                .await
                .map_err(bus_error)
        })
        .await
    }

    async fn may_clean_interface(&self, own_bearer: &str) -> Result<bool, String> {
        for other in self.bearers().await? {
            if other == own_bearer {
                continue;
            }
            match self.status(&other).await {
                Ok(status) if status.connected && status.interface == self.interface => {
                    return Ok(false)
                }
                Ok(_) => {}
                Err(error) if error == OWNER_MISSING => {}
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}

/// Keep both permits with an already-dispatched read, not with its waiter.
/// The observation deadline bounds lock acquisition and result publication;
/// cancellation/timeout may leave one read draining under its own RPC timeout.
/// No writes are made here, and a late reply is never published as fresh PCO.
async fn retained_serial_read<F>(
    modem: String,
    guard: NetworkGuard,
    deadline: Instant,
    read: F,
) -> Result<String, ImsBearerError>
where
    F: Future<Output = Result<String, ImsBearerError>> + Send + 'static,
{
    tokio::spawn(async move {
        let _lease = guard;
        let _serial = tokio::time::timeout_at(deadline, serial::acquire_for(&modem))
            .await
            .map_err(|_| session_changed("serial_wait_timeout"))?;
        if Instant::now() >= deadline {
            return Err(session_changed("read_timeout_unverified"));
        }
        if _lease.0.closing.load(Ordering::Acquire) || is_shutting_down() {
            return Err(session_changed("lease_closing"));
        }
        read.await
    })
    .await
    .map_err(|_| session_changed("at_read_task_failed"))?
}

fn create_properties<'a>(
    request: &'a PrimaryImsRequest<'_>,
) -> Result<HashMap<&'static str, Value<'a>>, String> {
    let family = request.family.flags();
    let mut properties = HashMap::from([
        ("apn", Value::from(request.apn)),
        ("ip-type", Value::from(family)),
        ("allow-roaming", Value::from(request.allow_roaming)),
    ]);
    if let Some(profile) = request.profile_id {
        // Preserve the caller's profile pin. MM 1.18 QMI loads that profile's
        // IP type and may ignore the separate ip-type property; never mutate
        // or drop the pin to force a different PDN. Consume the actual grant.
        let profile = i32::try_from(profile)
            .map_err(|_| "qca410_primary_mm_profile_id_invalid".to_string())?;
        properties.insert("profile-id", Value::from(profile));
    }
    Ok(properties)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LeaseRecord {
    version: u8,
    bus_id: String,
    owner: String,
    modem: String,
    bearer: String,
    device: String,
    interface: String,
    process_id: u32,
    process_start: u64,
    namespace: Option<String>,
    network: Option<NetdevConfig>,
    /// Version 2 retains the second address so crash recovery cannot leave a
    /// dual-stack address behind. Older binaries reject v2 instead of silently
    /// recovering only the first family. Existing v1 records remain readable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    additional_networks: Vec<NetdevConfig>,
}

impl LeaseRecord {
    fn key(&self) -> LeaseKey {
        (self.bus_id.clone(), self.owner.clone(), self.bearer.clone())
    }

    fn networks(&self) -> impl Iterator<Item = &NetdevConfig> {
        self.network.iter().chain(self.additional_networks.iter())
    }

    fn set_networks(&mut self, networks: &[NetdevConfig]) -> Result<(), String> {
        let Some(first) = networks.first() else {
            return Err("qca410_primary_mm_lease_network_missing".to_string());
        };
        if self.network.is_some() && !self.networks().eq(networks.iter()) {
            // The complete plan is written once, before either family starts.
            // Replacing/shrinking it could forget an already installed address.
            return Err("qca410_primary_mm_lease_network_change_refused".to_string());
        }
        let mut next = self.clone();
        next.network = Some(first.clone());
        next.additional_networks = networks[1..].to_vec();
        next.version = if next.additional_networks.is_empty() {
            1
        } else {
            2
        };
        next.validate()?;
        *self = next;
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        let valid_path = |path: &str, prefix: &str| {
            path.strip_prefix(prefix).is_some_and(|id| {
                !id.is_empty()
                    && id.bytes().all(|byte| byte.is_ascii_digit())
                    && id.parse::<u32>().is_ok()
            })
        };
        let valid_network_schema = match (
            self.version,
            self.network.as_ref(),
            self.additional_networks.as_slice(),
        ) {
            (1, _, []) => true,
            (2, Some(primary), [secondary]) => {
                primary.address.is_ipv4() != secondary.address.is_ipv4()
            }
            _ => false,
        };
        if !valid_network_schema
            || self.bus_id.len() != 32
            || !self.bus_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || zbus::names::UniqueName::try_from(self.owner.as_str()).is_err()
            || !valid_path(&self.modem, "/org/freedesktop/ModemManager1/Modem/")
            || !valid_path(&self.bearer, "/org/freedesktop/ModemManager1/Bearer/")
            || !self.device.starts_with("/dev/")
            || netdev::primary_netdev_for_qmi(&self.device).as_deref()
                != Some(self.interface.as_str())
            || self.interface.len() >= 16
            || self.process_id == 0
            || self.process_start == 0
        {
            return Err("qca410_primary_mm_lease_invalid".to_string());
        }
        if let Some(namespace) = &self.namespace {
            let suffix = namespace.strip_prefix(netns::DEFAULT_NAMESPACE_PREFIX);
            if !suffix.is_some_and(|suffix| {
                suffix.len() == 12 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
            }) || NetnsName::adopt(namespace).is_err()
            {
                return Err("qca410_primary_mm_lease_namespace_invalid".to_string());
            }
        }
        if self.networks().any(|network| {
            network.prefix > if network.address.is_ipv4() { 32 } else { 128 }
                || network
                    .probe_target
                    .is_some_and(|target| target.is_ipv4() != network.address.is_ipv4())
        }) {
            return Err("qca410_primary_mm_lease_network_invalid".to_string());
        }
        Ok(())
    }
}

pub(super) struct OwnedLease {
    bus: Arc<MmBus>,
    record: Mutex<LeaseRecord>,
    file: PathBuf,
    cleanup_lock: tokio::sync::Mutex<()>,
    network_activity: Arc<NetworkActivity>,
    done: AtomicBool,
    pub abandoned: AtomicBool,
}

impl OwnedLease {
    /// Must complete before Connect: an acknowledged active bearer always has
    /// an ownership record even if the caller or the whole process disappears.
    pub fn create(bus: Arc<MmBus>, bearer: &str) -> Result<Arc<Self>, String> {
        let record = LeaseRecord {
            version: 1,
            bus_id: bus.bus_id.clone(),
            owner: bus.owner.clone(),
            modem: bus.modem.clone(),
            bearer: bearer.to_string(),
            device: bus.device.clone(),
            interface: bus.interface.clone(),
            process_id: std::process::id(),
            process_start: process_start(std::process::id())?
                .ok_or_else(|| "qca410_primary_mm_process_identity_missing".to_string())?,
            namespace: None,
            network: None,
            additional_networks: Vec::new(),
        };
        record.validate()?;
        let directory = Path::new(STATE_DIR);
        ensure_directory(directory)?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let file = directory.join(format!(
            "lease-{}-{stamp}-{}.json",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        write_record(&file, &record)?;
        let key = record.key();
        let lease = Arc::new(Self {
            bus,
            record: Mutex::new(record),
            file,
            cleanup_lock: tokio::sync::Mutex::new(()),
            network_activity: Arc::default(),
            done: AtomicBool::new(false),
            abandoned: AtomicBool::new(false),
        });
        leases().lock().unwrap().insert(key, Arc::clone(&lease));
        Ok(lease)
    }

    pub fn path(&self) -> String {
        self.record.lock().unwrap().bearer.clone()
    }

    fn update(
        &self,
        change: impl FnOnce(&mut LeaseRecord) -> Result<(), String>,
    ) -> Result<NetworkGuard, String> {
        let guard = self.network_activity.enter()?;
        let mut stored = self.record.lock().unwrap();
        let mut next = stored.clone();
        change(&mut next)?;
        next.validate()?;
        write_record(&self.file, &next)?;
        *stored = next;
        Ok(guard)
    }

    pub fn network_will_be_configured(
        &self,
        networks: &[NetdevConfig],
    ) -> Result<NetworkGuard, String> {
        self.update(|record| record.set_networks(networks))
    }

    pub fn connection_will_start(&self) -> Result<NetworkGuard, String> {
        self.network_activity.enter()
    }

    pub fn namespace_will_change(&self, namespace: &str) -> Result<NetworkGuard, String> {
        self.update(|record| {
            record.namespace = Some(namespace.to_string());
            Ok(())
        })
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    pub fn is_closing(&self) -> bool {
        self.network_activity.closing.load(Ordering::Acquire)
    }

    pub async fn cleanup(&self) -> Result<(), String> {
        let _guard = self.cleanup_lock.lock().await;
        if self.is_done() {
            return Ok(());
        }
        self.network_activity.closing.store(true, Ordering::Release);
        // Do not erase the recovery intent while an interface move/configure
        // operation may still complete after the cleanup snapshot.
        while self.network_activity.active.load(Ordering::Acquire) != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let record = self.record.lock().unwrap().clone();
        if !self.bus.owner_is_current().await? {
            // IDs can be reused by a new MM daemon. Never redirect an old lease
            // to that daemon, and do not mutate its interface using old metadata.
            return self.forget(&record);
        }
        let mut network_error = None;
        match self.bus.may_clean_interface(&record.bearer).await {
            Ok(true) => {
                if let Some(namespace) = record.namespace.as_deref() {
                    let namespace = NetnsName::adopt(namespace).map_err(|e| e.to_string())?;
                    if netns::exists(&namespace) {
                        if let Err(error) =
                            netns::move_iface_out(&namespace, &record.interface).await
                        {
                            network_error = Some(error.to_string());
                        }
                    }
                }
                if network_error.is_none() {
                    network_error =
                        cleanup_networks_with(&record, |interface, network| async move {
                            netdev::teardown_verified(&interface, &network).await
                        })
                        .await
                        .err();
                }
            }
            Ok(false) => {
                tracing::warn!(
                    "Skipping old IMS interface cleanup: another MM bearer owns the interface"
                );
            }
            Err(error) if error == OWNER_MISSING => {}
            Err(error) => network_error = Some(error),
        }
        let _ = self.bus.disconnect(&record.bearer).await;
        match self.bus.delete(&record.bearer).await {
            Ok(()) => {}
            Err(error) if error == OWNER_MISSING => {}
            Err(error) => return Err(error),
        }
        if let Some(error) = network_error {
            return Err(error); // Keep the ledger so recovery can finish the link cleanup.
        }
        self.forget(&record)
    }

    fn forget(&self, record: &LeaseRecord) -> Result<(), String> {
        remove_record(&self.file)?;
        self.done.store(true, Ordering::Release);
        leases().lock().unwrap().remove(&record.key());
        Ok(())
    }
}

/// Attempt every recorded family even if one cleanup fails. Returning an
/// error prevents `forget`, so a later recovery can retry the complete plan.
async fn cleanup_networks_with<F, R>(record: &LeaseRecord, mut cleanup: F) -> Result<(), String>
where
    F: FnMut(String, NetdevConfig) -> R,
    R: Future<Output = Result<(), String>>,
{
    let mut first_error = None;
    for network in record.networks() {
        if let Err(error) = cleanup(record.interface.clone(), network.clone()).await {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub(super) fn cleanup_in_background(lease: Arc<OwnedLease>) {
    lease.abandoned.store(true, Ordering::Release);
    if lease.is_done() {
        return;
    }
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        runtime.spawn(async move {
            if let Err(error) = lease.cleanup().await {
                tracing::warn!(error, "Owned IMS cleanup deferred; durable lease retained");
            }
        });
    }
}

fn ensure_directory(directory: &Path) -> Result<(), String> {
    if directory == Path::new(STATE_DIR) {
        let parent = directory.parent().expect("runtime parent");
        if parent.exists() {
            let metadata = fs::symlink_metadata(parent)
                .map_err(|_| "qca410_primary_mm_lease_parent_failed".to_string())?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err("qca410_primary_mm_lease_parent_unsafe".to_string());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
                    return Err("qca410_primary_mm_lease_parent_unsafe".to_string());
                }
            }
        }
    }
    if !directory.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(directory)
            .map_err(|_| "qca410_primary_mm_lease_directory_failed".to_string())?;
    }
    let metadata = fs::symlink_metadata(directory)
        .map_err(|_| "qca410_primary_mm_lease_directory_failed".to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("qca410_primary_mm_lease_directory_unsafe".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err("qca410_primary_mm_lease_directory_permissions".to_string());
        }
        if directory == Path::new(STATE_DIR) {
            let parent = fs::symlink_metadata(directory.parent().expect("runtime parent"))
                .map_err(|_| "qca410_primary_mm_lease_parent_failed".to_string())?;
            if parent.file_type().is_symlink()
                || parent.uid() != metadata.uid()
                || parent.mode() & 0o022 != 0
            {
                return Err("qca410_primary_mm_lease_parent_unsafe".to_string());
            }
        }
    }
    Ok(())
}

fn write_record(file: &Path, record: &LeaseRecord) -> Result<(), String> {
    let parent = file
        .parent()
        .ok_or_else(|| "qca410_primary_mm_lease_path_invalid".to_string())?;
    let temporary = parent.join(format!(
        ".pending-{}-{}",
        std::process::id(),
        NEXT_FILE.fetch_add(1, Ordering::Relaxed)
    ));
    let data = serde_json::to_vec(record)
        .map_err(|_| "qca410_primary_mm_lease_encode_failed".to_string())?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| -> std::io::Result<()> {
        let mut output = options.open(&temporary)?;
        output.write_all(&data)?;
        output.sync_all()?;
        fs::rename(&temporary, file)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return Err("qca410_primary_mm_lease_write_failed".to_string());
    }
    Ok(())
}

fn remove_record(file: &Path) -> Result<(), String> {
    match fs::remove_file(file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("qca410_primary_mm_lease_remove_failed".to_string()),
    }
}

fn read_record(file: &Path) -> Result<LeaseRecord, String> {
    let metadata = fs::symlink_metadata(file)
        .map_err(|_| "qca410_primary_mm_lease_read_failed".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_RECORD_BYTES
    {
        return Err("qca410_primary_mm_lease_file_unsafe".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err("qca410_primary_mm_lease_file_permissions".to_string());
        }
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(file)
        .map_err(|_| "qca410_primary_mm_lease_read_failed".to_string())?;
    let record: LeaseRecord =
        serde_json::from_reader(std::io::Read::take(file, MAX_RECORD_BYTES + 1))
            .map_err(|_| "qca410_primary_mm_lease_decode_failed".to_string())?;
    record.validate()?;
    Ok(record)
}

fn parse_process_start(stat: &str) -> Option<u64> {
    stat.rsplit_once(") ")?
        .1
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

fn process_start(pid: u32) -> Result<Option<u64>, String> {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => parse_process_start(&stat)
            .map(Some)
            .ok_or_else(|| "qca410_primary_mm_process_identity_invalid".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("qca410_primary_mm_process_identity_unreadable".to_string()),
    }
}

fn same_generation(record: &LeaseRecord, bus_id: &str, owner: &str) -> bool {
    record.bus_id == bus_id && record.owner == owner
}

/// Reap only dead/abandoned application leases from the same bus AND MM owner.
pub(super) async fn recover_owned() -> Result<(), String> {
    let directory = Path::new(STATE_DIR);
    if !directory.exists() {
        return Ok(());
    }
    ensure_directory(directory)?;
    let entries =
        fs::read_dir(directory).map_err(|_| "qca410_primary_mm_lease_scan_failed".to_string())?;
    for entry in entries {
        let entry = entry.map_err(|_| "qca410_primary_mm_lease_scan_failed".to_string())?;
        let file = entry.path();
        if file.extension().and_then(|value| value.to_str()) != Some("json") {
            continue; // Incomplete atomic writes cannot precede an active bearer.
        }
        let record = read_record(&file)?;
        let existing = leases().lock().unwrap().get(&record.key()).cloned();
        if let Some(lease) = existing {
            if lease.abandoned.load(Ordering::Acquire) {
                lease.cleanup().await?;
            }
            continue;
        }
        if process_start(record.process_id)? == Some(record.process_start) {
            continue; // Another live SimAdmin instance is not ours to stop.
        }
        let bus = match MmBus::new(&record.device, &record.modem, &record.interface).await {
            Ok(bus) => bus,
            Err(error) if error == OWNER_MISSING => {
                remove_record(&file)?;
                continue;
            }
            Err(error) => return Err(error),
        };
        if !same_generation(&record, &bus.bus_id, &bus.owner) {
            remove_record(&file)?;
            continue; // A new daemon may have reused the exact same object path.
        }
        let key = record.key();
        let lease = Arc::new(OwnedLease {
            bus,
            record: Mutex::new(record),
            file,
            cleanup_lock: tokio::sync::Mutex::new(()),
            network_activity: Arc::default(),
            done: AtomicBool::new(false),
            abandoned: AtomicBool::new(true),
        });
        leases().lock().unwrap().insert(key, Arc::clone(&lease));
        lease.cleanup().await?;
    }
    Ok(())
}

pub(super) async fn shutdown_owned() {
    begin_shutdown();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let active: Vec<_> = leases().lock().unwrap().values().cloned().collect();
            if active.is_empty() && PENDING.load(Ordering::Acquire) == 0 {
                break;
            }
            let results =
                futures_util::future::join_all(active.iter().map(|lease| lease.cleanup())).await;
            if results.iter().any(Result::is_err) {
                tracing::warn!("Some IMS leases could not be released; recovery records retained");
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if result.is_err() {
        tracing::warn!(
            "IMS shutdown exceeded its budget; durable ownership records retained for recovery"
        );
    }
}

#[cfg(test)]
mod ip_config_dbus_tests {
    use super::*;

    const PATH: &str = "/org/freedesktop/ModemManager1/Bearer/91";

    struct FakeBearer {
        connected: Arc<AtomicBool>,
        disconnect_during_read: bool,
        profile_id: Arc<std::sync::atomic::AtomicI32>,
        changed_ip: Arc<AtomicBool>,
    }

    struct FakeModem {
        connected: Arc<AtomicBool>,
        profile_id: Arc<std::sync::atomic::AtomicI32>,
        changed_ip: Arc<AtomicBool>,
        commands: Arc<Mutex<Vec<String>>>,
        action: &'static str,
    }

    #[zbus::interface(name = "org.freedesktop.ModemManager1.Modem")]
    impl FakeModem {
        #[zbus(property)]
        fn primary_port(&self) -> String {
            "wwan0qmi0".to_string()
        }
        #[zbus(property)]
        fn bearers(&self) -> Vec<OwnedObjectPath> {
            vec![OwnedObjectPath::try_from(PATH).unwrap()]
        }
        fn command(&self, command: &str, _timeout: u32) -> String {
            self.commands.lock().unwrap().push(command.to_string());
            match command {
                "AT+CGACT?" => "+CGACT: 1,0\n+CGACT: 2,1".into(),
                "AT+CGDCONT?" => {
                    "+CGDCONT: 1,\"IPV4V6\",\"internet\"\n+CGDCONT: 2,\"IPV4V6\",\"ims\"".into()
                }
                "AT+CGCONTRDP=2" => {
                    match self.action {
                        "disconnect" => self.connected.store(false, Ordering::Release),
                        "profile" => self.profile_id.store(3, Ordering::Release),
                        "ip" => self.changed_ip.store(true, Ordering::Release),
                        "error" => return "ERROR".into(),
                        "error_profile" => {
                            self.profile_id.store(3, Ordering::Release);
                            return "ERROR".into();
                        }
                        "error_ip" => {
                            self.changed_ip.store(true, Ordering::Release);
                            return "ERROR".into();
                        }
                        "error_disconnect" => {
                            self.connected.store(false, Ordering::Release);
                            return "ERROR".into();
                        }
                        _ => {}
                    }
                    "+CGCONTRDP: 2,5,ims,2001:db8::a,2001:db8::b,,,2001:db8:2::10,2001:db8:2::11"
                        .into()
                }
                _ => "ERROR".into(),
            }
        }
    }

    fn text(value: &str) -> OwnedValue {
        OwnedValue::try_from(Value::from(value)).unwrap()
    }

    fn ip_config(ipv6: bool) -> HashMap<String, OwnedValue> {
        HashMap::from([
            ("method".into(), OwnedValue::from(2_u32)),
            (
                "address".into(),
                text(if ipv6 { "2001:db8::2" } else { "192.0.2.2" }),
            ),
            (
                "prefix".into(),
                OwnedValue::from(if ipv6 { 64_u32 } else { 30_u32 }),
            ),
            (
                "gateway".into(),
                text(if ipv6 { "2001:db8::1" } else { "192.0.2.1" }),
            ),
            (
                "dns1".into(),
                text(if ipv6 { "2001:db8::53" } else { "192.0.2.53" }),
            ),
        ])
    }

    #[zbus::interface(name = "org.freedesktop.ModemManager1.Bearer")]
    impl FakeBearer {
        #[zbus(property)]
        fn connected(&self) -> bool {
            self.connected.load(Ordering::Acquire)
        }
        #[zbus(property)]
        fn interface(&self) -> String {
            "wwan0".to_string()
        }
        #[zbus(property)]
        fn properties(&self) -> HashMap<String, OwnedValue> {
            HashMap::from([
                ("apn".into(), text("ims")),
                (
                    "profile-id".into(),
                    OwnedValue::from(self.profile_id.load(Ordering::Acquire)),
                ),
            ])
        }
        #[zbus(property)]
        fn ip4_config(&self) -> HashMap<String, OwnedValue> {
            if self.disconnect_during_read {
                self.connected.store(false, Ordering::Release);
            }
            ip_config(false)
        }
        #[zbus(property)]
        fn ip6_config(&self) -> HashMap<String, OwnedValue> {
            if self.disconnect_during_read {
                self.connected.store(false, Ordering::Release);
            }
            let mut config = ip_config(true);
            if self.changed_ip.load(Ordering::Acquire) {
                config.insert("address".into(), text("2001:db8::3"));
            }
            config
        }
    }

    async fn server(disconnect_during_read: bool) -> Connection {
        probe_server(disconnect_during_read, "").await.0
    }

    async fn probe_server(
        disconnect_during_read: bool,
        action: &'static str,
    ) -> (Connection, Arc<Mutex<Vec<String>>>) {
        // This filter is executed explicitly inside dbus-run-session on CI.
        // Never register a fake MM daemon on a real machine's system bus.
        let session = std::env::var("DBUS_SESSION_BUS_ADDRESS")
            .expect("run this test filter under dbus-run-session");
        assert_eq!(
            std::env::var("DBUS_SYSTEM_BUS_ADDRESS").ok().as_deref(),
            Some(session.as_str())
        );
        let connected = Arc::new(AtomicBool::new(true));
        let profile_id = Arc::new(std::sync::atomic::AtomicI32::new(2));
        let changed_ip = Arc::new(AtomicBool::new(false));
        let commands = Arc::new(Mutex::new(Vec::new()));
        let connection = zbus::connection::Builder::system()
            .unwrap()
            .name(SERVICE)
            .unwrap()
            .serve_at(
                "/org/freedesktop/ModemManager1/Modem/0",
                FakeModem {
                    connected: Arc::clone(&connected),
                    profile_id: Arc::clone(&profile_id),
                    changed_ip: Arc::clone(&changed_ip),
                    commands: Arc::clone(&commands),
                    action,
                },
            )
            .unwrap()
            .serve_at(
                PATH,
                FakeBearer {
                    connected,
                    disconnect_during_read,
                    profile_id,
                    changed_ip,
                },
            )
            .unwrap()
            .build()
            .await
            .unwrap();
        (connection, commands)
    }

    async fn bus() -> Arc<MmBus> {
        MmBus::new(
            "/dev/wwan0qmi0",
            "/org/freedesktop/ModemManager1/Modem/0",
            "wwan0",
        )
        .await
        .unwrap()
    }

    fn observation_guard() -> NetworkGuard {
        Arc::new(NetworkActivity::default()).enter().unwrap()
    }

    #[tokio::test]
    async fn owned_pcscf_uses_only_the_original_mm_read_only_command_path() {
        let (_server, commands) = probe_server(false, "").await;
        let bus = bus().await;
        let expected = bus
            .ip_settings(PATH, "ims", MmIpFamily::Ipv6)
            .await
            .unwrap()
            .unwrap();
        let discovery = bus
            .discover_pcscf(
                PATH,
                "ims",
                MmIpFamily::Ipv6,
                Some(2),
                &expected,
                &observation_guard(),
            )
            .await
            .unwrap();
        assert_eq!(discovery.source, "mm_owned_at_sole_pinned_ipv6_prefix");
        assert_eq!(discovery.candidates.len(), 2);
        assert_eq!(
            commands.lock().unwrap().as_slice(),
            [
                "AT+CGACT?",
                "AT+CGDCONT?",
                "AT+CGCONTRDP=2",
                "AT+CGACT?",
                "AT+CGDCONT?",
                "AT+CGCONTRDP=2",
            ]
        );
    }

    #[tokio::test]
    async fn actual_profile_pin_mismatch_blocks_at_before_the_first_command() {
        let (_server, commands) = probe_server(false, "").await;
        let bus = bus().await;
        let expected = bus
            .ip_settings(PATH, "ims", MmIpFamily::Ipv6)
            .await
            .unwrap()
            .unwrap();
        let error = bus
            .discover_pcscf(
                PATH,
                "ims",
                MmIpFamily::Ipv6,
                Some(3),
                &expected,
                &observation_guard(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::hardware::devices::transport::ImsBearerErrorKind::SessionLost
        );
        assert!(commands.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn pcscf_is_not_published_after_live_bearer_profile_or_ip_changes() {
        for action in [
            "disconnect",
            "profile",
            "ip",
            "error_profile",
            "error_ip",
            "error_disconnect",
        ] {
            let (server, _) = probe_server(false, action).await;
            let bus = bus().await;
            let expected = bus
                .ip_settings(PATH, "ims", MmIpFamily::Ipv6)
                .await
                .unwrap()
                .unwrap();
            let error = bus
                .discover_pcscf(
                    PATH,
                    "ims",
                    MmIpFamily::Ipv6,
                    Some(2),
                    &expected,
                    &observation_guard(),
                )
                .await
                .unwrap_err();
            assert_eq!(
                error.kind,
                crate::hardware::devices::transport::ImsBearerErrorKind::SessionLost,
                "{action}"
            );
            server.release_name(SERVICE).await.unwrap();
        }
    }

    #[tokio::test]
    async fn pcscf_adapter_rejects_mutating_commands_and_replaced_owners() {
        let (first, old_commands) = probe_server(false, "").await;
        let bus = bus().await;
        for command in [
            "AT+CGACT=0,2",
            "AT+CGDCONT=2,IPV6,ims",
            "AT+CFUN=0",
            "AT+CGCONTRDP=2;AT+CFUN=0",
            "AT+CGCONTRDP=02",
        ] {
            assert!(bus
                .pcscf_at_read(
                    command,
                    &observation_guard(),
                    Instant::now() + primary_ims_pcscf::BUDGET
                )
                .await
                .unwrap_err()
                .detail
                .ends_with("command_not_read_only"));
        }
        assert!(old_commands.lock().unwrap().is_empty());
        first.release_name(SERVICE).await.unwrap();
        let (_replacement, new_commands) = probe_server(false, "").await;
        let error = bus
            .pcscf_at_read(
                "AT+CGACT?",
                &observation_guard(),
                Instant::now() + primary_ims_pcscf::BUDGET,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::hardware::devices::transport::ImsBearerErrorKind::SessionLost
        );
        assert!(old_commands.lock().unwrap().is_empty());
        assert!(new_commands.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn unsupported_at_is_a_pcscf_failure_not_an_ip_config_replacement() {
        let (_server, _) = probe_server(false, "error").await;
        let bus = bus().await;
        let expected = bus
            .ip_settings(PATH, "ims", MmIpFamily::Ipv6)
            .await
            .unwrap()
            .unwrap();
        let error = bus
            .discover_pcscf(
                PATH,
                "ims",
                MmIpFamily::Ipv6,
                Some(2),
                &expected,
                &observation_guard(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.kind,
            crate::hardware::devices::transport::ImsBearerErrorKind::PcscfUnavailable
        );
        assert_eq!(
            bus.ip_settings(PATH, "ims", MmIpFamily::Ipv6)
                .await
                .unwrap()
                .unwrap(),
            expected
        );
    }

    #[tokio::test]
    async fn pcscf_commands_share_the_existing_modem_serial_permit() {
        let (_server, commands) = probe_server(false, "").await;
        let bus = bus().await;
        let permit = serial::acquire_for(&bus.modem).await;
        let reader = Arc::clone(&bus);
        let task = tokio::spawn(async move {
            reader
                .pcscf_at_read(
                    "AT+CGACT?",
                    &observation_guard(),
                    Instant::now() + primary_ims_pcscf::BUDGET,
                )
                .await
        });
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(commands.lock().unwrap().is_empty());
        tokio::time::timeout(
            Duration::from_secs(1),
            serial::with_serial_for("other-pcscf-modem", async {}),
        )
        .await
        .unwrap();
        drop(permit);
        assert!(task.await.unwrap().unwrap().contains("+CGACT:"));
        assert_eq!(commands.lock().unwrap().as_slice(), ["AT+CGACT?"]);
    }

    #[tokio::test]
    async fn typed_get_all_reads_bearer_dns_without_at_or_hardware() {
        let _server = server(false).await;
        let bus = bus().await;
        let ipv6 = bus
            .ip_settings(PATH, "ims", MmIpFamily::Ipv6)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ipv6.ipv6_address, Some("2001:db8::2".parse().unwrap()));
        assert_eq!(
            ipv6.ipv6_dns,
            vec!["2001:db8::53".parse::<std::net::IpAddr>().unwrap()]
        );
        assert!(ipv6.ipv4_address.is_none());
        assert!(ipv6.pcscf.is_empty());
        let ipv4 = bus
            .ip_settings(PATH, "ims", MmIpFamily::Ipv4)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ipv4.ipv4_prefix, Some(30));
        let dual = bus
            .ip_settings(PATH, "ims", MmIpFamily::Ipv4v6)
            .await
            .unwrap()
            .unwrap();
        assert!(dual.ipv4_address.is_some() && dual.ipv6_address.is_some());
        assert_eq!(dual.ipv4_dns.len(), 1);
        assert_eq!(dual.ipv6_dns.len(), 1);
        assert!(bus
            .ip_settings(PATH, "internet", MmIpFamily::Ipv6)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn disconnected_during_get_all_does_not_publish_stale_addressing() {
        let _server = server(true).await;
        let bus = bus().await;
        assert_eq!(
            bus.ip_settings(PATH, "ims", MmIpFamily::Ipv4v6)
                .await
                .unwrap_err(),
            "qca410_primary_mm_bearer_not_connected"
        );
    }

    #[tokio::test]
    async fn replaced_mm_owner_cannot_supply_an_old_sessions_ip_config() {
        let first = server(false).await;
        let bus = bus().await;
        first.release_name(SERVICE).await.unwrap();
        // The old unique connection still exists and can answer its old path,
        // but it is no longer the MM owner. Do not consume it or redirect to
        // the replacement daemon, even when the latter reuses the same path.
        let _replacement = server(false).await;
        assert_eq!(
            bus.ip_settings(PATH, "ims", MmIpFamily::Ipv4v6)
                .await
                .unwrap_err(),
            OWNER_MISSING
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn cancelled_read_keeps_permits(until_deadline: bool) {
        let activity = Arc::new(NetworkActivity::default());
        let guard = activity.enter().unwrap();
        let modem = format!("pcscf-cancel-{until_deadline}");
        let key = modem.clone();
        let (started, start) = tokio::sync::oneshot::channel();
        let (finish, finished) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let read = retained_serial_read(
                modem,
                guard,
                Instant::now() + Duration::from_secs(2),
                async move {
                    let _ = started.send(());
                    let _ = finished.await;
                    Ok("late read result".to_string())
                },
            );
            if until_deadline {
                assert!(tokio::time::timeout(Duration::from_millis(40), read)
                    .await
                    .is_err());
            } else {
                let _ = read.await;
            }
        });
        start.await.unwrap();
        if until_deadline {
            task.await.unwrap();
        } else {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        }
        assert_eq!(activity.active.load(Ordering::Acquire), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(20), serial::acquire_for(&key))
                .await
                .is_err()
        );
        // Cleanup may close admission, but it must still wait for this read.
        activity.closing.store(true, Ordering::Release);
        assert!(activity.enter().is_err());
        finish.send(()).unwrap();
        let _permit = tokio::time::timeout(Duration::from_secs(1), serial::acquire_for(&key))
            .await
            .unwrap();
        assert_eq!(activity.active.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn cancelled_pcscf_caller_cannot_release_a_dispatched_reads_permits() {
        cancelled_read_keeps_permits(false).await;
    }

    #[tokio::test]
    async fn pcscf_publication_timeout_drains_one_dispatched_read_without_publishing_it() {
        cancelled_read_keeps_permits(true).await;
    }

    #[tokio::test]
    async fn expired_serial_wait_does_not_start_a_late_at_command() {
        let key = "pcscf-expired-lock";
        let _permit = serial::acquire_for(key).await;
        let activity = Arc::new(NetworkActivity::default());
        let started = Arc::new(AtomicBool::new(false));
        let started_read = Arc::clone(&started);
        let error = retained_serial_read(
            key.to_string(),
            activity.enter().unwrap(),
            Instant::now() + Duration::from_millis(20),
            async move {
                started_read.store(true, Ordering::Release);
                Ok(String::new())
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind, ImsBearerErrorKind::SessionLost);
        assert!(!started.load(Ordering::Acquire));
        assert_eq!(activity.active.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn closing_a_lease_prevents_queued_reads_after_the_serial_wait() {
        let key = "pcscf-closing-lock";
        let permit = serial::acquire_for(key).await;
        let activity = Arc::new(NetworkActivity::default());
        let guard = activity.enter().unwrap();
        let task = tokio::spawn(retained_serial_read(
            key.to_string(),
            guard,
            Instant::now() + Duration::from_secs(2),
            async { panic!("a closing lease must not dispatch a queued read") },
        ));
        activity.closing.store(true, Ordering::Release);
        drop(permit);
        let error = task.await.unwrap().unwrap_err();
        assert!(error.detail.ends_with("lease_closing"));
        assert_eq!(activity.active.load(Ordering::Acquire), 0);
    }

    fn record() -> LeaseRecord {
        LeaseRecord {
            version: 1,
            bus_id: "0123456789abcdef0123456789abcdef".to_string(),
            owner: ":1.42".to_string(),
            modem: "/org/freedesktop/ModemManager1/Modem/0".to_string(),
            bearer: "/org/freedesktop/ModemManager1/Bearer/9".to_string(),
            device: "/dev/wwan0qmi0".to_string(),
            interface: "wwan0".to_string(),
            process_id: 123,
            process_start: 456,
            namespace: Some("sa-ue0123456789ab".to_string()),
            network: None,
            additional_networks: Vec::new(),
        }
    }

    #[test]
    fn recycled_mm_object_requires_the_same_bus_and_unique_owner() {
        let record = record();
        assert!(same_generation(&record, &record.bus_id, &record.owner));
        assert!(!same_generation(&record, &record.bus_id, ":1.43"));
        assert!(!same_generation(&record, "different-bus", &record.owner));
    }

    #[test]
    fn only_primary_device_paths_and_private_ue_namespaces_are_valid() {
        let good = record();
        good.validate().unwrap();
        for namespace in [
            "../sa-ue0123456789ab",
            "host",
            "other-app",
            "sa-ue../broken",
        ] {
            let mut bad = good.clone();
            bad.namespace = Some(namespace.to_string());
            assert!(bad.validate().is_err());
        }
        let mut secondary = good.clone();
        secondary.device = "/dev/wwan0at1".to_string();
        assert!(secondary.validate().is_err());
        let mut foreign = good;
        foreign.owner = SERVICE.to_string();
        assert!(
            foreign.validate().is_err(),
            "must pin a unique owner, not a well-known name"
        );
    }

    #[test]
    fn process_identity_handles_spaces_parentheses_and_pid_reuse() {
        let mut fields = vec!["0"; 20];
        fields[0] = "S";
        fields[19] = "456";
        let stat = format!("123 (simadmin ) worker) {}", fields.join(" "));
        assert_eq!(parse_process_start(&stat), Some(456));
        assert_ne!(parse_process_start(&stat), Some(457));
        assert_eq!(parse_process_start("truncated"), None);
    }

    #[test]
    fn cleanup_must_wait_for_network_mutations_and_reject_late_ones() {
        let activity = Arc::new(NetworkActivity::default());
        let first = activity.enter().unwrap();
        assert_eq!(activity.active.load(Ordering::Acquire), 1);
        activity.closing.store(true, Ordering::Release);
        assert!(activity.enter().is_err());
        assert_eq!(activity.active.load(Ordering::Acquire), 1);
        drop(first);
        assert_eq!(activity.active.load(Ordering::Acquire), 0);
    }

    #[test]
    fn dbus_ip_family_and_profile_types_match_modemmanager() {
        for (family, flags) in [
            (MmIpFamily::Ipv4, 1_u32),
            (MmIpFamily::Ipv6, 2_u32),
            (MmIpFamily::Ipv4v6, 4_u32),
        ] {
            let request = PrimaryImsRequest {
                device: "/dev/wwan0qmi0",
                modem: "/org/freedesktop/ModemManager1/Modem/0",
                interface: "wwan0",
                apn: "ims",
                profile_id: Some(2),
                family,
                allow_roaming: false,
            };
            let properties = create_properties(&request).unwrap();
            assert_eq!(
                u32::try_from(properties.get("ip-type").unwrap()).unwrap(),
                flags
            );
            assert_eq!(
                i32::try_from(properties.get("profile-id").unwrap()).unwrap(),
                2
            );
            assert_eq!(
                <&str>::try_from(properties.get("apn").unwrap()).unwrap(),
                "ims"
            );
            assert!(!bool::try_from(properties.get("allow-roaming").unwrap()).unwrap());
        }
    }

    fn network(ipv6: bool) -> NetdevConfig {
        NetdevConfig {
            address: if ipv6 { "2001:db8::2" } else { "192.0.2.2" }
                .parse()
                .unwrap(),
            prefix: if ipv6 { 64 } else { 30 },
            mtu: None,
            probe_target: Some(
                if ipv6 { "2001:db8::53" } else { "192.0.2.53" }
                    .parse()
                    .unwrap(),
            ),
        }
    }

    #[test]
    fn legacy_v1_json_without_additional_networks_remains_readable() {
        let legacy = r#"{
            "version":1,"bus_id":"0123456789abcdef0123456789abcdef",
            "owner":":1.42","modem":"/org/freedesktop/ModemManager1/Modem/0",
            "bearer":"/org/freedesktop/ModemManager1/Bearer/9",
            "device":"/dev/wwan0qmi0","interface":"wwan0",
            "process_id":123,"process_start":456,"namespace":null,
            "network":{"address":"192.0.2.2","prefix":30,"mtu":null,"probe_target":null}
        }"#;
        let record: LeaseRecord = serde_json::from_str(legacy).unwrap();
        record.validate().unwrap();
        assert_eq!(record.networks().count(), 1);
        assert!(record.additional_networks.is_empty());
        assert!(!serde_json::to_string(&record)
            .unwrap()
            .contains("additional_networks"));
    }

    #[test]
    fn dual_receipts_round_trip_every_address_in_both_preference_orders() {
        for ipv6_first in [false, true] {
            let mut original = record();
            let networks = [network(ipv6_first), network(!ipv6_first)];
            original.set_networks(&networks).unwrap();
            assert_eq!(
                original.version, 2,
                "old readers must refuse a dual receipt"
            );
            let encoded = serde_json::to_vec(&original).unwrap();
            let decoded: LeaseRecord = serde_json::from_slice(&encoded).unwrap();
            decoded.validate().unwrap();
            assert!(decoded.networks().eq(networks.iter()));
        }
    }

    #[test]
    fn malformed_receipt_network_versions_are_rejected() {
        let mut dual = record();
        dual.set_networks(&[network(false), network(true)]).unwrap();
        let mut cases = Vec::new();
        let mut bad = dual.clone();
        bad.version = 1;
        cases.push(bad);
        let mut bad = dual.clone();
        bad.version = 3;
        cases.push(bad);
        let mut bad = dual.clone();
        bad.network = None;
        cases.push(bad);
        let mut bad = dual.clone();
        bad.additional_networks.clear();
        cases.push(bad);
        let mut bad = dual.clone();
        bad.additional_networks = vec![network(false)];
        cases.push(bad);
        let mut bad = dual.clone();
        bad.additional_networks.push(network(false));
        cases.push(bad);
        let mut bad = dual.clone();
        bad.additional_networks[0].prefix = 129;
        cases.push(bad);
        let mut bad = dual;
        bad.additional_networks[0].probe_target = Some("192.0.2.53".parse().unwrap());
        cases.push(bad);
        for bad in cases {
            assert!(bad.validate().is_err());
        }
    }

    #[test]
    fn recorded_network_plan_cannot_be_replaced_or_shrunk() {
        let mut stored = record();
        let networks = [network(false), network(true)];
        stored.set_networks(&networks).unwrap();
        stored.set_networks(&networks).unwrap(); // Idempotent repeat is harmless.
        let original = serde_json::to_vec(&stored).unwrap();
        for changed in [
            vec![],
            vec![network(false)],
            vec![network(true), network(false)],
        ] {
            assert!(stored.set_networks(&changed).is_err());
            assert_eq!(serde_json::to_vec(&stored).unwrap(), original);
        }
        let mut fresh = record();
        assert!(fresh
            .set_networks(&[network(false), network(false)])
            .is_err());
        assert!(
            fresh.network.is_none(),
            "invalid plans must not mutate even in memory"
        );
    }

    #[tokio::test]
    async fn cleanup_failure_still_attempts_both_families_and_keeps_the_plan_for_retry() {
        let mut stored = record();
        let networks = [network(true), network(false)];
        stored.set_networks(&networks).unwrap();
        let original = serde_json::to_vec(&stored).unwrap();
        for failed_family in [4, 6] {
            let mut attempted = Vec::new();
            let result = cleanup_networks_with(&stored, |interface, network| {
                assert_eq!(interface, "wwan0");
                let fail = network.address.is_ipv4() == (failed_family == 4);
                attempted.push(network);
                std::future::ready(if fail {
                    Err("injected_cleanup_failure".to_string())
                } else {
                    Ok(())
                })
            })
            .await;
            assert_eq!(result.unwrap_err(), "injected_cleanup_failure");
            assert_eq!(attempted, networks);
            assert_eq!(serde_json::to_vec(&stored).unwrap(), original);
        }
        let mut retried = Vec::new();
        cleanup_networks_with(&stored, |_, network| {
            retried.push(network);
            std::future::ready(Ok(()))
        })
        .await
        .unwrap();
        assert_eq!(retried, networks);
    }

    #[test]
    fn atomic_records_round_trip_without_subscriber_or_authentication_fields() {
        let root = std::env::temp_dir().join(format!(
            "simadmin-mm-lease-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        ensure_directory(&root).unwrap();
        let path = root.join("lease.json");
        let mut expected = record();
        write_record(&path, &expected).unwrap();
        assert_eq!(read_record(&path).unwrap().bearer, expected.bearer);
        expected.network = Some(NetdevConfig {
            address: "192.0.2.1".parse().unwrap(),
            prefix: 32,
            mtu: None,
            probe_target: None,
        });
        write_record(&path, &expected).unwrap();
        assert_eq!(read_record(&path).unwrap().network, expected.network);
        let mut dual = record();
        dual.set_networks(&[network(true), network(false)]).unwrap();
        write_record(&path, &dual).unwrap();
        let restored = read_record(&path).unwrap();
        assert_eq!(restored.version, 2);
        assert!(restored.networks().eq(dual.networks()));
        let text = fs::read_to_string(&path).unwrap();
        for forbidden in ["imsi", "iccid", "password", "cookie", "nonce"] {
            assert!(!text.contains(forbidden));
        }
        remove_record(&path).unwrap();
        remove_record(&path).unwrap();
        fs::remove_dir(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlink_records_are_rejected_without_touching_the_target() {
        let root = std::env::temp_dir().join(format!(
            "simadmin-mm-symlink-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        ensure_directory(&root).unwrap();
        let target = root.join("target");
        fs::write(&target, b"do not touch").unwrap();
        let link = root.join("lease.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(read_record(&link).is_err());
        assert_eq!(fs::read(&target).unwrap(), b"do not touch");
        fs::remove_file(link).unwrap();
        fs::remove_file(target).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
