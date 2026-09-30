//! Explicit MM-only temporary profile maintenance, not an automatic fallback.
//! Existing profiles are fingerprints, not restore payloads: we never rewrite
//! them. An uncertain mutation leaves the receipt and refuses another create.

use super::*;
use crate::hardware::devices::transport::ImsBearerTransport as _;
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
    #[serde(default)]
    stable_sim_fingerprint: Option<String>,
    #[serde(default)]
    control_topology: Option<String>,
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
    Probing,
    Probed,
    RestoringReporting,
    Deleting,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CreationMethod {
    #[default]
    Qmi,
    At,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    version: u8,
    #[serde(default)]
    method: CreationMethod,
    phase: Phase,
    before: Snapshot,
    requested_family: u32,
    apn: String,
    tag: String,
    owned: Option<Profile>,
    owned_definition: Option<Definition>,
}

trait ProfileIo {
    fn method(&self) -> CreationMethod {
        CreationMethod::Qmi
    }
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
        && before.stable_sim_fingerprint == after.stable_sim_fingerprint
        && before.control_topology == after.control_topology
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
        || owned.apn != receipt.apn
        || owned.family != receipt.requested_family
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
        method: io.method(),
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
        || (receipt.method == CreationMethod::Qmi && owned.name != receipt.tag)
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
    method: CreationMethod,
    topology: fn(&str) -> Result<String, String>,
}

fn physical_control_topology(device: &str) -> Result<String, String> {
    let path = fs::canonicalize(
        Path::new("/sys/class/wwan").join(device.strip_prefix("/dev/").ok_or(ERROR)?),
    )
    .map_err(|_| "mm_ims_profile_lease_topology_unavailable")?;
    fingerprint(&path.to_string_lossy())
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
    fn method(&self) -> CreationMethod {
        self.method
    }

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
            sim_fingerprint: fingerprint(&(&sim.path, &sim.id, sim.slot))?,
            stable_sim_fingerprint: Some(fingerprint(&(&sim.id, sim.slot))?),
            control_topology: Some((self.topology)(&self.bus.device)?),
            eps_fingerprint: fingerprint(&eps)?,
            profiles,
            definitions,
            reporting,
        })
    }

    async fn create(&self, apn: &str, family: u32, tag: &str) -> Result<Profile, String> {
        self.quiescent().await?;
        if self.method == CreationMethod::At {
            return create_at_with(self, apn, family, |command| async move {
                self.command(&command).await
            })
            .await;
        }
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

/// Explicit alternative for firmware rejecting QMI Create Profile's optional
/// fields. This never activates a context and is not an automatic fallback.
/// The same capability parser as normal safe CID preparation is used, but an
/// APN match never authorizes reuse or overwrite in this maintenance path.
async fn create_at_with<I, F, Fut>(
    io: &I,
    apn: &str,
    family: u32,
    mut command: F,
) -> Result<Profile, String>
where
    I: ProfileIo,
    F: FnMut(String) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    validate_request(apn, family)?;
    let pdp_type = match family {
        1 => "IP",
        2 => "IPV6",
        4 => "IPV4V6",
        _ => return Err(ERROR.into()),
    };
    let before = io.snapshot().await?;
    let capabilities = command("AT+CGDCONT=?".into()).await?;
    let supported = crate::connectivity::modems::ims::cellular_ims::pcscf::supported_profile_cids(
        &capabilities,
        pdp_type,
    )
    .map_err(|_| "mm_ims_profile_lease_at_capabilities_invalid")?;
    let id = supported
        .into_iter()
        .map(i32::from)
        .find(|id| {
            (2..=16).contains(id)
                && !before.profiles.contains_key(id)
                && !before.definitions.contains_key(id)
        })
        .ok_or("mm_ims_profile_lease_no_unused_supported_cid")?;
    let activity = command("AT+CGACT?".into()).await?;
    verify_inactive_context(&activity, id)?;
    if io.snapshot().await? != before {
        return Err("mm_ims_profile_lease_at_inventory_changed".into());
    }
    // The explicit stopped-manager maintenance window excludes competing
    // SimAdmin writers; no selector points at an existing profile, even blank.
    command(format!("AT+CGDCONT={id},\"{pdp_type}\",\"{apn}\"")).await?;
    let after = io.snapshot().await?;
    if !unchanged_except(&before, &after, id)
        || after.reporting != before.reporting
        || !after
            .definitions
            .get(&id)
            .is_some_and(|d| d.family == family && d.apn == apn)
    {
        return Err("mm_ims_profile_lease_at_readback_mismatch".into());
    }
    let owned = after.profiles.get(&id).ok_or(ERROR)?;
    if owned.family != family || owned.apn != apn {
        return Err(ERROR.into());
    }
    Ok(owned.clone())
}

