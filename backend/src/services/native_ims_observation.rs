//! Explicit, cached modem-native IMS observations. This never supplies host SIP
//! registration, lease, voice/SMS readiness or permission for billable traffic.
//! GET paths are local projections; only `probe` may invoke the injected driver.

use std::{sync::{Arc, Mutex}, time::{Duration, Instant}};
use serde::Serialize;

use crate::{
    hardware::cellular::{bindings::ModemBinding, native_ims::{NativeImsProbeError, NativeImsSample}},
    platform::{config::ConfigManager, shutdown::ShutdownSignal},
    services::line_registry::{LineRuntime, LineRuntimeRegistry},
};

pub const OBSERVATION_TTL: Duration = Duration::from_secs(30);
const PROBE_COOLDOWN: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationState { NotProbed, Probing, Observed, Stale, Unavailable }

#[derive(Debug, Clone, Serialize)]
pub struct NativeImsObservationStatus {
    pub line_id: String,
    /// Installed adapter/topology only. Model and firmware qualification occurs
    /// during an explicit probe, never by reading this field.
    pub adapter_available: bool,
    pub state: ObservationState,
    pub sample: Option<NativeImsSample>,
    pub observed_at: Option<String>,
    pub age_seconds: Option<u64>,
    pub stale: bool,
    pub last_error: Option<&'static str>,
    pub sample_kind: &'static str,
    pub purpose: &'static str,
}

/// Deliberately neither Debug nor Serialize: this holds private binding values.
#[derive(Clone, PartialEq, Eq)]
struct Scope {
    binding: ModemBinding,
    binding_revision: u64,
    cellular_generation: u64,
    wifi_generation: u64,
    worker_generation: u64,
    host_epoch: u64,
}

impl Scope {
    fn capture(line: &LineRuntime) -> Self {
        let (binding, binding_revision) = line.native_observation_binding();
        Self { binding, binding_revision,
            cellular_generation: line.cellular_ims.generation(),
            wifi_generation: line.vowifi.native_observation_generation(),
            worker_generation: line.ue_worker.generation(),
            host_epoch: line.ims_registration.native_observation_epoch() }
    }
}

struct Sample {
    value: NativeImsSample,
    at: Instant,
    timestamp: String,
}

#[derive(Default)]
struct State {
    scope: Option<Scope>,
    provider_context: Option<u64>,
    sample: Option<Sample>,
    last_error: Option<&'static str>,
    next_probe: Option<Instant>,
    in_flight: bool,
}

#[derive(Default)]
pub struct NativeImsObservation {
    state: Mutex<State>,
}

struct ProbeGuard {
    cache: Arc<NativeImsObservation>,
    completed: bool,
}
impl Drop for ProbeGuard {
    fn drop(&mut self) {
        if !self.completed {
            let mut state = self.cache.state.lock().unwrap_or_else(|e| e.into_inner());
            state.in_flight = false;
            state.last_error = Some(NativeImsProbeError::Unavailable.code());
        }
    }
}

