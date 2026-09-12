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
use zbus::{
    fdo::DBusProxy,
    proxy::CacheProperties,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
    Connection, Proxy,
};

use crate::platform::netns::{self, NetnsName};

use super::{
    netdev::{self, NetdevConfig},
    primary_ims_session::{safe_error, PrimaryImsRequest, OWNER_MISSING},
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

fn create_properties<'a>(
    request: &'a PrimaryImsRequest<'_>,
) -> Result<HashMap<&'static str, Value<'a>>, String> {
    let family = match request.family {
        4 => 1_u32, // MMBearerIpFamily flags, not the QMI 4/6 enum.
        6 => 2_u32,
        _ => return Err("qca410_primary_mm_requires_explicit_ip_family".to_string()),
    };
    let mut properties = HashMap::from([
        ("apn", Value::from(request.apn)),
        ("ip-type", Value::from(family)),
        ("allow-roaming", Value::from(request.allow_roaming)),
    ]);
    if let Some(profile) = request.profile_id {
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
}

impl LeaseRecord {
    fn key(&self) -> LeaseKey {
        (self.bus_id.clone(), self.owner.clone(), self.bearer.clone())
    }

    fn validate(&self) -> Result<(), String> {
        let valid_path = |path: &str, prefix: &str| {
            path.strip_prefix(prefix).is_some_and(|id| {
                !id.is_empty()
                    && id.bytes().all(|byte| byte.is_ascii_digit())
                    && id.parse::<u32>().is_ok()
            })
        };
        if self.version != 1
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
        if self.network.as_ref().is_some_and(|network| {
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

    fn update(&self, change: impl FnOnce(&mut LeaseRecord)) -> Result<NetworkGuard, String> {
        let guard = self.network_activity.enter()?;
        let mut stored = self.record.lock().unwrap();
        let mut next = stored.clone();
        change(&mut next);
        next.validate()?;
        write_record(&self.file, &next)?;
        *stored = next;
        Ok(guard)
    }

    pub fn network_will_be_configured(
        &self,
        network: &NetdevConfig,
    ) -> Result<NetworkGuard, String> {
        self.update(|record| record.network = Some(network.clone()))
    }

    pub fn connection_will_start(&self) -> Result<NetworkGuard, String> {
        self.network_activity.enter()
    }

    pub fn namespace_will_change(&self, namespace: &str) -> Result<NetworkGuard, String> {
        self.update(|record| record.namespace = Some(namespace.to_string()))
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
                    if let Some(network) = &record.network {
                        netdev::teardown(&record.interface, network).await;
                    }
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
mod tests {
    use super::*;

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
        let request = PrimaryImsRequest {
            device: "/dev/wwan0qmi0",
            modem: "/org/freedesktop/ModemManager1/Modem/0",
            interface: "wwan0",
            apn: "ims",
            profile_id: Some(2),
            family: 4,
            allow_roaming: false,
        };
        let properties = create_properties(&request).unwrap();
        assert_eq!(
            u32::try_from(properties.get("ip-type").unwrap()).unwrap(),
            1
        );
        assert_eq!(
            i32::try_from(properties.get("profile-id").unwrap()).unwrap(),
            2
        );
        assert!(!bool::try_from(properties.get("allow-roaming").unwrap()).unwrap());
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