fn verify_inactive_context(text: &str, target: i32) -> Result<(), String> {
    if text.len() > 16384 {
        return Err(ERROR.into());
    }
    let mut seen = BTreeSet::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if matches!(line, "OK" | "AT+CGACT?") {
            continue;
        }
        let values = line
            .strip_prefix("+CGACT:")
            .ok_or(ERROR)?
            .split(',')
            .map(|s| s.trim().parse::<i32>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| ERROR)?;
        if values.len() != 2
            || !(1..=16).contains(&values[0])
            || !matches!(values[1], 0 | 1)
            || !seen.insert(values[0])
        {
            return Err(ERROR.into());
        }
        if values[0] == target && values[1] != 0 {
            return Err("mm_ims_profile_lease_cid_active".into());
        }
    }
    if seen.is_empty() {
        return Err(ERROR.into());
    }
    Ok(())
}

/// Persist the one-shot budget before invoking any diagnostic work. Keeping
/// this boundary independent of the live transport also lets cancellation and
/// failed persistence be tested without a modem or namespace.
async fn probe_with<I, S, F, Fut>(
    io: &I,
    store: &S,
    mut receipt: Receipt,
    apn: &str,
    family: u32,
    expected_plan: Option<&str>,
    probe: F,
) -> Result<serde_json::Value, String>
where
    I: ProfileIo,
    S: Store,
    F: FnOnce(Receipt) -> Fut,
    Fut: Future<Output = Result<serde_json::Value, String>>,
{
    if receipt.phase != Phase::Owned || receipt.apn != apn || receipt.requested_family != family {
        return Err("mm_ims_profile_probe_not_admitted".into());
    }
    let current = io.snapshot().await?;
    owned_matches(&receipt, &current)?;
    if Some(fingerprint(&(apn, family, &current))?.as_str()) != expected_plan {
        return Err("mm_ims_profile_lease_plan_changed".into());
    }
    if io.snapshot().await? != current {
        return Err(ERROR.into());
    }
    receipt.phase = Phase::Probing;
    store.save(&receipt)?; // a probe is never repeated after cancellation/crash
    let result = probe(receipt.clone()).await;
    receipt.phase = Phase::Probed;
    store.save(&receipt)?;
    result
}

/// Capability issued only from a verified Owned receipt. The diagnostic caller
/// cannot substitute a CID/APN/modem or turn a forced-family response into a
/// second activation on a mismatched profile.
pub(crate) struct VerifiedProfileProbe {
    bus: Arc<MmBus>,
    receipt: Receipt,
    used: AtomicBool,
}

impl VerifiedProfileProbe {
    pub(crate) fn cid(&self) -> u8 {
        self.receipt
            .owned
            .as_ref()
            .expect("verified owned profile")
            .id as u8
    }
    pub(crate) fn family(&self) -> u32 {
        self.receipt.requested_family
    }
    pub(crate) fn apn(&self) -> &str {
        &self.receipt.apn
    }
    pub(crate) fn label(&self) -> &str {
        &self.receipt.tag
    }
    pub(crate) fn accepts_endpoint(&self, modem: &str, device: &str) -> bool {
        device == self.bus.device
            && (modem == self.bus.modem || self.bus.modem.rsplit('/').next() == Some(modem))
    }
    pub(crate) async fn verify(&self) -> Result<(), String> {
        let io = MmProfileIo {
            bus: Arc::clone(&self.bus),
            method: self.receipt.method,
            topology: physical_control_topology,
        };
        owned_matches(&self.receipt, &io.snapshot().await?)?;
        Ok(())
    }
    pub(crate) async fn binding(
        &self,
    ) -> Result<crate::hardware::cellular::bindings::ModemBinding, String> {
        self.verify().await?;
        let bindings =
            crate::hardware::cellular::control::discover_modem_bindings(&self.bus.connection)
                .await
                .map_err(bus_error)?;
        let mut matching = bindings.into_iter().filter(|binding| {
            binding.present
                && binding.modem_path == self.bus.modem
                && binding.control_device() == Some(self.bus.device.as_str())
        });
        let binding = matching.next().ok_or(ERROR)?;
        if matching.next().is_some() {
            return Err(ERROR.into());
        }
        self.bus
            .verify_expected_sim(&binding.sim_iccid, binding.uim_slot)
            .await?;
        Ok(binding)
    }
    pub(crate) async fn serving_access(
        &self,
        binding: &crate::hardware::cellular::bindings::ModemBinding,
    ) -> Result<crate::connectivity::core::access_network::ServingAccessSnapshot, String> {
        use crate::hardware::cellular::observations::ModemObservationProvider;
        self.verify().await?;
        let observed = crate::hardware::cellular::mm_observations::ModemManagerObservations::new(
            Arc::new(self.bus.connection.clone()),
        )
        .serving_access(binding)
        .await
        .map_err(|error| error.to_string());
        self.verify().await?;
        observed
    }

