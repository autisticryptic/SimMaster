//! One-shot MM recovery for a verified IMS context with reporting disabled.
//!
//! This is not a second bearer backend, P-CSCF source, or IP-family policy.
//! Inspection uses the in-process owned lease and its original unique MM owner.
//! The caller must release that lease and hold line/bearer/registration gates
//! before execution. A durable per-MM-owner/device/SIM budget survives retries
//! and application restarts; no automatic reset/restore loop is permitted.

use std::{fs::OpenOptions, future::Future, io::Write, path::Path, sync::Arc, time::Duration};
use tokio::time::Instant;
use zbus::zvariant::OwnedObjectPath;

use super::{
    ensure_directory, leases, primary_ims_pcscf, primary_ims_settings, timed, MmBus, OwnedLease,
    BEARER, MODEM,
};
use crate::hardware::cellular::serial;
use primary_ims_settings::MmIpFamily;

const GPP: &str = "org.freedesktop.ModemManager1.Modem.Modem3gpp";
const BUDGET_DIR: &str = "/run/simadmin/mm-pcscf-recovery";

/// A typed plan only constructible after checking an owned, exclusive MM bearer.
/// SIM identity and raw definition snapshots stay private and are never logged.
pub struct RecoveryPlan {
    lease: Arc<OwnedLease>,
    bus: Arc<MmBus>,
    cid: u8,
    definitions: String,
    sim_path: String,
    sim_id: String,
    eps: serde_json::Value,
}

fn definition_rows(text: &str) -> Vec<String> {
    let mut rows: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with("+CGDCONT:"))
        .map(str::to_string)
        .collect();
    rows.sort();
    rows
}

fn reporting_flags(text: &str, cid: u8) -> Result<[u8; 3], String> {
    let mut result = None;
    let mut seen = std::collections::BTreeSet::new();
    if text.len() > 16384 {
        return Err("mm_pcscf_reporting_limit".into());
    }
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        if matches!(line, "OK" | "AT$QCPDPIMSCFGE?") {
            continue;
        }
        let rest = line
            .strip_prefix("$QCPDPIMSCFGE:")
            .ok_or("mm_pcscf_reporting_invalid")?;
        let values = rest
            .split(',')
            .map(|x| x.trim().parse::<u8>())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "mm_pcscf_reporting_invalid")?;
        if values.len() != 4
            || !(1..=16).contains(&values[0])
            || values[1..].iter().any(|value| *value > 1)
            || !seen.insert(values[0])
        {
            return Err("mm_pcscf_reporting_invalid".into());
        }
        if values[0] == cid {
            result = Some([values[1], values[2], values[3]]);
        }
    }
    result.ok_or_else(|| "mm_pcscf_reporting_absent".into())
}

async fn command(bus: &MmBus, command: &str) -> Result<String, String> {
    command_checked(bus, command, || std::future::ready(true)).await
}

pub(super) async fn command_checked<Allowed, AllowedFuture>(
    bus: &MmBus,
    command: &str,
    allowed: Allowed,
) -> Result<String, String>
where
    Allowed: Fn() -> AllowedFuture,
    AllowedFuture: Future<Output = bool>,
{
    serial::with_serial_for(&bus.modem, async {
        // The serial wait can span a SIM change with the same MM owner.
        // Validate the retained slot/SIM and live policy inside the permit.
        bus.ensure_sim_binding().await?;
        if !allowed().await {
            return Err("mm_pcscf_recovery_cancelled".into());
        }
        let result = timed(6, async {
            bus.proxy(&bus.modem, MODEM)
                .await?
                .call::<_, _, String>("Command", &(command, 4_u32))
                .await
                .map_err(|_| "mm_pcscf_command_failed".to_string())
        })
        .await?;
        bus.ensure_sim_binding().await?;
        if !allowed().await {
            return Err("mm_pcscf_recovery_cancelled".into());
        }
        if result.len() > 16384 {
            return Err("mm_pcscf_response_limit".into());
        }
        Ok(result)
    })
    .await
}

