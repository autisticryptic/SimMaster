//! Explicit MM-only temporary profile maintenance, not an automatic fallback.
//! Existing profiles are fingerprints, not restore payloads: we never rewrite
//! them. An uncertain mutation leaves the receipt and refuses another create.

use super::*;
use std::collections::BTreeSet;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

const PROFILE_MANAGER: &str = "org.freedesktop.ModemManager1.Modem.Modem3gpp.ProfileManager";
const GPP: &str = "org.freedesktop.ModemManager1.Modem.Modem3gpp";
const DIRECTORY: &str = "/var/lib/simadmin/mm-ims-profile-lease";
const ERROR: &str = "mm_ims_profile_lease_unverified";
type Properties = HashMap<String, OwnedValue>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Profile {
    id: i32,
    apn: String,
    family: u32,
    name: String,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Definition {
    family: u32,
    apn: String,
    fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Snapshot {
    bus_id: String,
    owner: String,
    modem: String,
    device: String,
    sim_fingerprint: String,
    eps_fingerprint: String,
    profiles: BTreeMap<i32, Profile>,
    definitions: BTreeMap<i32, Definition>,
    reporting: BTreeMap<i32, [u8; 3]>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Creating,
    Rejected,
    Owned,
    RestoringReporting,
    Deleting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    version: u8,
    phase: Phase,
    before: Snapshot,
    requested_family: u32,
    apn: String,
    tag: String,
    owned: Option<Profile>,
    owned_definition: Option<Definition>,
}

trait ProfileIo {
    fn snapshot(&self) -> impl Future<Output = Result<Snapshot, String>> + Send;
    fn create(
        &self,
        apn: &str,
        family: u32,
        tag: &str,
    ) -> impl Future<Output = Result<Profile, String>> + Send;
    fn restore_reporting(
        &self,
        id: i32,
        flags: [u8; 3],
    ) -> impl Future<Output = Result<(), String>> + Send;
    fn delete(&self, id: i32) -> impl Future<Output = Result<(), String>> + Send;
}

trait Store {
    fn save(&self, receipt: &Receipt) -> Result<(), String>;
    fn remove(&self) -> Result<(), String>;
}

fn fingerprint(value: &impl Serialize) -> Result<String, String> {
    fn canonical(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: BTreeMap<_, _> = map.into_iter().collect();
                serde_json::Value::Object(
                    sorted
                        .into_iter()
                        .map(|(key, value)| (key, canonical(value)))
                        .collect(),
                )
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(canonical).collect())
            }
            scalar => scalar,
        }
    }
    let canonical = canonical(serde_json::to_value(value).map_err(|_| ERROR)?);
    let bytes = serde_json::to_vec(&canonical).map_err(|_| ERROR)?;
    Ok(ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn validate_request(apn: &str, family: u32) -> Result<(), String> {
    if !matches!(family, 1 | 2 | 4)
        || apn.is_empty()
        || apn.len() > 100
        || !apn
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    {
        return Err("mm_ims_profile_lease_invalid_request".into());
    }
    Ok(())
}

fn profile_tag() -> Result<String, String> {
    use ring::rand::SecureRandom;
    // A diagnostic tag is not subscriber identity. Keep it to 16 ASCII bytes
    // for older profile-name implementations; do not send a 44-byte timestamp.
    let mut random = [0_u8; 7];
    ring::rand::SystemRandom::new()
        .fill(&mut random)
        .map_err(|_| ERROR)?;
    Ok(format!(
        "sa{}",
        random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    ))
}

fn explicit_create_rejection(error: &str) -> bool {
    // Match only the completed Create Profile's parameter rejection, not a
    // timeout, owner loss, readback failure, or generic UnknownMethod. A
    // rejection is retained until an explicit release verifies no changes.
    error.starts_with(
        "qca410_primary_mm_dbus_failed:org.freedesktop.ModemManager1.Error.Core.Failed:",
    ) && error.contains("Couldn't create profile: DS profile error: invalid-parameter-length:")
        && error.contains("QMI protocol error (81): 'ExtendedInternal'")
}

fn same_binding(before: &Snapshot, after: &Snapshot) -> bool {
    before.bus_id == after.bus_id
        && before.owner == after.owner
        && before.modem == after.modem
        && before.device == after.device
        && before.sim_fingerprint == after.sim_fingerprint
        && before.eps_fingerprint == after.eps_fingerprint
}

fn unchanged_except(before: &Snapshot, after: &Snapshot, owned: i32) -> bool {
    if !same_binding(before, after)
        || before.profiles.contains_key(&owned)
        || before.definitions.contains_key(&owned)
    {
        return false;
    }
    let mut profiles = after.profiles.clone();
    profiles.remove(&owned);
    let mut definitions = after.definitions.clone();
    definitions.remove(&owned);
    let mut reporting = after.reporting.clone();
    if let Some(original) = before.reporting.get(&owned) {
        reporting.insert(owned, *original);
    } else {
        return false;
    }
    before.profiles == profiles
        && before.definitions == definitions
        && before.reporting == reporting
}

fn owned_matches(receipt: &Receipt, snapshot: &Snapshot) -> Result<i32, String> {
    let owned = receipt.owned.as_ref().ok_or(ERROR)?;
    if receipt.version != 1
        || !(2..=16).contains(&owned.id)
        || !unchanged_except(&receipt.before, snapshot, owned.id)
        || snapshot.profiles.get(&owned.id) != Some(owned)
        || snapshot.definitions.get(&owned.id) != receipt.owned_definition.as_ref()
        || !snapshot.reporting.contains_key(&owned.id)
    {
        return Err(ERROR.into());
    }
    Ok(owned.id)
}

async fn acquire_with<I: ProfileIo, S: Store>(
    io: &I,
    store: &S,
    apn: &str,
    family: u32,
    expected_plan: &str,
) -> Result<Receipt, String> {
    validate_request(apn, family)?;
    let before = io.snapshot().await?;
    if fingerprint(&(apn, family, &before))? != expected_plan {
        return Err("mm_ims_profile_lease_plan_changed".into());
    }
    // Include all profile rows, Initial EPS, SIM and reporting in both checks.
    if io.snapshot().await? != before {
        return Err(ERROR.into());
    }
    // Until the device's Delete/reporting contract is independently verified,
    // accept only default reporting on every possibly allocated absent ID.
    if before
        .reporting
        .iter()
        .any(|(id, flags)| !before.definitions.contains_key(id) && *flags != [0, 0, 0])
    {
        return Err("mm_ims_profile_lease_unused_reporting_not_default".into());
    }
    let mut receipt = Receipt {
        version: 1,
        phase: Phase::Creating,
        before,
        requested_family: family,
        apn: apn.into(),
        tag: profile_tag()?,
        owned: None,
        owned_definition: None,
    };
    store.save(&receipt)?; // durable intent BEFORE dispatch, including timeout/cancellation
    let owned = match io.create(apn, family, &receipt.tag).await {
        Ok(owned) => owned,
        Err(error) if explicit_create_rejection(&error) => {
            receipt.phase = Phase::Rejected;
            store.save(&receipt)?;
            return Err(error);
        }
        Err(error) => return Err(error),
    };
    receipt.owned = Some(owned.clone());
    store.save(&receipt)?; // known returned ID never lost, even if validation fails
    let after = io.snapshot().await?;
    if !(2..=16).contains(&owned.id)
        || owned.apn != apn
        || owned.family != family
        || owned.name != receipt.tag
        || !unchanged_except(&receipt.before, &after, owned.id)
        || after.profiles.get(&owned.id) != Some(&owned)
        || after.reporting != receipt.before.reporting
        || !after
            .definitions
            .get(&owned.id)
            .is_some_and(|definition| definition.apn == apn && definition.family == family)
        || io.snapshot().await? != after
    {
        return Err(ERROR.into());
    }
    receipt.owned_definition = after.definitions.get(&owned.id).cloned();
    receipt.phase = Phase::Owned;
    store.save(&receipt)?;
    Ok(receipt)
}

async fn release_with<I: ProfileIo, S: Store>(
    io: &I,
    store: &S,
    mut receipt: Receipt,
) -> Result<(), String> {
    if receipt.version != 1 || receipt.phase == Phase::Creating {
        return Err("mm_ims_profile_lease_creation_unresolved".into());
    }
    let snapshot = io.snapshot().await?;
    if receipt.phase == Phase::Deleting || receipt.phase == Phase::Rejected {
        // Rejected Create has no known allocated object. Only close its intent
        // if complete original state is observed twice; never issue a Delete.
        // An uncertain Delete is never repeated. Only confirm an already
        // completed deletion after the exact original state has returned.
        if snapshot == receipt.before && io.snapshot().await? == snapshot {
            return store.remove();
        }
        return Err("mm_ims_profile_lease_deletion_unresolved".into());
    }
    let id = owned_matches(&receipt, &snapshot)?;
    let original = *receipt.before.reporting.get(&id).ok_or(ERROR)?;
    let observed = *snapshot.reporting.get(&id).ok_or(ERROR)?;
    if observed != original {
        if receipt.phase == Phase::RestoringReporting || observed != [1, 1, 1] {
            return Err("mm_ims_profile_lease_reporting_unresolved".into());
        }
        receipt.phase = Phase::RestoringReporting;
        store.save(&receipt)?;
        io.restore_reporting(id, original).await?;
    }
    let before_delete = io.snapshot().await?;
    owned_matches(&receipt, &before_delete)?;
    if before_delete.reporting != receipt.before.reporting || io.snapshot().await? != before_delete
    {
        return Err(ERROR.into());
    }
    receipt.phase = Phase::Deleting;
    store.save(&receipt)?;
    io.delete(id).await?;
    if io.snapshot().await? != receipt.before || io.snapshot().await? != receipt.before {
        return Err("mm_ims_profile_lease_original_state_not_restored".into());
    }
    store.remove()
}

fn profile(properties: Properties) -> Result<Profile, String> {
    let id = properties
        .get("profile-id")
        .and_then(|v| i32::try_from(v).ok())
        .ok_or(ERROR)?;
    let apn = properties
        .get("apn")
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or("")
        .to_string();
    let family = properties
        .get("ip-type")
        .and_then(|v| u32::try_from(v).ok())
        .ok_or(ERROR)?;
    if id < 1 || !matches!(family, 1 | 2 | 4) {
        return Err(ERROR.into());
    }
    let name = properties
        .get("profile-name")
        .and_then(|v| <&str>::try_from(v).ok())
        .unwrap_or("")
        .to_string();
    Ok(Profile {
        id,
        apn,
        family,
        name,
        fingerprint: fingerprint(&properties)?,
    })
}

fn definition_snapshot(text: &str) -> Result<BTreeMap<i32, Definition>, String> {
    let mut result = BTreeMap::new();
    if text.len() > 16384 {
        return Err(ERROR.into());
    }
    for line in text.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if matches!(line, "OK" | "AT+CGDCONT?") {
            continue;
        }
        let fields = line.strip_prefix("+CGDCONT:").ok_or(ERROR)?;
        let (id, rest) = fields.split_once(',').ok_or(ERROR)?;
        let id: i32 = id.trim().parse().map_err(|_| ERROR)?;
        if rest.matches('"').count() % 2 != 0 {
            return Err(ERROR.into());
        }
        let mut fields = rest.split(',');
        let quoted = |field: Option<&str>| -> Result<String, String> {
            let value = field
                .ok_or(ERROR)?
                .trim()
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .ok_or(ERROR)?;
            if value.contains('"') {
                return Err(ERROR.into());
            }
            Ok(value.to_string())
        };
        let family = match quoted(fields.next())?.as_str() {
            "IP" => 1,
            "IPV6" => 2,
            "IPV4V6" => 4,
            _ => return Err(ERROR.into()),
        };
        let apn = quoted(fields.next())?;
        let definition = Definition {
            family,
            apn,
            fingerprint: fingerprint(&line)?,
        };
        if !(1..=16).contains(&id) || result.insert(id, definition).is_some() {
            return Err(ERROR.into());
        }
    }
    if result.is_empty() {
        return Err(ERROR.into());
    }
    Ok(result)
}

