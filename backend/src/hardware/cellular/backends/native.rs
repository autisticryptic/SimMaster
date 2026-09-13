//! Native control plane, owned once per physical modem, not once per SIM task.
//! Queries/commands share the device gate. No operation falls back to MM.

use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

use super::{
    config::{NativeDeviceConfig, NativeProtocol},
    io::NativeIo,
    protocol::{self, at_payload, csv, labelled, CommandRequest, NetworkSnapshot},
    NativeError,
};
use crate::{
    api::models::*,
    connectivity::core::access_network::{AccessNetworkSource, ServingAccessSnapshot},
    hardware::{
        cellular::{
            bindings::{ModemBinding, SimIdentity},
            observations::{ModemObservationProvider, ObservationError},
            radio::{ModemRadioControl, RadioError, RadioState},
        },
        devices::transport::TransportFuture,
    },
};

#[derive(Debug, Clone, Default)]
pub struct NativeSnapshot {
    pub radio: RadioState,
    pub network: NetworkSnapshot,
    pub identity: SimIdentity,
    pub manufacturer: String,
    pub model: String,
    pub equipment_identifier: String,
    pub pin_state: String,
}

pub struct NativeDevice {
    pub spec: NativeDeviceConfig,
    pub(super) io: Arc<dyn NativeIo>,
    pub(super) operation: Arc<Mutex<()>>,
    pub(crate) active_interfaces: std::sync::Mutex<BTreeMap<String, String>>,
    pub(super) sms_cache: Mutex<BTreeMap<String, super::messages::NativeSms>>,
    refresh: Mutex<()>,
    voice_operation: Mutex<()>,
    snapshot: Mutex<Option<NativeSnapshot>>,
}

impl NativeDevice {
    pub fn new(spec: NativeDeviceConfig, io: Arc<dyn NativeIo>) -> Arc<Self> {
        Arc::new(Self {
            spec,
            io,
            operation: Arc::new(Mutex::new(())),
            active_interfaces: Default::default(),
            sms_cache: Default::default(),
            refresh: Mutex::new(()),
            voice_operation: Mutex::new(()),
            snapshot: Mutex::new(None),
        })
    }