async fn sim_identity(bus: &MmBus) -> Result<(String, String), String> {
    timed(5, async {
        let path: OwnedObjectPath = bus
            .proxy(&bus.modem, MODEM)
            .await?
            .get_property("Sim")
            .await
            .map_err(|_| "mm_pcscf_sim_unavailable")?;
        let id: String = bus
            .proxy(path.as_str(), "org.freedesktop.ModemManager1.Sim")
            .await?
            .get_property("SimIdentifier")
            .await
            .map_err(|_| "mm_pcscf_sim_unavailable")?;
        if id.is_empty() {
            return Err("mm_pcscf_sim_unavailable".into());
        }
        Ok((path.to_string(), id))
    })
    .await
}

async fn eps_settings(bus: &MmBus) -> Result<serde_json::Value, String> {
    timed(5, async {
        let settings: primary_ims_settings::Properties = bus
            .proxy(&bus.modem, GPP)
            .await?
            .get_property("InitialEpsBearerSettings")
            .await
            .map_err(|_| "mm_pcscf_eps_unavailable")?;
        serde_json::to_value(settings).map_err(|_| "mm_pcscf_eps_invalid".into())
    })
    .await
}

impl RecoveryPlan {
    /// MM-only: no lease means no recovery. We never create an owner from a
    /// cached modem path and never inspect/modify a native provider's session.
    pub async fn inspect(modem: &str) -> Result<Option<Self>, String> {
        let candidates: Vec<_> = leases()
            .lock()
            .unwrap()
            .values()
            .filter(|lease| lease.bus.modem == modem && !lease.is_done())
            .cloned()
            .collect();
        let [lease] = candidates.as_slice() else {
            return Ok(None);
        };
        if lease.is_closing() {
            return Ok(None);
        }
        let _guard = lease.connection_will_start()?;
        let bus = Arc::clone(&lease.bus);
        if !bus.owner_is_current().await? {
            return Ok(None);
        }
        let bearer = lease.path();
        if bus.bearers().await? != [bearer.clone()] {
            return Ok(None);
        }
        let status = bus.status(&bearer).await?;
        if !status.connected || status.interface != bus.data_interface() {
            return Ok(None);
        }
        let properties: primary_ims_settings::Properties = timed(5, async {
            bus.proxy(&bearer, "org.freedesktop.DBus.Properties")
                .await?
                .call("GetAll", &(BEARER,))
                .await
                .map_err(|_| "mm_pcscf_bearer_unavailable".to_string())
        })
        .await?;
        let profile_id = primary_ims_settings::profile_id(&properties)?;
        let Some(expected) = bus
            .ip_settings(&bearer, &status.apn, MmIpFamily::Ipv4v6)
            .await?
        else {
            return Ok(None);
        };
        let definitions = command(&bus, "AT+CGDCONT?").await?;
        let activity = command(&bus, "AT+CGACT?").await?;
        let Some(cid) = primary_ims_pcscf::sole_active_context(&activity, &definitions)
            .map_err(|_| "mm_pcscf_context_invalid")?
        else {
            return Ok(None);
        };
        let row = command(&bus, &format!("AT+CGCONTRDP={cid}")).await?;
        if primary_ims_pcscf::missing_reporting_context(
            &expected,
            profile_id,
            &status.apn,
            &activity,
            &definitions,
            &row,
        )
        .map_err(|_| "mm_pcscf_context_unassociated")?
            != Some(cid)
        {
            return Ok(None);
        }
        if reporting_flags(&command(&bus, "AT$QCPDPIMSCFGE?").await?, cid)? != [0, 0, 0] {
            return Ok(None);
        }
        let (sim_path, sim_id) = sim_identity(&bus).await?;
        let eps = eps_settings(&bus).await?;
        if command(&bus, "AT+CGACT?").await? != activity
            || command(&bus, &format!("AT+CGCONTRDP={cid}")).await? != row
            || definition_rows(&command(&bus, "AT+CGDCONT?").await?)
                != definition_rows(&definitions)
            || bus
                .pcscf_binding_snapshot(&bearer, &status.apn, MmIpFamily::Ipv4v6, profile_id)
                .await
                .map_err(|_| "mm_pcscf_bearer_changed")?
                != expected
        {
            return Err("mm_pcscf_observation_changed".into());
        }
        Ok(Some(Self {
            lease: Arc::clone(lease),
            bus,
            cid,
            definitions,
            sim_path,
            sim_id,
            eps,
        }))
    }