fn reporting_snapshot(text: &str) -> Result<BTreeMap<i32, [u8; 3]>, String> {
    let mut result = BTreeMap::new();
    if text.len() > 16384 {
        return Err(ERROR.into());
    }
    for line in text.lines().map(str::trim).filter(|s| !s.is_empty()) {
        if matches!(line, "OK" | "AT$QCPDPIMSCFGE?") {
            continue;
        }
        let values = line
            .strip_prefix("$QCPDPIMSCFGE:")
            .ok_or(ERROR)?
            .split(',')
            .map(|x| x.trim().parse::<u8>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ERROR)?;
        if values.len() != 4
            || !(1..=16).contains(&values[0])
            || values[1..].iter().any(|x| *x > 1)
            || result
                .insert(i32::from(values[0]), [values[1], values[2], values[3]])
                .is_some()
        {
            return Err(ERROR.into());
        }
    }
    // New ID is assigned by firmware; save every possible original reporting
    // value, not merely whichever ID we hope it will return.
    if result.keys().copied().collect::<Vec<_>>() != (1..=16).collect::<Vec<_>>() {
        return Err(ERROR.into());
    }
    Ok(result)
}

struct MmProfileIo {
    bus: Arc<MmBus>,
}

impl MmProfileIo {
    async fn quiescent(&self) -> Result<(), String> {
        self.bus.ensure_sim_binding().await?;
        if !self.bus.bearers().await?.is_empty() {
            return Err("mm_ims_profile_lease_bearers_present".into());
        }
        let calls: Vec<OwnedObjectPath> = timed(5, async {
            self.bus
                .proxy(&self.bus.modem, "org.freedesktop.ModemManager1.Modem.Voice")
                .await?
                .call("ListCalls", &())
                .await
                .map_err(bus_error)
        })
        .await?;
        if !calls.is_empty() {
            return Err("mm_ims_profile_lease_calls_present".into());
        }
        self.bus.ensure_sim_binding().await
    }