    pub(crate) async fn drain_bearers(&self) -> Result<(), String> {
        shutdown_owned().await;
        // A timed-out shutdown does not authorize deleting namespace/profile.
        if !leases().lock().unwrap().is_empty() || PENDING.load(Ordering::Acquire) != 0 {
            return Err("mm_ims_profile_probe_bearer_cleanup_pending".into());
        }
        require_no_bearer_receipts()?;
        Ok(())
    }
}

impl crate::hardware::devices::transport::ImsBearerTransport for VerifiedProfileProbe {
    fn endpoint_available(&self, primary_device: &str) -> bool {
        primary_device == self.bus.device
            && crate::hardware::devices::qcm410::ims_bearer::Qcm410ImsBearer
                .endpoint_available(primary_device)
    }
    fn establish_ims_bearer<'a>(
        &'a self,
        primary_device: &'a str,
        modem_id: &'a str,
        apn: &'a str,
        profile_id: Option<u32>,
        cid: u8,
        families: &'a [u8],
        allow_roaming: bool,
        expected_mm_sim: Option<(&'a str, u8)>,
    ) -> crate::hardware::devices::transport::TransportFuture<
        'a,
        Result<
            (
                crate::hardware::devices::transport::ImsBearerInfo,
                Box<dyn crate::hardware::devices::transport::ImsBearerHandle + Send>,
            ),
            ImsBearerError,
        >,
    > {
        Box::pin(async move {
            let family_matches = probe_family_matches(self.family(), families);
            if primary_device != self.bus.device
                || !(modem_id == self.bus.modem
                    || self.bus.modem.rsplit('/').next() == Some(modem_id))
                || apn != self.apn()
                || profile_id != Some(u32::from(self.cid()))
                || cid != self.cid()
                || !family_matches
                || expected_mm_sim.is_none()
                || self.used.swap(true, Ordering::AcqRel)
            {
                return Err(session_changed("profile_probe_request_changed_or_repeated"));
            }
            self.verify()
                .await
                .map_err(|_| session_changed("profile_probe_receipt_changed"))?;
            crate::hardware::devices::qcm410::ims_bearer::establish_with_bus(
                primary_device,
                modem_id,
                apn,
                profile_id,
                cid,
                families,
                allow_roaming,
                expected_mm_sim,
                Some(Arc::clone(&self.bus)),
            )
            .await
        })
    }
}

fn probe_family_matches(family: u32, requested: &[u8]) -> bool {
    match family {
        1 => requested == [4],
        2 => requested == [6],
        4 => requested == [4, 6] || requested == [6, 4],
        _ => false,
    }
}

async fn original_modem_absent(io: &MmProfileIo, original: &str) -> Result<bool, String> {
    if !io.bus.owner_is_current().await? {
        return Err(ERROR.into());
    }
    let objects: ManagedObjects = timed(10, async {
        io.bus
            .proxy(
                "/org/freedesktop/ModemManager1",
                "org.freedesktop.DBus.ObjectManager",
            )
            .await?
            .call("GetManagedObjects", &())
            .await
            .map_err(bus_error)
    })
    .await?;
    if !io.bus.owner_is_current().await? {
        return Err(ERROR.into());
    }
    Ok(!objects.keys().any(|path| path.as_str() == original))
}