    /// Shield the bounded protocol transaction from HTTP/task cancellation.
    /// A caller disappearing must not release the physical gate while the
    /// serial worker/child process still has commands in flight.
    pub async fn commands(
        self: &Arc<Self>,
        requests: Vec<CommandRequest>,
    ) -> Result<Vec<String>, NativeError> {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let _guard = this.operation.lock().await;
            let mut responses = Vec::with_capacity(requests.len());
            for request in requests {
                if super::is_shutting_down() {
                    return Err(NativeError::Unavailable(
                        "native_backend_shutting_down".into(),
                    ));
                }
                responses.push(this.io.execute(&request).await?);
            }
            Ok(responses)
        })
        .await
        .map_err(|_| NativeError::CommandFailed("native_controller_task_failed"))?
    }

    pub(crate) async fn external_sim_operation(
        self: &Arc<Self>,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, NativeError> {
        let guard = self.operation.clone().lock_owned().await;
        if super::is_shutting_down() {
            return Err(NativeError::Unavailable(
                "native_backend_shutting_down".into(),
            ));
        }
        self.io.verify_owner(&self.spec.control_device).await?;
        Ok(guard)
    }

    pub async fn command(self: &Arc<Self>, request: CommandRequest) -> Result<String, NativeError> {
        self.commands(vec![request])
            .await
            .map(|mut values| values.remove(0))
    }

    pub fn request(&self, action: &str) -> CommandRequest {
        CommandRequest::query(self.spec.protocol, &self.spec.control_device, action)
    }

    pub fn at_request(&self, command: &str) -> Result<CommandRequest, NativeError> {
        protocol::single_line(command)?;
        let device = self
            .spec
            .at_device
            .as_deref()
            .or_else(|| {
                (self.spec.protocol == NativeProtocol::At)
                    .then_some(self.spec.control_device.as_str())
            })
            .ok_or(NativeError::Unsupported("native_at_port_not_configured"))?;
        Ok(CommandRequest::query(NativeProtocol::At, device, command))
    }

    pub async fn at(self: &Arc<Self>, command: &str) -> Result<String, NativeError> {
        self.command(self.at_request(command)?).await
    }

    pub async fn radio(self: &Arc<Self>) -> Result<RadioState, NativeError> {
        let action = match self.spec.protocol {
            NativeProtocol::Qmi => "--dms-get-operating-mode",
            NativeProtocol::Mbim => "--query-radio-state",
            NativeProtocol::At => "AT+CFUN?",
        };
        let output = self.command(self.request(action)).await?;
        Ok(protocol::parse_radio(self.spec.protocol, &output))
    }

    pub async fn airplane(self: &Arc<Self>, enabled: bool) -> Result<(), NativeError> {
        let action = match (self.spec.protocol, enabled) {
            (NativeProtocol::Qmi, true) => "--dms-set-operating-mode=low-power",
            (NativeProtocol::Qmi, false) => "--dms-set-operating-mode=online",
            (NativeProtocol::Mbim, true) => "--set-radio-state=off",
            (NativeProtocol::Mbim, false) => "--set-radio-state=on",
            (NativeProtocol::At, true) => "AT+CFUN=4",
            (NativeProtocol::At, false) => "AT+CFUN=1",
        };
        self.command(self.request(action)).await?;
        for _ in 0..20 {
            if self.radio().await?.airplane_enabled() == Some(enabled) {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err(NativeError::Unavailable(
            "native_radio_transition_not_confirmed".into(),
        ))
    }

    pub async fn network(self: &Arc<Self>) -> Result<NetworkSnapshot, NativeError> {
        let action = match self.spec.protocol {
            NativeProtocol::Qmi => "--nas-get-serving-system",
            NativeProtocol::Mbim => "--query-registration-state",
            NativeProtocol::At => "AT+CEREG?",
        };
        let output = self.command(self.request(action)).await?;
        let mut result = protocol::parse_registration(self.spec.protocol, &output);
        if result.registration.registered() {
            self.verify_primary_slot().await?;
        }
        if self.spec.protocol == NativeProtocol::Qmi {
            // Scope PLMN fields to the actual serving block, not a later home
            // operator block, and never substitute 3GPP LAC for an LTE TAC.
            if let Some(block) = output.split("Current PLMN:").nth(1) {
                let block = block
                    .split("Full operator code info:")
                    .next()
                    .unwrap_or(block);
                result.plmn =
                    labelled(block, "MCC")
                        .zip(labelled(block, "MNC"))
                        .and_then(|(mcc, mnc)| {
                            let value = format!("{mcc}{mnc}");
                            (mcc.len() == 3
                                && matches!(mnc.len(), 2 | 3)
                                && protocol::valid_plmn(&value))
                            .then_some(value)
                        });
            } else {
                result.plmn = None;
            }
            result.tac = labelled(&output, "LTE tracking area code").and_then(|s| s.parse().ok());
            result.cell_id = labelled(&output, "3GPP cell ID").and_then(|s| s.parse().ok());
        }
        if self.spec.protocol == NativeProtocol::At {
            if let Ok(operator) = self.at("AT+COPS?").await {
                let parsed = protocol::parse_registration(NativeProtocol::At, &operator);
                result.plmn = parsed.plmn;
                result.operator = parsed.operator;
            }
        }
        result.signal_percent = self.signal_percent().await;
        Ok(result)
    }

    async fn signal_percent(self: &Arc<Self>) -> Option<u8> {
        let actions: &[&str] = match self.spec.protocol {
            NativeProtocol::Qmi => &["--nas-get-signal-info", "--nas-get-signal-strength"],
            NativeProtocol::Mbim => &["--query-signal-state"],
            NativeProtocol::At => &[],
        };
        for action in actions {
            if let Ok(output) = self.command(self.request(action)).await {
                if let Some(percent) = protocol::parse_signal_percent(self.spec.protocol, &output) {
                    return Some(percent);
                }
            }
        }
        self.at("AT+CSQ")
            .await
            .ok()
            .and_then(|s| protocol::parse_signal_percent(NativeProtocol::At, &s))
    }

    pub async fn sim_identity(self: &Arc<Self>) -> Result<SimIdentity, NativeError> {
        self.verify_primary_slot().await?;
        let mut identity = SimIdentity::default();
        match self.spec.protocol {
            NativeProtocol::Qmi => {
                if let Ok(output) = self.command(self.request("--dms-uim-get-imsi")).await {
                    identity.imsi = labelled(&output, "IMSI").unwrap_or_default().into();
                }
                if let Ok(output) = self.command(self.request("--dms-uim-get-iccid")).await {
                    identity.iccid = labelled(&output, "ICCID").unwrap_or_default().into();
                }
            }
            NativeProtocol::Mbim => {
                let output = self
                    .command(self.request("--query-subscriber-ready-status"))
                    .await?;
                identity.imsi = labelled(&output, "Subscriber ID")
                    .unwrap_or_default()
                    .into();
                identity.iccid = labelled(&output, "SIM ICCID").unwrap_or_default().into();
            }
            NativeProtocol::At => {}
        }
        if !valid_imsi(&identity.imsi) {
            let output = self.at("AT+CIMI").await?;
            identity.imsi = output
                .lines()
                .map(str::trim)
                .find(|v| valid_imsi(v))
                .ok_or_else(|| NativeError::Unavailable("native_sim_imsi_unavailable".into()))?
                .into();
        }
        if identity.iccid.is_empty() {
            if let Ok(output) = self.at("AT+CCID").await {
                identity.iccid = at_payload(&output, "+CCID:")
                    .unwrap_or_default()
                    .trim_matches('"')
                    .into();
            }
        }
        // EF-AD, not the visited PLMN or a guessed MNC length, selects home.
        if let Ok(output) = self.at("AT+CRSM=176,28589,0,0,4").await {
            if let Some(length) = ef_ad_mnc_length(&output) {
                if identity.imsi.len() >= 3 + length {
                    identity.operator_id = identity.imsi[..3 + length].into();
                }
            }
        }
        if !valid_imsi(&identity.imsi) {
            return Err(NativeError::Unavailable(
                "native_sim_imsi_unavailable".into(),
            ));
        }
        Ok(identity)
    }

    pub async fn refresh(self: &Arc<Self>) -> Result<NativeSnapshot, NativeError> {
        let _refresh = self.refresh.lock().await;
        let radio = self.radio().await?;
        let network = self.network().await.unwrap_or_default();
        let identity = self.sim_identity().await.unwrap_or_default();
        let query_text = |output: String| {
            output
                .lines()
                .map(str::trim)
                .find(|s| !s.is_empty() && *s != "OK" && !s.starts_with("AT"))
                .unwrap_or_default()
                .to_string()
        };
        let (manufacturer, model, equipment_identifier) =
            if self.spec.protocol == NativeProtocol::Qmi {
                let manufacturer = self
                    .command(self.request("--dms-get-manufacturer"))
                    .await
                    .ok()
                    .and_then(|s| labelled(&s, "Manufacturer").map(str::to_string))
                    .unwrap_or_default();
                let model = self
                    .command(self.request("--dms-get-model"))
                    .await
                    .ok()
                    .and_then(|s| labelled(&s, "Model").map(str::to_string))
                    .unwrap_or_default();
                let equipment = self
                    .command(self.request("--dms-get-ids"))
                    .await
                    .ok()
                    .and_then(|s| labelled(&s, "IMEI").map(str::to_string))
                    .unwrap_or_default();
                (manufacturer, model, equipment)
            } else {
                (
                    self.at("AT+CGMI").await.map(query_text).unwrap_or_default(),
                    self.at("AT+CGMM").await.map(query_text).unwrap_or_default(),
                    self.at("AT+CGSN").await.map(query_text).unwrap_or_default(),
                )
            };
        let pin_state = self
            .at("AT+CPIN?")
            .await
            .ok()
            .and_then(|s| at_payload(&s, "+CPIN:").map(str::to_string))
            .unwrap_or_else(|| "unknown".into());
        let snapshot = NativeSnapshot {
            radio,
            network,
            identity,
            manufacturer,
            model,
            equipment_identifier,
            pin_state,
        };
        *self.snapshot.lock().await = Some(snapshot.clone());
        Ok(snapshot)
    }

    pub fn binding(&self, snapshot: &NativeSnapshot, present: bool) -> ModemBinding {
        ModemBinding {
            line_id: self.spec.line_id(),
            slot_source: "native_explicit_physical_anchor".into(),
            slot_stable: true,
            hardware_key: self.spec.hardware_key.clone(),
            equipment_identifier: snapshot.equipment_identifier.clone(),
            modem_id: self.spec.selector(),
            modem_path: self.spec.selector(),
            manufacturer: snapshot.manufacturer.clone(),
            model: snapshot.model.clone(),
            device_family: "native_unvalidated".into(),
            control_transport: match self.spec.protocol {
                NativeProtocol::Qmi => "native_qmi",
                NativeProtocol::Mbim => "native_mbim",
                NativeProtocol::At => "native_at",
            }
            .into(),
            primary_port: self.spec.control_device.clone(),
            qmi_device: (self.spec.protocol == NativeProtocol::Qmi)
                .then(|| self.spec.control_device.clone()),
            uim_slot: self.spec.uim_slot,
            sim_iccid: snapshot.identity.iccid.clone(),
            operator_id: snapshot.identity.operator_id.clone(),
            state: snapshot.network.registration.label().into(),
            present,
            sim_type: "unknown".into(),
            esim_status: "unknown".into(),
            line_kind: "baseband".into(),
            ..Default::default()
        }
    }

    pub async fn register(self: &Arc<Self>, plmn: &str) -> Result<(), NativeError> {
        if !plmn.is_empty() && !protocol::valid_plmn(plmn) {
            return Err(NativeError::Protocol("native_operator_plmn_invalid".into()));
        }
        if self.spec.protocol == NativeProtocol::Qmi {
            return self.select_qmi_operator(plmn).await;
        }
        let action = if plmn.is_empty() {
            "AT+COPS=0".to_string()
        } else {
            format!("AT+COPS=1,2,\"{plmn}\"")
        };
        // AT network selection is shared across QMI/MBIM combinations, but
        // an absent AT capability is explicit, not an MM escape path.
        let mut request = self.at_request(&action)?;
        request.timeout_seconds = 120;
        self.command(request).await.map(|_| ())
    }

    pub async fn calls(self: &Arc<Self>) -> Result<CallListResponse, NativeError> {
        self.verify_primary_slot().await?;
        let text = self.at("AT+CLCC").await?;
        let mut calls = Vec::new();
        for row in text.lines().filter_map(|l| l.trim().strip_prefix("+CLCC:")) {
            let fields = csv(row.trim())?;
            if fields.len() < 5 {
                continue;
            }
            let index = fields[0]
                .parse::<u32>()
                .map_err(|_| NativeError::Protocol("native_call_id_invalid".into()))?;
            let state = match fields[2].as_str() {
                "0" => "active",
                "1" => "held",
                "2" => "dialing",
                "3" => "alerting",
                "4" => "incoming",
                "5" => "waiting",
                _ => "unknown",
            };
            calls.push(CallInfo {
                path: format!("{}:call:{index}", self.spec.selector()),
                line_id: self.spec.line_id(),
                phone_number: fields.get(5).cloned().unwrap_or_default(),
                direction: if fields[1] == "1" {
                    "incoming"
                } else {
                    "outgoing"
                }
                .into(),
                state: state.into(),
                start_time: None,
            });
        }
        Ok(CallListResponse { calls })
    }

    pub async fn call(self: &Arc<Self>, path: &str) -> Result<CallInfo, NativeError> {
        self.calls()
            .await?
            .calls
            .into_iter()
            .find(|c| c.path == path)
            .ok_or_else(|| NativeError::Unavailable("native_call_not_found_on_device".into()))
    }

    pub async fn dial(self: &Arc<Self>, number: &str) -> Result<String, NativeError> {
        protocol::phone_number(number)?;
        let this = self.clone();
        let number = number.to_string();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let _voice = this.voice_operation.lock().await;
            let mut attempted = false;
            let result = this.dial_owned(&number, &sender, &mut attempted).await;
            let failed = result.is_err();
            let abandoned = sender.send(result).is_err();
            if attempted && (failed || abandoned) {
                // Only a fresh observation of this outgoing call authorizes
                // cleanup. Never ATH an unrelated incoming call on cancellation.
                match this.calls().await {
                    Ok(calls) => {
                        for call in calls
                            .calls
                            .into_iter()
                            .filter(|c| c.direction == "outgoing" && c.phone_number == number)
                        {
                            if let Some(index) = call
                                .path
                                .rsplit(':')
                                .next()
                                .and_then(|s| s.parse::<u32>().ok())
                            {
                                let _ = this.at(&format!("AT+CHLD=1{index}")).await;
                            }
                        }
                    }
                    Err(_) => {
                        tracing::warn!(line_id = %this.spec.line_id(), "Native dial outcome unconfirmed; caller must inspect the owned modem before retrying")
                    }
                }
            }
        });
        receiver
            .await
            .map_err(|_| NativeError::CommandFailed("native_voice_controller_task_failed"))?
    }

    async fn dial_owned(
        self: &Arc<Self>,
        number: &str,
        caller: &tokio::sync::oneshot::Sender<Result<String, NativeError>>,
        attempted: &mut bool,
    ) -> Result<String, NativeError> {
        if caller.is_closed() {
            return Err(NativeError::Unavailable("native_dial_cancelled".into()));
        }
        if !self.calls().await?.calls.is_empty() {
            return Err(NativeError::Unavailable("native_voice_device_busy".into()));
        }
        if caller.is_closed() {
            return Err(NativeError::Unavailable("native_dial_cancelled".into()));
        }
        *attempted = true;
        self.at(&format!("ATD{number};")).await?;
        for _ in 0..5 {
            if let Some(call) = self
                .calls()
                .await?
                .calls
                .into_iter()
                .find(|c| c.direction == "outgoing")
            {
                return Ok(call.path);
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        // Do not retry dialing when the first call may actually have started.
        Err(NativeError::Unavailable(
            "native_call_started_identity_unconfirmed".into(),
        ))
    }

    pub async fn hangup(self: &Arc<Self>, path: Option<&str>) -> Result<(), NativeError> {
        let _voice = self.voice_operation.lock().await;
        if let Some(path) = path {
            self.call(path).await?;
            let index = path
                .rsplit(':')
                .next()
                .and_then(|i| i.parse::<u32>().ok())
                .ok_or_else(|| NativeError::Protocol("native_call_id_invalid".into()))?;
            self.at(&format!("AT+CHLD=1{index}")).await?;
        } else {
            self.at("ATH").await?;
        }
        Ok(())
    }

    pub async fn answer(self: &Arc<Self>, path: &str) -> Result<(), NativeError> {
        let _voice = self.voice_operation.lock().await;
        let call = self.call(path).await?;
        if !matches!(call.state.as_str(), "incoming" | "waiting") {
            return Err(NativeError::Unavailable("native_call_not_ringing".into()));
        }
        self.at("ATA").await.map(|_| ())
    }

    pub async fn dtmf(self: &Arc<Self>, path: &str, digit: &str) -> Result<(), NativeError> {
        let _voice = self.voice_operation.lock().await;
        self.call(path).await?;
        if digit.len() != 1 || !digit.bytes().all(|c| b"0123456789*#ABCD".contains(&c)) {
            return Err(NativeError::Protocol("native_dtmf_invalid".into()));
        }
        self.at(&format!("AT+VTS={digit}")).await.map(|_| ())
    }

    pub async fn call_settings(self: &Arc<Self>) -> Result<CallSettingsResponse, NativeError> {
        let output = self.at("AT+CCWA=1,2").await?;
        let waiting = at_payload(&output, "+CCWA:").and_then(|s| s.split(',').next());
        Ok(CallSettingsResponse {
            voice_call_waiting: match waiting {
                Some("1") => "enabled",
                Some("0") => "disabled",
                _ => "unknown",
            }
            .into(),
            ..Default::default()
        })
    }

    pub async fn reset(self: &Arc<Self>, sim_only: bool) -> Result<(), NativeError> {
        if sim_only && self.spec.protocol == NativeProtocol::Qmi {
            self.command(self.request(&format!("--uim-sim-power-off={}", self.spec.uim_slot)))
                .await?;
            self.command(self.request(&format!("--uim-sim-power-on={}", self.spec.uim_slot)))
                .await?;
            return Ok(());
        }
        let action = match self.spec.protocol {
            NativeProtocol::Qmi => "--dms-set-operating-mode=reset",
            NativeProtocol::At => "AT+CFUN=1,1",
            NativeProtocol::Mbim => {
                return Err(NativeError::Unsupported(
                    "native_mbim_reset_requires_device_driver",
                ))
            }
        };
        self.command(self.request(action)).await.map(|_| ())
    }

    pub(super) async fn verify_primary_slot(self: &Arc<Self>) -> Result<(), NativeError> {
        if self.spec.protocol != NativeProtocol::Qmi {
            return Ok(());
        }
        let text = self.command(self.request("--uim-get-card-status")).await?;
        let slot = text.lines().find_map(|line| {
            let value = line.trim().strip_prefix("Primary GW:")?;
            let value = value.trim().strip_prefix("slot '")?;
            value.split('\'').next()?.parse::<u8>().ok()
        });
        if slot == Some(self.spec.uim_slot) {
            Ok(())
        } else {
            Err(NativeError::OwnerConflict(
                "native_primary_sim_slot_unconfirmed_or_mismatched".into(),
            ))
        }
    }
}

fn valid_imsi(value: &str) -> bool {
    (5..=15).contains(&value.len()) && value.bytes().all(|c| c.is_ascii_digit())
}

fn ef_ad_mnc_length(text: &str) -> Option<usize> {
    let fields = csv(at_payload(text, "+CRSM:")?).ok()?;
    if fields.len() < 3
        || !matches!(
            (fields[0].as_str(), fields[1].as_str()),
            ("144", "0") | ("145", _)
        )
    {
        return None;
    }
    let bytes = fields[2].as_bytes();
    let length = std::str::from_utf8(bytes.get(6..8)?)
        .ok()
        .and_then(|s| u8::from_str_radix(s, 16).ok())?
        & 0x0f;
    matches!(length, 2 | 3).then_some(length as usize)
}

pub struct NativeFleet {
    devices: BTreeMap<String, Arc<NativeDevice>>,
}

impl NativeFleet {
    pub fn new(devices: Vec<Arc<NativeDevice>>) -> Result<Arc<Self>, NativeError> {
        let mut map = BTreeMap::new();
        for device in devices {
            if map.insert(device.spec.selector(), device).is_some() {
                return Err(NativeError::OwnerConflict(
                    "native_duplicate_controller".into(),
                ));
            }
        }
        Ok(Arc::new(Self { devices: map }))
    }

    pub fn device(&self, selector: &str) -> Result<Arc<NativeDevice>, NativeError> {
        self.devices
            .get(selector)
            .cloned()
            .ok_or_else(|| NativeError::Unavailable("native_device_not_owned".into()))
    }

    pub fn all(&self) -> Vec<Arc<NativeDevice>> {
        self.devices.values().cloned().collect()
    }

    pub fn by_control_device(&self, port: &str) -> Result<Arc<NativeDevice>, NativeError> {
        self.devices
            .values()
            .find(|d| d.spec.control_device == port)
            .cloned()
            .ok_or_else(|| NativeError::Unavailable("native_control_device_not_owned".into()))
    }
}

impl ModemObservationProvider for NativeFleet {
    fn name(&self) -> &'static str {
        "native_unvalidated"
    }

    fn discover(&self) -> TransportFuture<'_, Result<Vec<ModemBinding>, ObservationError>> {
        Box::pin(async move {
            let mut bindings = Vec::new();
            for device in self.devices.values() {
                match device.refresh().await {
                    Ok(snapshot) => bindings.push(device.binding(&snapshot, true)),
                    Err(error) => {
                        tracing::warn!(line_id = %device.spec.line_id(), %error, "Native discovery could not verify device");
                        bindings.push(device.binding(&NativeSnapshot::default(), false));
                    }
                }
            }
            Ok(bindings)
        })
    }

    fn serving_access<'a>(
        &'a self,
        binding: &'a ModemBinding,
    ) -> TransportFuture<'a, Result<ServingAccessSnapshot, ObservationError>> {
        Box::pin(async move {
            let device = self
                .device(&binding.modem_path)
                .map_err(|e| ObservationError::Unavailable(e.to_string()))?;
            let network = device
                .network()
                .await
                .map_err(|e| ObservationError::Transient(e.to_string()))?;
            if !network.registration.registered() {
                return Err(ObservationError::Unavailable(
                    "native_network_not_registered".into(),
                ));
            }
            let plmn = network
                .plmn
                .as_deref()
                .filter(|p| protocol::valid_plmn(p))
                .ok_or_else(|| {
                    ObservationError::Unavailable("native_serving_plmn_unknown".into())
                })?;
            ServingAccessSnapshot::new(
                &plmn[..3],
                &plmn[3..],
                &network.technology,
                network.cell_id.unwrap_or(0),
                network.tac.unwrap_or(0),
                None,
                AccessNetworkSource::Native,
            )
            .ok_or_else(|| ObservationError::Unavailable("native_serving_cell_incomplete".into()))
        })
    }
}