    async fn command(&self, command: &str) -> Result<String, String> {
        recovery::command_checked(&self.bus, command, || async {
            self.quiescent().await.is_ok()
        })
        .await
    }
}

impl ProfileIo for MmProfileIo {
    async fn snapshot(&self) -> Result<Snapshot, String> {
        self.quiescent().await?;
        let index: String = timed(5, async {
            self.bus
                .proxy(&self.bus.modem, PROFILE_MANAGER)
                .await?
                .get_property("IndexField")
                .await
                .map_err(bus_error)
        })
        .await?;
        if index != "profile-id" {
            return Err("mm_ims_profile_lease_index_unsupported".into());
        }
        let list: Vec<Properties> = timed(10, async {
            self.bus
                .proxy(&self.bus.modem, PROFILE_MANAGER)
                .await?
                .call("List", &())
                .await
                .map_err(bus_error)
        })
        .await?;
        let mut profiles = BTreeMap::new();
        for item in list {
            let item = profile(item)?;
            if profiles.insert(item.id, item).is_some() {
                return Err(ERROR.into());
            }
        }
        let eps: Properties = timed(5, async {
            self.bus
                .proxy(&self.bus.modem, GPP)
                .await?
                .get_property("InitialEpsBearerSettings")
                .await
                .map_err(bus_error)
        })
        .await?;
        let definitions = definition_snapshot(&self.command("AT+CGDCONT?").await?)?;
        if profiles.keys().collect::<BTreeSet<_>>() != definitions.keys().collect::<BTreeSet<_>>() {
            return Err("mm_ims_profile_lease_at_mm_inventory_mismatch".into());
        }
        let reporting = reporting_snapshot(&self.command("AT$QCPDPIMSCFGE?").await?)?;
        self.quiescent().await?;
        let sim = self.bus.read_sim_binding().await?;
        Ok(Snapshot {
            bus_id: self.bus.bus_id.clone(),
            owner: self.bus.owner.clone(),
            modem: self.bus.modem.clone(),
            device: self.bus.device.clone(),
            sim_fingerprint: fingerprint(&(sim.path, sim.id, sim.slot))?,
            eps_fingerprint: fingerprint(&eps)?,
            profiles,
            definitions,
            reporting,
        })
    }