fn rebind_profile_receipt(receipt: &Receipt, current: &Snapshot) -> Result<Receipt, String> {
    let old = &receipt.before;
    if old.bus_id != current.bus_id
        || old.owner != current.owner
        || old.device != current.device
        || old.stable_sim_fingerprint.is_none()
        || old.stable_sim_fingerprint != current.stable_sim_fingerprint
        || old.control_topology.is_none()
        || old.control_topology != current.control_topology
        || old.eps_fingerprint != current.eps_fingerprint
        || !matches!(
            receipt.phase,
            Phase::Probing | Phase::Probed | Phase::Owned | Phase::RestoringReporting
        )
    {
        return Err("mm_ims_profile_lease_rebinding_unverified".into());
    }
    let mut next = receipt.clone();
    next.before.modem = current.modem.clone();
    next.before.sim_fingerprint = current.sim_fingerprint.clone();
    owned_matches(&next, current)?;
    Ok(next)
}

async fn reconcile_profile_modem<S: Store>(
    io: &MmProfileIo,
    store: &S,
    receipt: Receipt,
) -> Result<Receipt, String> {
    // This is profile-only reconciliation; never redirect any bearer cleanup.
    let original = receipt.before.modem.clone();
    if !original_modem_absent(io, &original).await? {
        return Err(ERROR.into());
    }
    let current = io.snapshot().await?;
    let next = rebind_profile_receipt(&receipt, &current)?;
    if io.snapshot().await? != current || !original_modem_absent(io, &original).await? {
        return Err(ERROR.into());
    }
    store.save(&next)?;
    Ok(next)
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

fn require_no_bearer_receipts() -> Result<(), String> {
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
    Ok(())
}

fn require_stopped() -> Result<(), String> {
    require_no_bearer_receipts()?;
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

/// Root-only explicit diagnostic command. Only `probe` activates a bearer;
/// no service starts, fallback-policy changes or persistent application
/// configuration writes.
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
    if !matches!(
        action,
        "inspect" | "acquire" | "acquire-at" | "probe" | "release"
    ) {
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
    let io = MmProfileIo {
        bus,
        topology: physical_control_topology,
        method: if action == "acquire-at" {
            CreationMethod::At
        } else {
            CreationMethod::Qmi
        },
    };
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
        "acquire" | "acquire-at" => {
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
                serde_json::json!({"action":action,"profile_id":result.owned.ok_or(ERROR)?.id,"family":family,"receipt":store.file,"existing_profiles_unchanged":true}),
            )
        }
        "probe" => {
            let receipt = existing.ok_or("mm_ims_profile_lease_receipt_missing")?;
            probe_with(
                &io,
                &store,
                receipt,
                apn,
                family,
                expected_plan,
                |receipt| async {
                    let capability = VerifiedProfileProbe {
                        bus: Arc::clone(&io.bus),
                        receipt,
                        used: AtomicBool::new(false),
                    };
                    crate::connectivity::modems::ims::cellular_ims::live::probe_owned_profile_once(
                        &capability,
                    )
                    .await
                },
            )
            .await
        }
        "release" => {
            let mut receipt = existing.ok_or("mm_ims_profile_lease_receipt_missing")?;
            if receipt.apn != apn || receipt.requested_family != family {
                return Err("mm_ims_profile_lease_release_selector_mismatch".into());
            }
            require_stopped()?;
            // The tag also names the unique probe namespace. A leftover
            // namespace (even empty) needs separate reconciliation; deleting
            // the profile ledger must not erase the last ownership reference.
            let namespace = crate::platform::netns::NetnsName::for_line(
                crate::platform::netns::DEFAULT_NAMESPACE_PREFIX,
                &format!("ims-profile-probe-{}", receipt.tag),
            );
            if crate::platform::netns::exists(&namespace) {
                return Err("mm_ims_profile_probe_namespace_remaining".into());
            }
            if receipt.before.modem != io.bus.modem {
                receipt = reconcile_profile_modem(&io, &store, receipt).await?;
            }
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
            stable_sim_fingerprint: Some("sim-id-slot".into()),
            control_topology: Some("physical-qmi-port".into()),
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
        snapshot_count: AtomicUsize,
        change_snapshot_at: AtomicUsize,
    }
    impl FakeIo {
        fn new(returned: i32) -> Self {
            Self {
                state: Mutex::new(baseline()),
                calls: Mutex::new(Vec::new()),
                returned,
                behavior: AtomicI32::new(0),
                snapshot_count: AtomicUsize::new(0),
                change_snapshot_at: AtomicUsize::new(0),
            }
        }
    }
    impl ProfileIo for FakeIo {
        async fn snapshot(&self) -> Result<Snapshot, String> {
            let count = self.snapshot_count.fetch_add(1, Ordering::SeqCst) + 1;
            let mut snapshot = self.state.lock().unwrap().clone();
            if self.change_snapshot_at.load(Ordering::SeqCst) == count {
                snapshot.eps_fingerprint = "changed-during-snapshot".into();
            }
            Ok(snapshot)
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

    #[tokio::test]
    async fn temporary_profile_at_creation_checks_capability_activity_and_snapshot_before_one_write(
    ) {
        for failure in [
            "none",
            "capability",
            "active",
            "changed",
            "write",
            "readback",
        ] {
            let io = FakeIo::new(9);
            let mut calls = Vec::new();
            let result = create_at_with(&io, "ims", 1, |command| {
                calls.push(command.clone());
                let result = match command.as_str() {
                    "AT+CGDCONT=?" if failure == "capability" => {
                        Ok("+CGDCONT: (1-16),\"IPV6\"".into())
                    }
                    "AT+CGDCONT=?" => Ok("+CGDCONT: (3,9-16),\"IP\"".into()),
                    "AT+CGACT?" => {
                        if failure == "changed" {
                            io.state.lock().unwrap().eps_fingerprint = "changed".into();
                        }
                        Ok(if failure == "active" {
                            "+CGACT: 9,1"
                        } else {
                            "+CGACT: 3,0"
                        }
                        .into())
                    }
                    "AT+CGDCONT=9,\"IP\",\"ims\"" => {
                        if failure == "write" {
                            Err("write reply lost".into())
                        } else {
                            let mut state = io.state.lock().unwrap();
                            state.profiles.insert(
                                9,
                                Profile {
                                    id: 9,
                                    family: 1,
                                    apn: "ims".into(),
                                    name: String::new(),
                                    fingerprint: "new".into(),
                                },
                            );
                            state.definitions.insert(
                                9,
                                Definition {
                                    family: if failure == "readback" { 2 } else { 1 },
                                    apn: "ims".into(),
                                    fingerprint: "row".into(),
                                },
                            );
                            Ok("OK".into())
                        }
                    }
                    _ => panic!("unexpected AT command {command}"),
                };
                std::future::ready(result)
            })
            .await;
            assert_eq!(result.is_ok(), failure == "none");
            let writes = calls
                .iter()
                .filter(|c| c.starts_with("AT+CGDCONT=") && !c.ends_with('?'))
                .count();
            assert_eq!(
                writes,
                usize::from(matches!(failure, "none" | "write" | "readback"))
            );
            assert_eq!(
                io.state.lock().unwrap().profiles[&3],
                baseline().profiles[&3]
            );
        }
    }

    #[test]
    fn temporary_profile_at_activity_requires_valid_unambiguous_observation() {
        for bad in [
            "",
            "ERROR",
            "+CGACT: 9,1",
            "+CGACT: 9,0\\n+CGACT: 9,0",
            "+CGACT: 9,2",
        ] {
            assert!(verify_inactive_context(bad, 9).is_err());
        }
        verify_inactive_context("+CGACT: 3,0\n+CGACT: 9,0", 9).unwrap();
    }

    async fn unexpected_probe(_: Receipt) -> Result<serde_json::Value, String> {
        panic!("unadmitted or repeated probe dispatched");
    }

    #[tokio::test]
    async fn profile_probe_persists_one_shot_budget_on_success_and_failure() {
        for succeeds in [true, false] {
            let io = FakeIo::new(9);
            let store = MemoryStore::default();
            let receipt = acquire(&io, &store, 1).await;
            let token = fingerprint(&("ims", 1_u32, io.snapshot().await.unwrap())).unwrap();
            let result = probe_with(&io, &store, receipt, "ims", 1, Some(&token), |admitted| {
                assert_eq!(admitted.phase, Phase::Probing);
                assert_eq!(
                    store.saved.lock().unwrap().as_ref().unwrap().phase,
                    Phase::Probing
                );
                std::future::ready(if succeeds {
                    Ok(serde_json::json!({"registered": true}))
                } else {
                    Err("mock-probe-failed".into())
                })
            })
            .await;
            assert_eq!(result.is_ok(), succeeds);
            let saved = store.saved.lock().unwrap().clone().unwrap();
            assert_eq!(saved.phase, Phase::Probed);
            for phase in [
                Phase::Creating,
                Phase::Rejected,
                Phase::Probing,
                Phase::Probed,
                Phase::RestoringReporting,
                Phase::Deleting,
            ] {
                let mut replay = saved.clone();
                replay.phase = phase;
                assert!(probe_with(
                    &io,
                    &store,
                    replay,
                    "ims",
                    1,
                    Some(&token),
                    unexpected_probe,
                )
                .await
                .is_err());
            }
            release_with(&io, &store, saved).await.unwrap();
            assert_eq!(*io.calls.lock().unwrap(), ["Set-without-id", "Delete:9"]);
            assert!(store.saved.lock().unwrap().is_none());
        }
    }

    #[tokio::test]
    async fn profile_probe_rejects_stale_plan_selectors_and_inconsistent_owned_receipt() {
        for change in 0..9 {
            let io = FakeIo::new(9);
            let store = MemoryStore::default();
            let mut receipt = acquire(&io, &store, 1).await;
            let mut apn = "ims";
            let mut family = 1;
            match change {
                0 | 1 | 8 => {}
                2 => apn = "other",
                3 => family = 2,
                4 => {
                    // Even a matching selector/token must not contradict the
                    // family of the profile that was actually acquired.
                    receipt.requested_family = 2;
                    family = 2;
                }
                5 => {
                    receipt.apn = "other".into();
                    apn = "other";
                }
                6 => {
                    io.state
                        .lock()
                        .unwrap()
                        .profiles
                        .get_mut(&9)
                        .unwrap()
                        .fingerprint = "changed".into();
                }
                _ => io.state.lock().unwrap().owner = ":1.99".into(),
            }
            let token = fingerprint(&(apn, family, io.snapshot().await.unwrap())).unwrap();
            let plan = match change {
                0 => None,
                1 => Some("stale"),
                _ => Some(token.as_str()),
            };
            if change == 8 {
                io.change_snapshot_at.store(
                    io.snapshot_count.load(Ordering::SeqCst) + 2,
                    Ordering::SeqCst,
                );
            }
            assert!(
                probe_with(&io, &store, receipt, apn, family, plan, unexpected_probe,)
                    .await
                    .is_err()
            );
            assert_eq!(store.save_count.load(Ordering::SeqCst), 3);
            assert_eq!(*io.calls.lock().unwrap(), ["Set-without-id"]);
        }
    }

    #[tokio::test]
    async fn profile_probe_cancellation_and_failed_persistence_never_replenish_budget() {
        for failure in ["intent", "completion", "cancelled"] {
            let io = FakeIo::new(9);
            let store = MemoryStore::default();
            let receipt = acquire(&io, &store, 1).await;
            let token = fingerprint(&("ims", 1_u32, io.snapshot().await.unwrap())).unwrap();
            if failure != "cancelled" {
                store
                    .fail_at
                    .store(if failure == "intent" { 4 } else { 5 }, Ordering::SeqCst);
            }
            let dispatched = AtomicUsize::new(0);
            let result = tokio::time::timeout(
                Duration::from_millis(10),
                probe_with(&io, &store, receipt, "ims", 1, Some(&token), |_| async {
                    dispatched.fetch_add(1, Ordering::SeqCst);
                    if failure == "cancelled" {
                        std::future::pending::<()>().await;
                    }
                    Ok(serde_json::json!({"registered": false}))
                }),
            )
            .await;
            assert!(!matches!(result, Ok(Ok(_))));
            assert_eq!(
                dispatched.load(Ordering::SeqCst),
                usize::from(failure != "intent")
            );
            let saved = store.saved.lock().unwrap().clone().unwrap();
            if failure == "intent" {
                assert_eq!(saved.phase, Phase::Owned);
            } else {
                assert_eq!(saved.phase, Phase::Probing);
                assert!(
                    probe_with(&io, &store, saved, "ims", 1, Some(&token), unexpected_probe,)
                        .await
                        .is_err()
                );
            }
            assert_eq!(*io.calls.lock().unwrap(), ["Set-without-id"]);
        }
    }

    #[tokio::test]
    async fn profile_probe_rebind_preserves_phase_and_never_repeats_uncertain_reporting() {
        for phase in [
            Phase::Owned,
            Phase::Probing,
            Phase::Probed,
            Phase::RestoringReporting,
        ] {
            for reporting in [[0, 0, 0], [1, 1, 1]] {
                let io = FakeIo::new(9);
                let store = MemoryStore::default();
                let mut receipt = acquire(&io, &store, 1).await;
                receipt.phase = phase.clone();
                {
                    let mut current = io.state.lock().unwrap();
                    current.modem = "/modem/1".into();
                    current.sim_fingerprint = "same-card-new-sim-path".into();
                    current.reporting.insert(9, reporting);
                }
                let reconciled =
                    rebind_profile_receipt(&receipt, &io.snapshot().await.unwrap()).unwrap();
                assert_eq!(reconciled.phase, phase);
                store.save(&reconciled).unwrap();
                let result = release_with(&io, &store, reconciled).await;
                if phase == Phase::RestoringReporting && reporting == [1, 1, 1] {
                    assert!(result.is_err());
                    assert_eq!(*io.calls.lock().unwrap(), ["Set-without-id"]);
                    assert!(store.saved.lock().unwrap().is_some());
                } else {
                    result.unwrap();
                    assert_eq!(io.calls.lock().unwrap().last().unwrap(), "Delete:9");
                    assert!(store.saved.lock().unwrap().is_none());
                    let mut restored = receipt.before;
                    restored.modem = "/modem/1".into();
                    restored.sim_fingerprint = "same-card-new-sim-path".into();
                    assert_eq!(io.snapshot().await.unwrap(), restored);
                }
            }
        }
    }

    #[test]
    fn profile_probe_family_guard_never_turns_a_single_profile_into_a_different_attempt() {
        assert!(probe_family_matches(4, &[6, 4]));
        assert!(probe_family_matches(4, &[4, 6]));
        assert!(probe_family_matches(1, &[4]));
        assert!(probe_family_matches(2, &[6]));
        for (family, requested) in [
            (4, vec![4]),
            (4, vec![6]),
            (1, vec![6]),
            (2, vec![4]),
            (1, vec![4, 6]),
            (4, vec![4, 4]),
        ] {
            assert!(!probe_family_matches(family, &requested));
        }
    }

    #[tokio::test]
    async fn profile_probe_reconciliation_requires_stable_owner_sim_topology_and_exact_profile() {
        let io = FakeIo::new(9);
        let store = MemoryStore::default();
        let receipt = acquire(&io, &store, 1).await;
        let mut current = io.snapshot().await.unwrap();
        current.modem = "/modem/1".into();
        current.sim_fingerprint = "new-sim-object-same-card".into();
        let reconciled = rebind_profile_receipt(&receipt, &current).unwrap();
        assert_eq!(reconciled.before.modem, current.modem);
        assert_eq!(
            reconciled.before.stable_sim_fingerprint,
            receipt.before.stable_sim_fingerprint
        );
        for kind in 0..12 {
            let mut bad = current.clone();
            match kind {
                0 => bad.owner = ":1.43".into(),
                1 => bad.stable_sim_fingerprint = Some("different-card".into()),
                2 => bad.control_topology = Some("different-device".into()),
                3 => bad.eps_fingerprint = "changed".into(),
                4 => bad.profiles.get_mut(&9).unwrap().fingerprint = "reused-id".into(),
                5 => bad.profiles.get_mut(&3).unwrap().fingerprint = "other-changed".into(),
                6 => bad.bus_id = "new-boot".into(),
                7 => bad.device = "/dev/wwan1qmi0".into(),
                8 => bad.definitions.get_mut(&9).unwrap().fingerprint = "changed-at-row".into(),
                9 => {
                    bad.reporting.insert(3, [1, 1, 1]);
                }
                10 => bad.stable_sim_fingerprint = None,
                _ => bad.control_topology = None,
            }
            assert!(rebind_profile_receipt(&receipt, &bad).is_err());
        }
        for phase in [Phase::Creating, Phase::Rejected, Phase::Deleting] {
            let mut unresolved = receipt.clone();
            unresolved.phase = phase;
            assert!(rebind_profile_receipt(&unresolved, &current).is_err());
        }
        for missing_sim in [true, false] {
            let mut legacy = receipt.clone();
            if missing_sim {
                legacy.before.stable_sim_fingerprint = None;
            } else {
                legacy.before.control_topology = None;
            }
            assert!(rebind_profile_receipt(&legacy, &current).is_err());
        }
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