impl NativeImsObservation {
    fn begin(self: &Arc<Self>, scope: Scope, provider_context: u64, now: Instant) -> Result<ProbeGuard, NativeImsProbeError> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        // Scope changes never clear an in-flight physical transaction or turn
        // repeated requests into an unbounded per-device queue.
        if state.in_flight || state.next_probe.is_some_and(|deadline| now < deadline) {
            return Err(NativeImsProbeError::Busy);
        }
        if state.scope.as_ref() != Some(&scope) || state.provider_context != Some(provider_context) {
            state.scope = Some(scope);
            state.provider_context = Some(provider_context);
            state.sample = None;
            state.last_error = None;
        }
        state.in_flight = true;
        state.next_probe = Some(now + PROBE_COOLDOWN);
        Ok(ProbeGuard { cache: Arc::clone(self), completed: false })
    }

    fn finish(&self, guard: &mut ProbeGuard, scope_current: bool, result: Result<NativeImsSample, NativeImsProbeError>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.in_flight = false;
        if !scope_current {
            state.scope = None;
            state.sample = None;
            state.last_error = Some(NativeImsProbeError::BindingChanged.code());
        } else {
            match result {
                Ok(sample) => {
                    state.sample = Some(Sample { value: sample, at: Instant::now(),
                        timestamp: chrono::Utc::now().to_rfc3339() });
                    state.last_error = None;
                }
                Err(error) => state.last_error = Some(error.code()),
            }
        }
        guard.completed = true;
    }

    fn status(&self, line_id: &str, scope: &Scope, adapter_available: bool,
        provider_context: Option<u64>, blocked: Option<NativeImsProbeError>, now: Instant,
    ) -> NativeImsObservationStatus {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let same_scope = state.scope.as_ref() == Some(scope) && state.provider_context == provider_context;
        let current = same_scope && blocked.is_none();
        let sample = current.then_some(state.sample.as_ref()).flatten();
        let age = sample.map(|sample| now.saturating_duration_since(sample.at));
        let stale = !current || state.in_flight || state.last_error.is_some()
            || age.is_none_or(|age| age >= OBSERVATION_TTL);
        let last_error = blocked.map(NativeImsProbeError::code)
            .or_else(|| current.then_some(state.last_error).flatten());
        let status = if state.in_flight { ObservationState::Probing }
            else if blocked.is_some() || (same_scope && state.last_error.is_some()) { ObservationState::Unavailable }
            else if sample.is_some() { if stale { ObservationState::Stale } else { ObservationState::Observed } }
            else { ObservationState::NotProbed };
        NativeImsObservationStatus { line_id: line_id.to_owned(), adapter_available, state: status,
            sample: sample.map(|sample| sample.value), observed_at: sample.map(|sample| sample.timestamp.clone()),
            age_seconds: age.map(|age| age.as_secs()), stale, last_error,
            sample_kind: "sequential_not_atomic", purpose: "modem_observation_not_host_ims_readiness" }
    }
}

fn admission(line: &LineRuntime, config: &ConfigManager, shutdown: &ShutdownSignal) -> Result<(), NativeImsProbeError> {
    if shutdown.is_shutting_down() || crate::hardware::cellular::backends::is_shutting_down() {
        return Err(NativeImsProbeError::Unavailable);
    }
    let binding = line.binding();
    if !binding.present || binding.slot_conflict || !binding.slot_stable
        || !matches!(binding.line_kind.as_str(), "" | "baseband")
        || !binding.modem_path.starts_with("native:") || binding.hardware_key.is_empty()
        || binding.uim_slot == 0 || binding.primary_port.is_empty()
        || !line.cellular_ims.mm_binding_ready()
    { return Err(NativeImsProbeError::BindingChanged); }
    let digits = binding.sim_iccid.trim().trim_end_matches(['F', 'f']);
    if !(19..=20).contains(&digits.len()) || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(NativeImsProbeError::BindingChanged);
    }
    let profile = config.get_line_profile(&binding.line_id);
    if !profile.enabled || profile.airplane_mode_enabled { return Err(NativeImsProbeError::Unavailable); }
    if profile.cellular_ims_connection_enabled || profile.vowifi.enabled
        || line.cellular_ims_retry_running.load(std::sync::atomic::Ordering::Acquire)
        || line.vowifi_restore_in_progress()
        || !line.cellular_ims_live.idle_for_native_ims_observation()
        || !crate::connectivity::modems::ims::vowifi::live::idle_for_native_ims_observation(&binding.line_id)
    { return Err(NativeImsProbeError::Busy); }
    Ok(())
}

/// Safe zero-probe-I/O view. The only asynchronous lookup is the registry map.
/// No refresh/discovery, device query, worker creation, cleanup or config write.
pub async fn status(registry: &LineRuntimeRegistry, config: &ConfigManager,
    shutdown: &ShutdownSignal, line_id: &str,
) -> Option<NativeImsObservationStatus> {
    let line = registry.get(line_id).await?;
    Some(project(&line, registry, config, shutdown))
}