    async fn create(&self, apn: &str, family: u32, tag: &str) -> Result<Profile, String> {
        self.quiescent().await?;
        // ProfileManager Set without its index invokes QMI Create Profile, not
        // Modify Profile. No requested ID, auth secret, APN-type or EPS writes.
        let properties = HashMap::from([
            ("apn", Value::from(apn)),
            ("ip-type", Value::from(family)),
            ("profile-name", Value::from(tag)),
            ("allowed-auth", Value::from(1_u32)),
        ]);
        let stored: Properties = timed(25, async {
            self.bus
                .proxy(&self.bus.modem, PROFILE_MANAGER)
                .await?
                .call("Set", &(properties,))
                .await
                .map_err(bus_error)
        })
        .await?;
        self.bus.ensure_sim_binding().await?;
        profile(stored)
    }

    async fn restore_reporting(&self, id: i32, flags: [u8; 3]) -> Result<(), String> {
        if !(2..=16).contains(&id) || flags.iter().any(|f| *f > 1) {
            return Err(ERROR.into());
        }
        self.command(&format!(
            "AT$QCPDPIMSCFGE={id},{},{},{}",
            flags[0], flags[1], flags[2]
        ))
        .await?;
        Ok(())
    }

    async fn delete(&self, id: i32) -> Result<(), String> {
        self.quiescent().await?;
        let properties = HashMap::from([("profile-id", Value::from(id))]);
        timed(15, async {
            self.bus
                .proxy(&self.bus.modem, PROFILE_MANAGER)
                .await?
                .call::<_, _, ()>("Delete", &(properties,))
                .await
                .map_err(bus_error)
        })
        .await?;
        self.bus.ensure_sim_binding().await
    }
}

struct DiskStore {
    file: PathBuf,
}
impl Store for DiskStore {
    fn save(&self, receipt: &Receipt) -> Result<(), String> {
        write_record(&self.file, receipt)?;
        fs::File::open(self.file.parent().ok_or(ERROR)?)
            .and_then(|f| f.sync_all())
            .map_err(|_| ERROR.to_string())
    }
    fn remove(&self) -> Result<(), String> {
        remove_record(&self.file)?;
        fs::File::open(self.file.parent().ok_or(ERROR)?)
            .and_then(|f| f.sync_all())
            .map_err(|_| ERROR.to_string())
    }
}

fn require_stopped() -> Result<(), String> {
    match fs::read_dir(STATE_DIR) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.map_err(|_| ERROR)?.path();
                if matches!(
                    path.extension().and_then(|s| s.to_str()),
                    Some("json" | "create")
                ) {
                    return Err("mm_ims_profile_lease_bearer_receipt_remaining".into());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(ERROR.into()),
    }
    // Maintenance must not race any manager process, including beta8 started
    // manually. Inspect cmdline/exe locally; never print their values.
    for item in fs::read_dir("/proc").map_err(|_| ERROR)? {
        let item = item.map_err(|_| ERROR)?;
        let Some(pid) = item
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        if let Ok(exe) = fs::read_link(item.path().join("exe")) {
            if exe
                .file_name()
                .is_some_and(|name| name.to_string_lossy().contains("simadmin"))
            {
                return Err("mm_ims_profile_lease_other_simadmin_running".into());
            }
        }
    }
    Ok(())
}