impl ModemRadioControl for NativeFleet {
    fn observe<'a>(
        &'a self,
        binding: &'a ModemBinding,
    ) -> TransportFuture<'a, Result<RadioState, RadioError>> {
        Box::pin(async move {
            if binding.line_kind == "reader" {
                return Err(RadioError::Unsupported("line_has_no_baseband".into()));
            }
            if !binding.present {
                return Err(RadioError::Absent);
            }
            self.device(&binding.modem_path)
                .map_err(|e| RadioError::Unavailable(e.to_string()))?
                .radio()
                .await
                .map_err(|e| RadioError::Unavailable(e.to_string()))
        })
    }

    fn set_airplane_mode<'a>(
        &'a self,
        binding: &'a ModemBinding,
        enabled: bool,
    ) -> TransportFuture<'a, Result<(), RadioError>> {
        Box::pin(async move {
            if !binding.present {
                return Err(RadioError::Absent);
            }
            self.device(&binding.modem_path)
                .map_err(|e| RadioError::Unavailable(e.to_string()))?
                .airplane(enabled)
                .await
                .map_err(|e| RadioError::Failed(e.to_string()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScriptedIo {
        requests: std::sync::Mutex<Vec<CommandRequest>>,
        replies: std::sync::Mutex<std::collections::VecDeque<Result<String, NativeError>>>,
    }

    impl NativeIo for ScriptedIo {
        fn execute<'a>(
            &'a self,
            request: &'a CommandRequest,
        ) -> TransportFuture<'a, Result<String, NativeError>> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(request.clone());
                self.replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected native IO")
            })
        }
    }

    fn at_device(io: Arc<dyn NativeIo>) -> Arc<NativeDevice> {
        NativeDevice::new(
            NativeDeviceConfig {
                hardware_key: "test-native".into(),
                sysfs_anchor: "/sys/devices/fixture".into(),
                protocol: NativeProtocol::At,
                control_device: "/dev/fixture-at".into(),
                at_device: None,
                sms_reception_enabled: false,
                uim_slot: 1,
                ims: None,
                data: None,
            },
            io,
        )
    }

    #[tokio::test]
    async fn native_radio_executes_protocol_commands_without_mm_and_confirms_state() {
        let io = Arc::new(ScriptedIo {
            requests: Default::default(),
            replies: std::sync::Mutex::new([Ok("OK".into()), Ok("+CFUN: 4\r\nOK".into())].into()),
        });
        let device = at_device(io.clone());
        device.airplane(true).await.unwrap();
        let requests = io.requests.lock().unwrap();
        assert_eq!(requests[0].arguments, ["AT+CFUN=4"]);
        assert_eq!(requests[1].arguments, ["AT+CFUN?"]);
        assert!(requests.iter().all(|r| r.tool == protocol::Tool::At));
    }

    #[tokio::test]
    async fn failed_native_io_never_calls_a_second_backend() {
        let io = Arc::new(ScriptedIo {
            requests: Default::default(),
            replies: std::sync::Mutex::new([Err(NativeError::CommandFailed("fixture"))].into()),
        });
        let device = at_device(io.clone());
        assert!(device.radio().await.is_err());
        assert_eq!(io.requests.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn qmi_signal_does_not_require_an_at_port() {
        let io = Arc::new(ScriptedIo {
            requests: Default::default(),
            replies: std::sync::Mutex::new([Ok("LTE:\n RSSI: '-55 dBm'".into())].into()),
        });
        let mut spec = at_device(io.clone()).spec.clone();
        spec.protocol = NativeProtocol::Qmi;
        let device = NativeDevice::new(spec, io.clone());
        assert_eq!(device.signal_percent().await, Some(93));
        let requests = io.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].tool, protocol::Tool::Qmi);
        assert!(requests[0]
            .arguments
            .iter()
            .any(|a| a == "--nas-get-signal-info"));
    }

    #[test]
    fn ef_ad_requires_a_success_status_and_valid_mnc_length() {
        assert_eq!(ef_ad_mnc_length("+CRSM: 144,0,\"00000003\""), Some(3));
        assert_eq!(ef_ad_mnc_length("+CRSM: 144,0,\"00000002\""), Some(2));
        assert_eq!(ef_ad_mnc_length("+CRSM: 106,130,\"00000003\""), None);
        assert_eq!(ef_ad_mnc_length("+CRSM: 144,0,\"000000FF\""), None);
    }

    struct CancelledDialIo {
        dialled: std::sync::atomic::AtomicBool,
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        cleaned: tokio::sync::Notify,
    }
    impl NativeIo for CancelledDialIo {
        fn execute<'a>(
            &'a self,
            request: &'a CommandRequest,
        ) -> TransportFuture<'a, Result<String, NativeError>> {
            Box::pin(async move {
                use std::sync::atomic::Ordering;
                let action = &request.arguments[0];
                if action.starts_with("ATD") {
                    self.entered.notify_one();
                    self.release.notified().await;
                    self.dialled.store(true, Ordering::Release);
                } else if action == "AT+CLCC" && self.dialled.load(Ordering::Acquire) {
                    return Ok("+CLCC: 1,0,2,0,0,\"12345\",129\r\nOK".into());
                } else if action == "AT+CHLD=11" {
                    self.dialled.store(false, Ordering::Release);
                    self.cleaned.notify_one();
                }
                Ok("OK".into())
            })
        }
    }

    #[tokio::test]
    async fn caller_cancellation_after_atd_does_not_abandon_a_billable_outgoing_call() {
        let io = Arc::new(CancelledDialIo {
            dialled: std::sync::atomic::AtomicBool::new(false),
            entered: Default::default(),
            release: Default::default(),
            cleaned: Default::default(),
        });
        let device = at_device(io.clone());
        let caller = tokio::spawn(async move { device.dial("12345").await });
        tokio::time::timeout(std::time::Duration::from_secs(2), io.entered.notified())
            .await
            .unwrap();
        caller.abort();
        let _ = caller.await;
        io.release.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(2), io.cleaned.notified())
            .await
            .unwrap();
        assert!(!io.dialled.load(std::sync::atomic::Ordering::Acquire));
    }
}