    pub fn context_id(&self) -> u8 {
        self.cid
    }

    pub fn budget_available(&self) -> bool {
        !budget_path(
            &self.bus.bus_id,
            &self.bus.owner,
            &self.bus.device,
            &self.sim_id,
        )
        .exists()
    }

    async fn identity_is_current(&self) -> Result<(), String> {
        self.bus.ensure_sim_binding().await?;
        if !self.bus.owner_is_current().await?
            || self.bus.primary_port().await? != self.bus.device.trim_start_matches("/dev/")
        {
            return Err("mm_pcscf_owner_or_endpoint_changed".into());
        }
        let (path, id) = sim_identity(&self.bus).await?;
        if path != self.sim_path || id != self.sim_id {
            return Err("mm_pcscf_sim_changed".into());
        }
        Ok(())
    }

    async fn released_and_unchanged(&self) -> Result<(), String> {
        self.identity_is_current().await?;
        if !self.lease.is_done() || !self.bus.bearers().await?.is_empty() {
            return Err("mm_pcscf_bearers_not_released".into());
        }
        if definition_rows(&command(&self.bus, "AT+CGDCONT?").await?)
            != definition_rows(&self.definitions)
            || eps_settings(&self.bus).await? != self.eps
        {
            return Err("mm_pcscf_profile_or_eps_changed".into());
        }
        Ok(())
    }

    /// Caller holds all lifecycle gates in a shielded task and rechecks its
    /// live policy (calls, data intent, generation, enabled flags) between steps.
    /// A consumed budget is intentionally never erased, including on failure.
    pub async fn execute<Allowed, AllowedFuture, Restore, RestoreFuture>(
        &self,
        allowed: Allowed,
        restore_radio: Restore,
    ) -> Result<(), String>
    where
        Allowed: Fn() -> AllowedFuture,
        AllowedFuture: Future<Output = bool>,
        Restore: Fn() -> RestoreFuture,
        RestoreFuture: Future<Output = bool>,
    {
        self.released_and_unchanged().await?;
        if !allowed().await {
            return Err("mm_pcscf_recovery_cancelled".into());
        }
        if reporting_flags(&command(&self.bus, "AT$QCPDPIMSCFGE?").await?, self.cid)? != [0, 0, 0] {
            return Err("mm_pcscf_reporting_changed".into());
        }
        // Never turn on a modem that was already disabled by another actor.
        let state: i32 = timed(5, async {
            self.bus
                .proxy(&self.bus.modem, MODEM)
                .await?
                .get_property("State")
                .await
                .map_err(|_| "mm_pcscf_state_unavailable".to_string())
        })
        .await?;
        if !matches!(state, 6 | 7 | 8 | 10 | 11) {
            return Err("mm_pcscf_radio_not_enabled".into());
        }
        ensure_directory(Path::new(BUDGET_DIR))?;
        claim_budget(&budget_path(
            &self.bus.bus_id,
            &self.bus.owner,
            &self.bus.device,
            &self.sim_id,
        ))?;
        run_cycle_with(
            |step| {
                let allowed = &allowed;
                let restore_radio = &restore_radio;
                async move {
                    self.identity_is_current().await?;
                    // The final Enable is compensation for our own Disable. An IMS
                    // generation/data/call change stops work, not radio restoration.
                    // Explicit airplane mode, a replacement owner or SIM still wins.
                    let permitted = if step == Step::Enable {
                        restore_radio().await
                    } else {
                        allowed().await
                    };
                    if !permitted {
                        return Err("mm_pcscf_recovery_cancelled".into());
                    }
                    match step {
                        Step::Arm => {
                            self.released_and_unchanged().await?;
                            command_checked(
                                &self.bus,
                                &format!("AT$QCPDPIMSCFGE={},1,1,1", self.cid),
                                allowed,
                            )
                            .await?;
                            if reporting_flags(
                                &command(&self.bus, "AT$QCPDPIMSCFGE?").await?,
                                self.cid,
                            )? != [1, 1, 1]
                            {
                                return Err("mm_pcscf_reporting_not_confirmed".into());
                            }
                        }
                        Step::Disable | Step::Enable => {
                            if step == Step::Disable {
                                self.released_and_unchanged().await?;
                                // Recheck MM itself after inspection and the firmware
                                // cleanup grace period, not only the app call cache.
                                let calls: Vec<OwnedObjectPath> = timed(5, async {
                                    self.bus
                                        .proxy(
                                            &self.bus.modem,
                                            "org.freedesktop.ModemManager1.Modem.Voice",
                                        )
                                        .await?
                                        .call("ListCalls", &())
                                        .await
                                        .map_err(|_| "mm_pcscf_calls_unavailable".to_string())
                                })
                                .await?;
                                if !calls.is_empty() {
                                    return Err("mm_pcscf_calls_active".into());
                                }
                                self.identity_is_current().await?;
                                if !allowed().await {
                                    return Err("mm_pcscf_recovery_cancelled".into());
                                }
                            }
                            timed(60, async {
                                self.bus
                                    .proxy(&self.bus.modem, MODEM)
                                    .await?
                                    .call::<_, _, ()>("Enable", &(step == Step::Enable,))
                                    .await
                                    .map_err(|_| "mm_pcscf_enable_failed".to_string())
                            })
                            .await?;
                        }
                        Step::Low => {
                            wait_for_state(
                                &self.bus,
                                |state| state == 3,
                                Duration::from_secs(50),
                                || async {
                                    allowed().await && self.identity_is_current().await.is_ok()
                                },
                            )
                            .await?;
                            self.identity_is_current().await?;
                            if !allowed().await {
                                return Err("mm_pcscf_recovery_cancelled".into());
                            }
                            timed(15, async {
                                self.bus
                                    .proxy(&self.bus.modem, MODEM)
                                    .await?
                                    .call::<_, _, ()>("SetPowerState", &(2_u32,))
                                    .await
                                    .map_err(|_| "mm_pcscf_low_power_failed".to_string())
                            })
                            .await?;
                            tokio::time::sleep(Duration::from_secs(3)).await;
                        }
                        Step::WaitRegistered => {
                            // MM REGISTERED is 8, not 9 (DISCONNECTING). Do not require
                            // an application bearer before starting the IMS attempt.
                            wait_for_state(
                                &self.bus,
                                registered_state,
                                Duration::from_secs(180),
                                || async {
                                    allowed().await && self.identity_is_current().await.is_ok()
                                },
                            )
                            .await?;
                            self.released_and_unchanged().await?;
                        }
                    }
                    self.identity_is_current().await
                }
            },
            &allowed,
            &restore_radio,
        )
        .await
    }
}