/// Root-only explicit diagnostic command. No service starts, bearer activation,
/// fallback-policy changes or persistent application configuration writes.
pub async fn maintain(
    action: &str,
    modem: &str,
    device: &str,
    family: &str,
    apn: &str,
    expected_plan: Option<&str>,
) -> Result<serde_json::Value, String> {
    if unsafe { libc::geteuid() } != 0 {
        return Err("mm_ims_profile_lease_root_required".into());
    }
    if !matches!(action, "inspect" | "acquire" | "release") {
        return Err(ERROR.into());
    }
    let family = match family {
        "ipv4" => 1,
        "ipv6" => 2,
        "ipv4v6" => 4,
        _ => return Err(ERROR.into()),
    };
    validate_request(apn, family)?;
    let Some(suffix) = modem.strip_prefix("/org/freedesktop/ModemManager1/Modem/") else {
        return Err(ERROR.into());
    };
    if suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ERROR.into());
    }
    let interface = netdev::primary_netdev_for_qmi(device).ok_or(ERROR)?;
    netdev::verify_mm_data_interface(device, &interface)?;
    require_stopped()?;
    // QMI profiles can survive a reboot; /run is not a sufficient ownership
    // ledger for this maintenance feature. A new bus/owner must retain it.
    ensure_directory(Path::new("/var/lib/simadmin"))?;
    ensure_directory(Path::new(DIRECTORY))?;
    let file = Path::new(DIRECTORY).join(format!("profile-{}.json", fingerprint(&device)?));
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(file.with_extension("lock"))
        .map_err(|_| ERROR)?;
    if lock.metadata().map_err(|_| ERROR)?.uid() != 0
        || lock.metadata().map_err(|_| ERROR)?.mode() & 0o077 != 0
        || unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0
    {
        return Err("mm_ims_profile_lease_busy_or_unsafe_lock".into());
    }
    let bus = MmBus::new(device, modem, &interface).await?;
    bus.pin_sim_binding().await?;
    let io = MmProfileIo { bus };
    let store = DiskStore { file };
    let existing = match fs::symlink_metadata(&store.file) {
        Ok(metadata) => {
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.uid() != 0
                || metadata.mode() & 0o077 != 0
                || metadata.len() > 128 * 1024
            {
                return Err(ERROR.into());
            }
            let input = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&store.file)
                .map_err(|_| ERROR)?;
            Some(
                serde_json::from_reader::<_, Receipt>(std::io::Read::take(input, 128 * 1024 + 1))
                    .map_err(|_| ERROR)?,
            )
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(ERROR.into()),
    };
    match action {
        "inspect" => {
            let snapshot = io.snapshot().await?;
            Ok(
                serde_json::json!({"action":"inspect", "plan":fingerprint(&(apn,family,&snapshot))?,
                "family":family,"profile_count":snapshot.profiles.len(),"pending_phase":existing.map(|r|r.phase),"mutated_profiles":false}),
            )
        }
        "acquire" => {
            if existing.is_some() {
                return Err("mm_ims_profile_lease_pending_receipt".into());
            }
            require_stopped()?;
            let result = acquire_with(
                &io,
                &store,
                apn,
                family,
                expected_plan.ok_or("mm_ims_profile_lease_expected_plan_required")?,
            )
            .await?;
            Ok(
                serde_json::json!({"action":"acquire","profile_id":result.owned.ok_or(ERROR)?.id,"family":family,"receipt":store.file,"existing_profiles_unchanged":true}),
            )
        }
        "release" => {
            let receipt = existing.ok_or("mm_ims_profile_lease_receipt_missing")?;
            if receipt.apn != apn || receipt.requested_family != family {
                return Err("mm_ims_profile_lease_release_selector_mismatch".into());
            }
            require_stopped()?;
            release_with(&io, &store, receipt).await?;
            Ok(serde_json::json!({"action":"release","original_state_verified":true}))
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
#[path = "primary_ims_profile_lease_dbus_tests.rs"]
mod dbus_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicI32;

    fn baseline() -> Snapshot {
        Snapshot {
            bus_id: "bus".into(),
            owner: ":1.42".into(),
            modem: "/modem/0".into(),
            device: "/dev/wwan0qmi0".into(),
            sim_fingerprint: "sim-a".into(),
            eps_fingerprint: "eps".into(),
            profiles: BTreeMap::from([(
                3,
                Profile {
                    id: 3,
                    apn: "ims".into(),
                    family: 4,
                    name: String::new(),
                    fingerprint: "original".into(),
                },
            )]),
            definitions: BTreeMap::from([(
                3,
                Definition {
                    family: 4,
                    apn: "ims".into(),
                    fingerprint: "original-row".into(),
                },
            )]),
            reporting: (1..=16).map(|id| (id, [0, 0, 0])).collect(),
        }
    }
    #[derive(Default)]
    struct MemoryStore {
        saved: Mutex<Option<Receipt>>,
        fail_save: AtomicBool,
        save_count: AtomicUsize,
        fail_at: AtomicUsize,
    }
    impl Store for MemoryStore {
        fn save(&self, receipt: &Receipt) -> Result<(), String> {
            let count = self.save_count.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_save.load(Ordering::SeqCst) || self.fail_at.load(Ordering::SeqCst) == count
            {
                return Err("disk-full".into());
            }
            *self.saved.lock().unwrap() = Some(receipt.clone());
            Ok(())
        }
        fn remove(&self) -> Result<(), String> {
            self.saved.lock().unwrap().take();
            Ok(())
        }
    }
    struct FakeIo {
        state: Mutex<Snapshot>,
        calls: Mutex<Vec<String>>,
        returned: i32,
        behavior: AtomicI32,
    }
    impl FakeIo {
        fn new(returned: i32) -> Self {
            Self {
                state: Mutex::new(baseline()),
                calls: Mutex::new(Vec::new()),
                returned,
                behavior: AtomicI32::new(0),
            }
        }
    }
    impl ProfileIo for FakeIo {
        async fn snapshot(&self) -> Result<Snapshot, String> {
            Ok(self.state.lock().unwrap().clone())
        }
        async fn create(&self, apn: &str, family: u32, tag: &str) -> Result<Profile, String> {
            self.calls.lock().unwrap().push("Set-without-id".into());
            if self.behavior.load(Ordering::SeqCst) == 1 {
                return Err("uncertain-set".into());
            }
            if self.behavior.load(Ordering::SeqCst) == 11 {
                return Err("qca410_primary_mm_dbus_failed:org.freedesktop.ModemManager1.Error.Core.Failed: Couldn't create profile: DS profile error: invalid-parameter-length: QMI protocol error (81): 'ExtendedInternal'".into());
            }
            if self.behavior.load(Ordering::SeqCst) == 2 {
                std::future::pending::<()>().await;
            }
            let p = Profile {
                id: self.returned,
                apn: apn.into(),
                family,
                name: tag.into(),
                fingerprint: "new-owned".into(),
            };
            let mut s = self.state.lock().unwrap();
            s.profiles.insert(p.id, p.clone());
            s.definitions.insert(
                p.id,
                Definition {
                    family,
                    apn: apn.into(),
                    fingerprint: "new-row".into(),
                },
            );
            if self.behavior.load(Ordering::SeqCst) == 3 {
                s.eps_fingerprint = "changed".into();
            }
            if self.behavior.load(Ordering::SeqCst) == 9 {
                s.definitions.get_mut(&p.id).unwrap().family = 2;
            }
            if self.behavior.load(Ordering::SeqCst) == 10 {
                s.definitions.get_mut(&p.id).unwrap().apn = "other".into();
            }
            Ok(p)
        }
        async fn restore_reporting(&self, id: i32, flags: [u8; 3]) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("reporting:{id}"));
            if self.behavior.load(Ordering::SeqCst) == 5 {
                return Err("reporting-timeout".into());
            }
            self.state.lock().unwrap().reporting.insert(id, flags);
            if self.behavior.load(Ordering::SeqCst) == 6 {
                return Err("reporting-reply-lost".into());
            }
            Ok(())
        }
        async fn delete(&self, id: i32) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("Delete:{id}"));
            if self.behavior.load(Ordering::SeqCst) == 4 {
                return Err("uncertain-delete".into());
            }
            let mut s = self.state.lock().unwrap();
            s.profiles.remove(&id);
            s.definitions.remove(&id);
            if self.behavior.load(Ordering::SeqCst) == 7 {
                return Err("delete-reply-lost".into());
            }
            if self.behavior.load(Ordering::SeqCst) == 8 {
                s.reporting.insert(id, [1, 0, 0]);
            }
            Ok(())
        }
    }
    async fn acquire(io: &FakeIo, store: &MemoryStore, family: u32) -> Receipt {
        let token = fingerprint(&("ims", family, io.snapshot().await.unwrap())).unwrap();
        acquire_with(io, store, "ims", family, &token)
            .await
            .unwrap()
    }

    #[test]
    fn temporary_profile_tag_is_short_ascii_and_rejection_is_narrow() {
        let tag = profile_tag().unwrap();
        assert_eq!(tag.len(), 16);
        assert!(tag.bytes().all(|b| b.is_ascii_alphanumeric()));
        let rejected = "qca410_primary_mm_dbus_failed:org.freedesktop.ModemManager1.Error.Core.Failed: Unhandled QMI protocol error (81): Couldn't create profile: DS profile error: invalid-parameter-length: QMI protocol error (81): 'ExtendedInternal'";
        assert!(explicit_create_rejection(rejected));
        for error in [
            "timeout",
            "unknownmethod",
            "invalid-parameter-length",
            "Couldn't read back profile",
            "owner_missing",
        ] {
            assert!(!explicit_create_rejection(error));
        }
    }

    #[tokio::test]
    async fn temporary_profile_rejected_intent_is_only_closed_after_original_state_verification() {
        let io = FakeIo::new(9);
        let store = MemoryStore::default();
        io.behavior.store(11, Ordering::SeqCst);
        let token = fingerprint(&("ims", 1_u32, baseline())).unwrap();
        assert!(acquire_with(&io, &store, "ims", 1, &token).await.is_err());
        let receipt = store.saved.lock().unwrap().clone().unwrap();
        assert_eq!(receipt.phase, Phase::Rejected);
        assert!(receipt.owned.is_none());
        io.state.lock().unwrap().eps_fingerprint = "changed".into();
        assert!(release_with(&io, &store, receipt.clone()).await.is_err());
        *io.state.lock().unwrap() = baseline();
        release_with(&io, &store, receipt).await.unwrap();
        assert_eq!(*io.calls.lock().unwrap(), ["Set-without-id"]);
        assert!(store.saved.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn temporary_profiles_use_returned_ids_and_preserve_existing_dual_stack_profile() {
        for family in [4, 2, 1] {
            for returned in [4, 9, 16] {
                let io = FakeIo::new(returned);
                let store = MemoryStore::default();
                let before = io.snapshot().await.unwrap();
                let r = acquire(&io, &store, family).await;
                assert_eq!(r.owned.as_ref().unwrap().id, returned);
                assert_eq!(io.state.lock().unwrap().profiles[&3], before.profiles[&3]);
                io.state
                    .lock()
                    .unwrap()
                    .reporting
                    .insert(returned, [1, 1, 1]);
                release_with(&io, &store, r).await.unwrap();
                assert_eq!(io.snapshot().await.unwrap(), before);
                assert!(store.saved.lock().unwrap().is_none());
                assert_eq!(
                    *io.calls.lock().unwrap(),
                    vec![
                        "Set-without-id".to_string(),
                        format!("reporting:{returned}"),
                        format!("Delete:{returned}")
                    ]
                );
            }
        }
    }

    #[tokio::test]
    async fn temporary_profile_intent_survives_uncertain_set_timeout_and_invalid_returned_id() {
        for behavior in [1, 2, 3] {
            let io = FakeIo::new(9);
            io.behavior.store(behavior, Ordering::SeqCst);
            let store = MemoryStore::default();
            let token = fingerprint(&("ims", 1_u32, baseline())).unwrap();
            let result = tokio::time::timeout(
                Duration::from_millis(10),
                acquire_with(&io, &store, "ims", 1, &token),
            )
            .await;
            assert!(!matches!(result, Ok(Ok(_))));
            assert_eq!(
                store.saved.lock().unwrap().as_ref().unwrap().phase,
                Phase::Creating
            );
            assert_eq!(*io.calls.lock().unwrap(), ["Set-without-id"]);
        }
        for returned in [1, 3, 17] {
            let io = FakeIo::new(returned);
            let store = MemoryStore::default();
            let token = fingerprint(&("ims", 1_u32, baseline())).unwrap();
            assert!(acquire_with(&io, &store, "ims", 1, &token).await.is_err());
            assert!(store.saved.lock().unwrap().is_some());
            assert_eq!(
                io.calls.lock().unwrap().len(),
                1,
                "never delete a returned old/unverified id"
            );
        }
    }

    #[tokio::test]
    async fn temporary_profile_plan_or_intent_failure_never_dispatches_a_write() {
        let io = FakeIo::new(9);
        let store = MemoryStore::default();
        assert!(acquire_with(&io, &store, "ims", 1, "stale").await.is_err());
        store.fail_save.store(true, Ordering::SeqCst);
        let token = fingerprint(&("ims", 1_u32, baseline())).unwrap();
        assert!(acquire_with(&io, &store, "ims", 1, &token).await.is_err());
        assert!(io.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn temporary_profile_cleanup_refuses_binding_eps_and_existing_profile_changes() {
        for change in 0..7 {
            let io = FakeIo::new(9);
            let store = MemoryStore::default();
            let r = acquire(&io, &store, 1).await;
            {
                let mut s = io.state.lock().unwrap();
                match change {
                    0 => s.owner = ":1.99".into(),
                    1 => s.sim_fingerprint = "sim-b".into(),
                    2 => s.eps_fingerprint = "new".into(),
                    3 => s.profiles.get_mut(&3).unwrap().fingerprint = "altered".into(),
                    4 => s.profiles.get_mut(&9).unwrap().fingerprint = "altered".into(),
                    5 => {
                        s.definitions.get_mut(&3).unwrap().fingerprint = "altered".into();
                    }
                    _ => {
                        s.reporting.insert(3, [1, 0, 0]);
                    }
                }
            }
            assert!(release_with(&io, &store, r).await.is_err());
            assert_eq!(
                io.calls.lock().unwrap().len(),
                1,
                "no mutation of unverified resources"
            );
            assert!(store.saved.lock().unwrap().is_some());
        }
    }

    #[tokio::test]
    async fn temporary_profile_delete_is_never_repeated_after_an_ambiguous_result() {
        let io = FakeIo::new(9);
        let store = MemoryStore::default();
        let r = acquire(&io, &store, 1).await;
        io.behavior.store(4, Ordering::SeqCst);
        assert!(release_with(&io, &store, r).await.is_err());
        let saved = store.saved.lock().unwrap().clone().unwrap();
        assert_eq!(saved.phase, Phase::Deleting);
        assert!(release_with(&io, &store, saved.clone()).await.is_err());
        assert_eq!(*io.calls.lock().unwrap(), ["Set-without-id", "Delete:9"]);
        *io.state.lock().unwrap() = baseline();
        release_with(&io, &store, saved).await.unwrap();
        assert!(store.saved.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn temporary_profile_requires_matching_at_family_apn_and_default_unused_reporting() {
        for behavior in [9, 10] {
            let io = FakeIo::new(9);
            let store = MemoryStore::default();
            io.behavior.store(behavior, Ordering::SeqCst);
            let token = fingerprint(&("ims", 1_u32, baseline())).unwrap();
            assert!(acquire_with(&io, &store, "ims", 1, &token).await.is_err());
            assert_eq!(
                store.saved.lock().unwrap().as_ref().unwrap().phase,
                Phase::Creating
            );
        }
        let io = FakeIo::new(9);
        let store = MemoryStore::default();
        io.state.lock().unwrap().reporting.insert(9, [0, 1, 0]);
        let token = fingerprint(&("ims", 1_u32, io.snapshot().await.unwrap())).unwrap();
        assert!(acquire_with(&io, &store, "ims", 1, &token).await.is_err());
        assert!(io.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn temporary_profile_preserves_each_failed_post_intent_save() {
        for fail_at in [2, 3] {
            let io = FakeIo::new(9);
            let store = MemoryStore::default();
            store.fail_at.store(fail_at, Ordering::SeqCst);
            let token = fingerprint(&("ims", 1_u32, baseline())).unwrap();
            assert!(acquire_with(&io, &store, "ims", 1, &token).await.is_err());
            assert!(store.saved.lock().unwrap().is_some());
            assert_eq!(*io.calls.lock().unwrap(), ["Set-without-id"]);
        }
        for fail_at in [4, 5] {
            let io = FakeIo::new(9);
            let store = MemoryStore::default();
            let receipt = acquire(&io, &store, 1).await;
            io.state.lock().unwrap().reporting.insert(9, [1, 1, 1]);
            store.fail_at.store(fail_at, Ordering::SeqCst);
            assert!(release_with(&io, &store, receipt).await.is_err());
            assert!(!io
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|s| s.starts_with("Delete")));
            assert!(store.saved.lock().unwrap().is_some());
        }
    }

    #[tokio::test]
    async fn temporary_profile_never_repeats_uncertain_reporting_or_delete_writes() {
        for behavior in [5, 6, 7, 8] {
            let io = FakeIo::new(9);
            let store = MemoryStore::default();
            let receipt = acquire(&io, &store, 1).await;
            if behavior <= 6 {
                io.state.lock().unwrap().reporting.insert(9, [1, 1, 1]);
            }
            io.behavior.store(behavior, Ordering::SeqCst);
            assert!(release_with(&io, &store, receipt).await.is_err());
            let retained = store.saved.lock().unwrap().clone().unwrap();
            let again = release_with(&io, &store, retained).await;
            assert_eq!(again.is_ok(), matches!(behavior, 6 | 7));
            let calls = io.calls.lock().unwrap();
            assert!(calls.iter().filter(|s| s.starts_with("reporting")).count() <= 1);
            assert!(calls.iter().filter(|s| s.starts_with("Delete")).count() <= 1);
        }
    }

    #[test]
    fn temporary_profile_snapshot_hash_is_order_independent_and_omits_secrets_from_receipt() {
        let a = HashMap::from([
            ("z", serde_json::json!({"password":"fixture-only","n":1})),
            ("a", serde_json::json!(2)),
        ]);
        let b = HashMap::from([
            ("a", serde_json::json!(2)),
            ("z", serde_json::json!({"n":1,"password":"fixture-only"})),
        ]);
        assert_eq!(fingerprint(&a).unwrap(), fingerprint(&b).unwrap());
        assert_ne!(
            fingerprint(&OwnedValue::from(1_u32)).unwrap(),
            fingerprint(&OwnedValue::from(1_i32)).unwrap()
        );
        let props = HashMap::from([
            ("profile-id".into(), OwnedValue::from(9_i32)),
            (
                "apn".into(),
                OwnedValue::try_from(Value::from("ims")).unwrap(),
            ),
            ("ip-type".into(), OwnedValue::from(1_u32)),
            (
                "password".into(),
                OwnedValue::try_from(Value::from("fixture-only")).unwrap(),
            ),
        ]);
        let p = profile(props).unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert!(!json.contains("fixture-only"));
        assert!(!json.contains("password"));
    }

    #[test]
    fn temporary_profile_reporting_and_definition_parsers_fail_closed() {
        let all = (1..=16)
            .map(|id| format!("$QCPDPIMSCFGE: {id},0,0,0"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(reporting_snapshot(&all).unwrap().len(), 16);
        assert!(reporting_snapshot("$QCPDPIMSCFGE: 1,0,0,0").is_err());
        assert!(reporting_snapshot(&(all.clone() + "\n$QCPDPIMSCFGE: 1,0,0,0")).is_err());
        assert!(reporting_snapshot(&all.replace("1,0,0,0", "1,2,0,0")).is_err());
        let row = "+CGDCONT: 3,\"IPV4V6\",\"ims\",\"0.0.0.0\",0,0";
        assert_eq!(definition_snapshot(row).unwrap().len(), 1);
        assert!(definition_snapshot(&(row.to_string() + "\n" + row)).is_err());
        assert!(definition_snapshot("ERROR").is_err());
        assert!(validate_request("ims\";AT", 1).is_err());
    }
}