fn project(line: &LineRuntime, registry: &LineRuntimeRegistry, config: &ConfigManager,
    shutdown: &ShutdownSignal,
) -> NativeImsObservationStatus {
    let scope = Scope::capture(line);
    let provider = registry.observation_provider();
    let available = provider.supports_native_ims_probe(&scope.binding);
    let context = provider.native_ims_probe_context(&scope.binding);
    let blocked = if !available { Some(NativeImsProbeError::Unsupported) }
        else if context.is_none() { Some(NativeImsProbeError::Unavailable) }
        else { admission(line, config, shutdown).err() };
    line.native_ims_observation.status(&scope.binding.line_id, &scope, available, context, blocked, Instant::now())
}

/// Only called by an authenticated, explicit POST. The owned task retains ALL
/// exclusion guards while the driver's own shielded transaction drains. Never
/// timeout this join and release exclusion while serial work is still running.
pub async fn probe(registry: Arc<LineRuntimeRegistry>, config: Arc<ConfigManager>,
    shutdown: ShutdownSignal, line_id: &str,
) -> Result<NativeImsObservationStatus, NativeImsProbeError> {
    let line = registry.get(line_id).await.ok_or(NativeImsProbeError::BindingChanged)?;
    let scope = Scope::capture(&line);
    let provider = registry.observation_provider();
    if !provider.supports_native_ims_probe(&scope.binding) { return Err(NativeImsProbeError::Unsupported); }
    admission(&line, &config, &shutdown)?;
    let provider_context = provider.native_ims_probe_context(&scope.binding).ok_or(NativeImsProbeError::Unavailable)?;
    let mut flight = line.native_ims_observation.begin(scope.clone(), provider_context, Instant::now())?;
    let task = tokio::spawn(async move {
        let work = async {
            // Nonwaiting acquisition cannot deadlock with normal transition ->
            // bearer/access -> device paths, and cannot accumulate pending probes.
            let _transition = line.ims_registration.transition_lock.try_lock().map_err(|_| NativeImsProbeError::Busy)?;
            let _coordinator = line.ims_registration.try_native_observation().map_err(|_| NativeImsProbeError::Busy)?;
            let _bearer = line.bearer_operation_lock.try_lock().map_err(|_| NativeImsProbeError::Busy)?;
            let _cellular = line.cellular_ims_connect_lock.try_lock().map_err(|_| NativeImsProbeError::Busy)?;
            let _wifi = line.vowifi_connect_lock.try_lock().map_err(|_| NativeImsProbeError::Busy)?;
            admission(&line, &config, &shutdown)?;
            if Scope::capture(&line) != scope || provider.native_ims_probe_context(&scope.binding) != Some(provider_context) {
                return Err(NativeImsProbeError::BindingChanged);
            }
            let result = provider.probe_native_ims(&scope.binding).await;
            // Map lookup cannot dispatch hardware. Check the original runtime,
            // not another Arc with the same line_id that appeared during await.
            let current = registry.get(&scope.binding.line_id).await;
            let valid = current.as_ref().is_some_and(|current| Arc::ptr_eq(current, &line))
                && Scope::capture(&line) == scope
                && provider.native_ims_probe_context(&scope.binding) == Some(provider_context)
                && admission(&line, &config, &shutdown).is_ok();
            let result = if valid { result } else { Err(NativeImsProbeError::BindingChanged) };
            line.native_ims_observation.finish(&mut flight, valid, result);
            result.map(|_| project(&line, &registry, &config, &shutdown))
        }.await;
        // Pre-dispatch failure still resolves the cache's in-flight state but
        // does not fabricate a sample or erase an earlier valid timestamp.
        if !flight.completed {
            let valid = Scope::capture(&line) == scope;
            let error = work.as_ref().err().copied().unwrap_or(NativeImsProbeError::Unavailable);
            line.native_ims_observation.finish(&mut flight, valid, Err(error));
        }
        work
    });
    task.await.map_err(|_| NativeImsProbeError::Unavailable)?
}

#[cfg(test)]
#[path = "native_ims_observation_tests.rs"]
pub(crate) mod tests;