fn registered_state(state: i32) -> bool {
    matches!(state, 8 | 10 | 11)
}

async fn wait_for_state<Allowed, AllowedFuture>(
    bus: &MmBus,
    ready: impl Fn(i32) -> bool,
    budget: Duration,
    allowed: Allowed,
) -> Result<(), String>
where
    Allowed: Fn() -> AllowedFuture,
    AllowedFuture: Future<Output = bool>,
{
    wait_for_state_with(ready, budget, allowed, || async {
        if !bus.owner_is_current().await? {
            return Err("mm_pcscf_owner_changed".into());
        }
        timed(5, async {
            bus.proxy(&bus.modem, MODEM)
                .await?
                .get_property("State")
                .await
                .map_err(|_| "mm_pcscf_state_unavailable".to_string())
        })
        .await
    })
    .await
}

async fn wait_for_state_with<Allowed, AllowedFuture, Read, ReadFuture>(
    ready: impl Fn(i32) -> bool,
    budget: Duration,
    allowed: Allowed,
    read: Read,
) -> Result<(), String>
where
    Allowed: Fn() -> AllowedFuture,
    AllowedFuture: Future<Output = bool>,
    Read: Fn() -> ReadFuture,
    ReadFuture: Future<Output = Result<i32, String>>,
{
    let deadline = Instant::now() + budget;
    loop {
        if !allowed().await {
            return Err("mm_pcscf_recovery_cancelled".into());
        }
        let state = read().await?;
        if !allowed().await {
            return Err("mm_pcscf_recovery_cancelled".into());
        }
        if ready(state) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("mm_pcscf_registration_wait_expired".into());
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Arm,
    Disable,
    Low,
    Enable,
    WaitRegistered,
}

async fn run_cycle_with<Run, Fut, Allowed, AllowedFuture, Restore, RestoreFuture>(
    mut run: Run,
    allowed: Allowed,
    restore_radio: Restore,
) -> Result<(), String>
where
    Run: FnMut(Step) -> Fut,
    Fut: Future<Output = Result<(), String>>,
    Allowed: Fn() -> AllowedFuture,
    AllowedFuture: Future<Output = bool>,
    Restore: Fn() -> RestoreFuture,
    RestoreFuture: Future<Output = bool>,
{
    let cancelled = || Err("mm_pcscf_recovery_cancelled".to_string());
    if !allowed().await {
        return cancelled();
    }
    run(Step::Arm).await?;
    // No Disable dispatched yet: cancellation needs no radio compensation.
    if !allowed().await {
        return cancelled();
    }
    // Disabling may succeed even if its acknowledgement is lost. Once sent,
    // restore once under radio intent, independently of ordinary IMS cancellation.
    let down = match run(Step::Disable).await {
        Ok(()) if allowed().await => run(Step::Low).await,
        Ok(()) => cancelled(),
        Err(error) => Err(error),
    };
    let enabled = if restore_radio().await {
        run(Step::Enable).await
    } else {
        cancelled()
    };
    down?;
    enabled?;
    if !allowed().await {
        return cancelled();
    }
    run(Step::WaitRegistered).await
}

fn budget_path(bus_id: &str, owner: &str, device: &str, sim: &str) -> std::path::PathBuf {
    let input = format!("{bus_id}\0{owner}\0{device}\0{sim}");
    let hash = ring::digest::digest(&ring::digest::SHA256, input.as_bytes());
    let key: String = hash
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Path::new(BUDGET_DIR).join(format!("{key}.attempted"))
}

fn claim_budget(path: &Path) -> Result<(), String> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "mm_pcscf_recovery_budget_unavailable")?;
    file.write_all(b"one-shot MM P-CSCF recovery attempted\n")
        .and_then(|_| file.sync_all())
        .map_err(|_| "mm_pcscf_recovery_budget_unconfirmed".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::ready;

    #[test]
    fn reporting_parser_requires_one_complete_target_row() {
        assert_eq!(
            reporting_flags("$QCPDPIMSCFGE: 1,0,0,0\n$QCPDPIMSCFGE: 3,1,1,1\nOK", 1).unwrap(),
            [0, 0, 0]
        );
        for text in [
            "",
            "ERROR",
            "$QCPDPIMSCFGE: 1,0,0",
            "$QCPDPIMSCFGE: 1,2,0,0",
            "$QCPDPIMSCFGE: 3,0,0,0",
            "$QCPDPIMSCFGE: 1,0,0,0\n$QCPDPIMSCFGE: 1,0,0,0",
        ] {
            assert!(reporting_flags(text, 1).is_err(), "{text}");
        }
    }

    #[test]
    fn mm_registered_does_not_require_an_application_bearer() {
        assert!(registered_state(8));
        for state in [-1, 0, 1, 2, 3, 4, 5, 6, 7, 9, 12] {
            assert!(!registered_state(state));
        }
        assert!(registered_state(10));
        assert!(registered_state(11));
    }

    #[tokio::test]
    async fn one_cycle_arms_before_detach_and_never_repeats_a_radio_step() {
        let mut steps = Vec::new();
        run_cycle_with(
            |step| {
                steps.push(step);
                ready(Ok(()))
            },
            || ready(true),
            || ready(true),
        )
        .await
        .unwrap();
        assert_eq!(
            steps,
            vec![
                Step::Arm,
                Step::Disable,
                Step::Low,
                Step::Enable,
                Step::WaitRegistered
            ]
        );
    }

    #[tokio::test]
    async fn failure_restores_enable_once_without_a_second_detach() {
        for fail in [
            Step::Arm,
            Step::Disable,
            Step::Low,
            Step::Enable,
            Step::WaitRegistered,
        ] {
            let mut steps = Vec::new();
            let result = run_cycle_with(
                |step| {
                    steps.push(step);
                    ready(if step == fail {
                        Err("failed".into())
                    } else {
                        Ok(())
                    })
                },
                || ready(true),
                || ready(true),
            )
            .await;
            assert!(result.is_err());
            assert!(steps.iter().filter(|step| **step == Step::Disable).count() <= 1);
            assert!(steps.iter().filter(|step| **step == Step::Low).count() <= 1);
            assert_eq!(
                steps.iter().filter(|step| **step == Step::Enable).count(),
                usize::from(fail != Step::Arm)
            );
            if fail == Step::Arm {
                assert_eq!(steps, vec![Step::Arm]);
            }
        }
    }

    #[tokio::test]
    async fn cancellation_after_disable_restores_radio_but_does_not_continue() {
        use std::cell::Cell;
        for cancel_at in [Step::Arm, Step::Disable, Step::Low] {
            let permitted = Cell::new(true);
            let mut sent = Vec::new();
            let result = run_cycle_with(
                |step| {
                    sent.push(step);
                    if step == cancel_at {
                        permitted.set(false);
                    }
                    ready(Ok(()))
                },
                || ready(permitted.get()),
                || ready(true),
            )
            .await;
            assert!(result.is_err());
            assert_eq!(
                sent.iter().filter(|step| **step == Step::Enable).count(),
                usize::from(cancel_at != Step::Arm)
            );
            assert!(!sent.contains(&Step::WaitRegistered));
            if cancel_at == Step::Disable {
                assert!(!sent.contains(&Step::Low));
            }
        }
    }

    #[tokio::test]
    async fn explicit_radio_off_or_replacement_blocks_compensating_enable() {
        use std::cell::Cell;
        let radio_allowed = Cell::new(true);
        let mut sent = Vec::new();
        let result = run_cycle_with(
            |step| {
                sent.push(step);
                if step == Step::Disable {
                    radio_allowed.set(false);
                }
                ready(Ok(()))
            },
            || ready(radio_allowed.get()),
            || ready(radio_allowed.get()),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(sent, vec![Step::Arm, Step::Disable]);
    }

    #[tokio::test]
    async fn state_wait_checks_cancellation_before_and_after_io() {
        use std::cell::Cell;
        let reads = Cell::new(0);
        let result = wait_for_state_with(
            registered_state,
            Duration::from_secs(180),
            || ready(false),
            || {
                reads.set(reads.get() + 1);
                ready(Ok(8))
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "mm_pcscf_recovery_cancelled");
        assert_eq!(reads.get(), 0);
        let current = Cell::new(true);
        let result = wait_for_state_with(
            registered_state,
            Duration::from_secs(180),
            || ready(current.get()),
            || {
                current.set(false);
                ready(Ok(8))
            },
        )
        .await;
        assert_eq!(result.unwrap_err(), "mm_pcscf_recovery_cancelled");
    }

    #[tokio::test]
    async fn state_wait_accepts_registered_and_bounds_unregistered_or_failed_io() {
        assert!(wait_for_state_with(
            registered_state,
            Duration::ZERO,
            || ready(true),
            || ready(Ok(8))
        )
        .await
        .is_ok());
        assert_eq!(
            wait_for_state_with(
                registered_state,
                Duration::ZERO,
                || ready(true),
                || ready(Ok(7))
            )
            .await
            .unwrap_err(),
            "mm_pcscf_registration_wait_expired"
        );
        assert_eq!(
            wait_for_state_with(
                registered_state,
                Duration::ZERO,
                || ready(true),
                || ready(Err("read_failed".into()))
            )
            .await
            .unwrap_err(),
            "read_failed"
        );
    }

    #[test]
    fn budget_survives_new_callers_and_is_not_reset_by_result() {
        let root = std::env::temp_dir().join(format!(
            "simadmin-mm-recovery-{}-{}",
            std::process::id(),
            super::super::NEXT_FILE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("budget");
        claim_budget(&file).unwrap();
        assert!(claim_budget(&file).is_err());
        assert_ne!(
            budget_path("bus", ":1.1", "device", "sim1"),
            budget_path("bus", ":1.1", "device", "sim2")
        );
        assert_eq!(
            budget_path("bus", ":1.1", "device", "sim1"),
            budget_path("bus", ":1.1", "device", "sim1")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
