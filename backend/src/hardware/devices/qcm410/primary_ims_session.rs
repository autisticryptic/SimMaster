//! QCA410 primary-QMI IMS sessions owned by ModemManager.
//!
//! The control endpoint is still the primary qmi0 through qmi-proxy, never
//! DATA6. ModemManager keeps the WDS client, BAM-DMUX binding, IP-family setup
//! and indications together for the entire bearer lifetime. A standalone
//! qmicli follow process is NOT equivalent on the field-tested firmware:
//! it acquired an IMS address but received no SIP traffic and disconnected.
//!
//! This module creates and releases only its own IMS bearer. It neither asks
//! NetworkManager to connect nor installs host routes. The caller exclusively
//! configures the returned data interface and moves it into the UE namespace.

use std::{
    future::Future,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use tokio::{
    sync::{oneshot, watch},
    task::JoinHandle,
};

use super::{
    netdev::{self, NetdevConfig},
    primary_ims_lifecycle::{self as lifecycle, MmBus, OwnedLease},
};

const BEARER_PREFIX: &str = "/org/freedesktop/ModemManager1/Bearer/";
pub(super) const OWNER_MISSING: &str = "qca410_primary_mm_owner_missing";

pub(super) struct PrimaryImsRequest<'a> {
    pub device: &'a str,
    pub modem: &'a str,
    pub interface: &'a str,
    pub apn: &'a str,
    pub profile_id: Option<u32>,
    pub family: u8,
    pub allow_roaming: bool,
}

pub(super) struct PrimaryImsSession {
    controller: Arc<Controller>,
    bearer: String,
    loss: watch::Receiver<Option<String>>,
    monitor: JoinHandle<()>,
}

impl PrimaryImsSession {
    pub async fn start(request: PrimaryImsRequest<'_>) -> Result<Self, String> {
        create_args(&request)?;
        if lifecycle::is_shutting_down() {
            return Err("qca410_primary_mm_shutting_down".to_string());
        }
        let request = OwnedRequest::from(request);
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut cancellation = CancelSetup {
            flag: Arc::clone(&cancelled),
            armed: true,
        };
        let pending = lifecycle::PendingSetup::new();
        let (sender, receiver) = oneshot::channel();
        // Shield CreateBearer/Connect from caller cancellation. If the caller
        // disappears, the task must still record the returned object and clean
        // it up; abandoning an RPC future loses ownership of its side effects.
        tokio::spawn(async move {
            let _pending = pending;
            let result = Self::start_inner(request, cancelled).await;
            deliver_setup(sender, result, |mut session| async move {
                session.stop().await;
            })
            .await;
        });
        let result = receiver
            .await
            .map_err(|_| "qca410_primary_mm_setup_task_failed".to_string())?;
        cancellation.armed = false;
        result
    }

    async fn start_inner(
        request: OwnedRequest,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        lifecycle::recover_owned().await?;
        if cancelled.load(Ordering::Acquire) || lifecycle::is_shutting_down() {
            return Err("qca410_primary_mm_setup_cancelled".to_string());
        }
        let bus = MmBus::new(&request.device, &request.modem, &request.interface).await?;
        let controller = Arc::new(Controller {
            bus,
            request,
            cancelled,
            previous: Mutex::new(None),
            owned: Mutex::new(None),
        });
        let runner = Arc::clone(&controller);
        let bearer = prepare_with(&controller.request.borrowed(), move |args| {
            Arc::clone(&runner).run(args)
        })
        .await?;
        let (sender, loss) = watch::channel(None);
        let monitored_bearer = bearer.clone();
        let monitor_controller = Arc::clone(&controller);
        let monitor = tokio::spawn(async move {
            let mut read_failures = 0_u32;
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let result = Arc::clone(&monitor_controller)
                    .run(status_args(&monitored_bearer))
                    .await;
                if let Some(error) = observed_loss(&result) {
                    let _ = sender.send(Some(error));
                    break;
                }
                if result.is_err() {
                    read_failures = read_failures.saturating_add(1);
                    if read_failures == 1 || read_failures % 30 == 0 {
                        tracing::warn!(
                            read_failures,
                            "Primary IMS bearer status temporarily unavailable; not treating a D-Bus timeout as bearer loss"
                        );
                    }
                } else {
                    read_failures = 0;
                }
            }
        });
        Ok(Self {
            controller,
            bearer,
            loss,
            monitor,
        })
    }

    pub fn path(&self) -> &str {
        &self.bearer
    }

    /// Cached provider observation only; this must not open a new QMI client.
    pub fn check_liveness(&self) -> Result<(), String> {
        let lease = self.controller.owned(&self.bearer)?;
        if lease.is_done() || lease.is_closing() || lifecycle::is_shutting_down() {
            return Err("qca410_primary_mm_session_closing".to_string());
        }
        if let Some(error) = self.loss.borrow().as_ref() {
            return Err(error.clone());
        }
        if self.monitor.is_finished() {
            return Err("qca410_primary_mm_status_monitor_stopped".to_string());
        }
        Ok(())
    }

    pub async fn stop(&mut self) {
        self.monitor.abort();
        let _ = (&mut self.monitor).await;
        let lease = self.controller.owned(&self.bearer);
        if let Err(error) = async { lease?.cleanup().await }.await {
            tracing::warn!(error, "Could not release the owned primary IMS bearer");
        }
    }

    pub fn network_will_be_configured(
        &self,
        config: &NetdevConfig,
    ) -> Result<lifecycle::NetworkGuard, String> {
        self.controller
            .owned(&self.bearer)?
            .network_will_be_configured(config)
    }

    pub fn namespace_will_change(
        &self,
        namespace: &str,
    ) -> Result<lifecycle::NetworkGuard, String> {
        self.controller
            .owned(&self.bearer)?
            .namespace_will_change(namespace)
    }
}

async fn deliver_setup<T, F, R>(
    sender: oneshot::Sender<Result<T, String>>,
    result: Result<T, String>,
    cleanup: F,
) where
    F: FnOnce(T) -> R,
    R: Future<Output = ()>,
{
    if let Err(Ok(resource)) = sender.send(result) {
        cleanup(resource).await;
    }
}

impl Drop for PrimaryImsSession {
    fn drop(&mut self) {
        self.monitor.abort();
        // Once the monitor and this session drop their controller references,
        // Controller::drop arranges cleanup, including cancelled setup paths.
    }
}

struct CancelSetup {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl Drop for CancelSetup {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::Release);
        }
    }
}

struct OwnedRequest {
    device: String,
    modem: String,
    interface: String,
    apn: String,
    profile_id: Option<u32>,
    family: u8,
    allow_roaming: bool,
}

impl From<PrimaryImsRequest<'_>> for OwnedRequest {
    fn from(request: PrimaryImsRequest<'_>) -> Self {
        Self {
            device: request.device.to_string(),
            modem: request.modem.to_string(),
            interface: request.interface.to_string(),
            apn: request.apn.to_string(),
            profile_id: request.profile_id,
            family: request.family,
            allow_roaming: request.allow_roaming,
        }
    }
}

impl OwnedRequest {
    fn borrowed(&self) -> PrimaryImsRequest<'_> {
        PrimaryImsRequest {
            device: &self.device,
            modem: &self.modem,
            interface: &self.interface,
            apn: &self.apn,
            profile_id: self.profile_id,
            family: self.family,
            allow_roaming: self.allow_roaming,
        }
    }
}

struct Controller {
    bus: Arc<MmBus>,
    request: OwnedRequest,
    cancelled: Arc<AtomicBool>,
    previous: Mutex<Option<Vec<String>>>,
    owned: Mutex<Option<Arc<OwnedLease>>>,
}

impl Controller {
    fn owned(&self, path: &str) -> Result<Arc<OwnedLease>, String> {
        self.owned
            .lock()
            .unwrap()
            .as_ref()
            .filter(|lease| lease.path() == path)
            .cloned()
            .ok_or_else(|| "qca410_primary_mm_bearer_not_owned".to_string())
    }

    async fn run(self: Arc<Self>, args: Vec<String>) -> Result<String, String> {
        if args.len() != 3 {
            return Err("qca410_primary_mm_internal_operation_invalid".to_string());
        }
        match (args[0].as_str(), args[2].as_str()) {
            ("-m", "-K") if args[1] == self.bus.modem => {
                let port = self.bus.primary_port().await?;
                let bearers = self.bus.bearers().await?;
                let mut previous = self.previous.lock().unwrap();
                if previous.is_none() {
                    *previous = Some(bearers.clone());
                }
                Ok(format!(
                    "modem.generic.primary-port : {port}\nmodem.generic.bearers : {}",
                    bearers.join(", ")
                ))
            }
            ("-m", operation)
                if operation.starts_with("--create-bearer=") && args[1] == self.bus.modem =>
            {
                if args != create_args(&self.request.borrowed())?
                    || self.cancelled.load(Ordering::Acquire)
                    || lifecycle::is_shutting_down()
                {
                    return Err("qca410_primary_mm_setup_cancelled".to_string());
                }
                let previous =
                    self.previous.lock().unwrap().clone().ok_or_else(|| {
                        "qca410_primary_mm_missing_ownership_snapshot".to_string()
                    })?;
                let bearer = self.bus.create(&self.request.borrowed()).await?;
                if !previous.contains(&bearer) {
                    match OwnedLease::create(Arc::clone(&self.bus), &bearer) {
                        Ok(lease) => *self.owned.lock().unwrap() = Some(lease),
                        Err(error) => {
                            // The object is known and still disconnected. Do
                            // not activate if durable ownership cannot be saved.
                            let _ = self.bus.delete(&bearer).await;
                            return Err(error);
                        }
                    }
                }
                Ok(bearer)
            }
            ("-b", "--connect") => {
                let lease = self.owned(&args[1])?;
                if self.cancelled.load(Ordering::Acquire) || lifecycle::is_shutting_down() {
                    return Err("qca410_primary_mm_setup_cancelled".to_string());
                }
                let _connection_guard = lease.connection_will_start()?;
                self.bus.connect(&args[1]).await?;
                Ok("connected".to_string())
            }
            ("-b", "-K") => {
                let status = self.bus.status(&args[1]).await?;
                Ok(format!(
                    "bearer.status.connected : {}\nbearer.status.interface : {}\nbearer.properties.apn : {}",
                    if status.connected { "yes" } else { "no" },
                    status.interface,
                    status.apn
                ))
            }
            ("-b", "--disconnect") => {
                self.owned(&args[1])?.cleanup().await?;
                Ok("released".to_string())
            }
            ("-m", operation)
                if operation.starts_with("--delete-bearer=") && args[1] == self.bus.modem =>
            {
                let path = operation.trim_start_matches("--delete-bearer=");
                self.owned(path)?.cleanup().await?;
                Ok("released".to_string())
            }
            _ => Err("qca410_primary_mm_internal_operation_invalid".to_string()),
        }
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        let owned = self
            .owned
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(lease) = owned {
            lifecycle::cleanup_in_background(Arc::clone(lease));
        }
    }
}

fn value<'a>(output: &'a str, key: &str) -> Option<&'a str> {
    output.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| value.trim())
    })
}

fn connected(output: &str) -> Option<bool> {
    match value(output, "bearer.status.connected")? {
        "yes" | "true" => Some(true),
        "no" | "false" => Some(false),
        _ => None,
    }
}

fn bearer_paths(output: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for word in output.split_whitespace() {
        let path = word.trim_matches(['\'', '"', '[', ']', ',', ';']);
        if path
            .strip_prefix(BEARER_PREFIX)
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
            && !paths.iter().any(|existing| existing == path)
        {
            paths.push(path.to_string());
        }
    }
    paths
}

fn modem_bearer_paths(output: &str) -> Vec<String> {
    let mut paths = Vec::new();
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        // Initial EPS attachment is not a user-created Linux data bearer.
        // Only enumerate Modem.Bearers, not the separate initial-EPS object.
        if key == "modem.generic.bearers" || key.starts_with("modem.generic.bearers.") {
            for path in bearer_paths(value) {
                if !paths.contains(&path) {
                    paths.push(path);
                }
            }
        }
    }
    paths
}

fn create_args(request: &PrimaryImsRequest<'_>) -> Result<Vec<String>, String> {
    let family = match request.family {
        4 => "ipv4",
        6 => "ipv6",
        _ => return Err("qca410_primary_mm_requires_explicit_ip_family".to_string()),
    };
    let mut properties = format!(
        "apn={},ip-type={family},allow-roaming={}",
        request.apn,
        if request.allow_roaming { "yes" } else { "no" }
    );
    if let Some(profile) = request.profile_id {
        properties.push_str(&format!(",profile-id={profile}"));
    }
    Ok(vec![
        "-m".to_string(),
        request.modem.to_string(),
        format!("--create-bearer={properties}"),
    ])
}

fn status_args(bearer: &str) -> Vec<String> {
    vec!["-b".to_string(), bearer.to_string(), "-K".to_string()]
}

/// Injecting the operation runner keeps ownership regressions hardware-free.
/// These mmcli-style labels are test fixtures; production performs typed D-Bus
/// calls pinned to one unique MM owner, never shell/CLI lookups by a stale ID.
async fn prepare_with<F, R>(request: &PrimaryImsRequest<'_>, mut run: F) -> Result<String, String>
where
    F: FnMut(Vec<String>) -> R + Send,
    R: Future<Output = Result<String, String>> + Send,
{
    let create = create_args(request)?;
    if !request.device.starts_with("/dev/")
        || netdev::primary_netdev_for_qmi(request.device).as_deref() != Some(request.interface)
    {
        return Err("qca410_ims_requires_primary_qmi_proxy".to_string());
    }
    let modem = run(vec![
        "-m".to_string(),
        request.modem.to_string(),
        "-K".to_string(),
    ])
    .await?;
    // A line must never borrow another modem's primary endpoint, even when
    // ModemManager object numbers change after a SIM or modem re-enumeration.
    if value(&modem, "modem.generic.primary-port") != request.device.strip_prefix("/dev/") {
        return Err("qca410_primary_mm_control_endpoint_mismatch".to_string());
    }
    let previous = modem_bearer_paths(&modem);
    let created = run(create).await?;
    let paths = bearer_paths(&created);
    if paths.len() != 1 {
        return Err("qca410_primary_mm_created_bearer_path_missing".to_string());
    }
    let bearer = paths[0].clone();
    if previous.contains(&bearer) {
        // Some controllers may reuse a matching object. It is not ours merely
        // because CreateBearer returned it; do not connect, move or delete it.
        return Err("qca410_primary_mm_bearer_not_new".to_string());
    }
    let result: Result<(), String> = async {
        run(vec![
            "-b".to_string(),
            bearer.clone(),
            "--connect".to_string(),
        ])
        .await?;
        let status = run(status_args(&bearer)).await?;
        if connected(&status) != Some(true) {
            return Err("qca410_primary_mm_bearer_not_connected".to_string());
        }
        if value(&status, "bearer.status.interface") != Some(request.interface) {
            return Err("qca410_primary_mm_data_interface_mismatch".to_string());
        }
        if !value(&status, "bearer.properties.apn")
            .is_some_and(|apn| apn.eq_ignore_ascii_case(request.apn))
        {
            return Err("qca410_primary_mm_apn_mismatch".to_string());
        }

        // Only an exclusive, application-created IMS data interface can move
        // into a UE namespace. Never relabel an existing Internet bearer as
        // application-owned or disconnect another program's bearer to steal it.
        let listed = run(vec![
            "-m".to_string(),
            request.modem.to_string(),
            "-K".to_string(),
        ])
        .await?;
        for other in modem_bearer_paths(&listed)
            .into_iter()
            .filter(|path| path != &bearer)
        {
            let status = run(status_args(&other)).await?;
            let Some(is_connected) = connected(&status) else {
                return Err("qca410_primary_mm_other_bearer_status_unknown".to_string());
            };
            if is_connected && value(&status, "bearer.status.interface") == Some(request.interface)
            {
                return Err("qca410_primary_mm_interface_already_owned".to_string());
            }
        }
        Ok(())
    }
    .await;
    if let Err(error) = result {
        if let Err(cleanup_error) = release_with(request.modem, &bearer, &mut run).await {
            return Err(format!(
                "{error}:owned_bearer_cleanup_failed:{cleanup_error}"
            ));
        }
        return Err(error);
    }
    Ok(bearer)
}

async fn release_with<F, R>(modem: &str, bearer: &str, run: &mut F) -> Result<(), String>
where
    F: FnMut(Vec<String>) -> R + Send,
    R: Future<Output = Result<String, String>> + Send,
{
    // Disconnect may report "already disconnected". Always attempt deletion
    // of this exact owned object; never use a modem-wide Disconnect/Delete.
    let _ = run(vec![
        "-b".to_string(),
        bearer.to_string(),
        "--disconnect".to_string(),
    ])
    .await;
    match run(vec![
        "-m".to_string(),
        modem.to_string(),
        format!("--delete-bearer={bearer}"),
    ])
    .await
    {
        Ok(_) => Ok(()),
        Err(error) if error == OWNER_MISSING => Ok(()),
        Err(error) => Err(error),
    }
}

fn observed_loss(result: &Result<String, String>) -> Option<String> {
    match result {
        Ok(status) => match connected(status) {
            Some(true) => None,
            Some(false) => Some("qca410_primary_mm_bearer_disconnected".to_string()),
            None => Some("qca410_primary_mm_bearer_status_invalid".to_string()),
        },
        Err(error) if error == OWNER_MISSING => Some(error.clone()),
        // A failed status read is not proof that the WDS owner has gone away.
        Err(_) => None,
    }
}

pub(super) fn safe_error(error: &str) -> String {
    let lower = error.to_ascii_lowercase();
    if [
        "authorization",
        "username",
        "password",
        "secret",
        "cookie",
        "token",
        "nonce",
        "imsi",
        "iccid",
        "imei",
    ]
    .iter()
    .any(|word| lower.contains(word))
    {
        return "sensitive_details_omitted".to_string();
    }
    let mut result = String::new();
    let mut digits = String::new();
    let flush_digits = |result: &mut String, digits: &mut String| {
        if digits.len() >= 7 {
            result.push_str("[identifier-redacted]");
        } else {
            result.push_str(digits);
        }
        digits.clear();
    };
    let compact = error.split_whitespace().collect::<Vec<_>>().join(" ");
    for character in compact.chars().take(512) {
        if character.is_ascii_digit() {
            digits.push(character);
        } else {
            flush_digits(&mut result, &mut digits);
            result.push(character);
        }
    }
    flush_digits(&mut result, &mut digits);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::VecDeque,
        future::{ready, Ready},
        sync::{Arc, Mutex},
    };

    const MODEM: &str = "/org/freedesktop/ModemManager1/Modem/56";
    const BEARER: &str = "/org/freedesktop/ModemManager1/Bearer/45";
    const OTHER: &str = "/org/freedesktop/ModemManager1/Bearer/46";

    fn request() -> PrimaryImsRequest<'static> {
        PrimaryImsRequest {
            device: "/dev/wwan0qmi0",
            modem: MODEM,
            interface: "wwan0",
            apn: "ims",
            profile_id: Some(2),
            family: 4,
            allow_roaming: true,
        }
    }

    #[tokio::test]
    async fn cancelled_receiver_releases_a_completed_setup() {
        let (sender, receiver) = oneshot::channel::<Result<u8, String>>();
        drop(receiver);
        let cleaned = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cleaned);
        deliver_setup(sender, Ok(7), move |resource| async move {
            assert_eq!(resource, 7);
            flag.store(true, Ordering::Release);
        })
        .await;
        assert!(cleaned.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn accepted_setup_keeps_its_resource_and_failed_setup_has_none_to_release() {
        let (sender, receiver) = oneshot::channel::<Result<u8, String>>();
        deliver_setup(sender, Ok(7), |_| async { panic!("still owned by caller") }).await;
        assert_eq!(receiver.await.unwrap().unwrap(), 7);
        let (sender, receiver) = oneshot::channel::<Result<u8, String>>();
        drop(receiver);
        deliver_setup(sender, Err("failed".to_string()), |_| async {
            panic!("no resource was created")
        })
        .await;
    }

    #[test]
    fn dropping_a_setup_waiter_marks_cancellation_before_connect() {
        let flag = Arc::new(AtomicBool::new(false));
        {
            let _guard = CancelSetup {
                flag: Arc::clone(&flag),
                armed: true,
            };
        }
        assert!(flag.load(Ordering::Acquire));
    }

    fn status(interface: &str, connected: bool) -> String {
        format!(
            "bearer.status.connected : {}\nbearer.status.interface : {interface}\nbearer.properties.apn : ims\n",
            if connected { "yes" } else { "no" }
        )
    }

    type Calls = Arc<Mutex<Vec<Vec<String>>>>;

    fn runner(
        replies: Vec<Result<String, String>>,
    ) -> (
        impl FnMut(Vec<String>) -> Ready<Result<String, String>> + Send,
        Calls,
    ) {
        let calls: Calls = Arc::default();
        let recorded = Arc::clone(&calls);
        let mut replies = VecDeque::from(replies);
        (
            move |args| {
                recorded.lock().unwrap().push(args);
                ready(replies.pop_front().expect("unexpected modem command"))
            },
            calls,
        )
    }

    fn successful_replies() -> Vec<Result<String, String>> {
        vec![
            Ok("modem.generic.primary-port : wwan0qmi0".to_string()),
            Ok(format!("Successfully created bearer: {BEARER}")),
            Ok("connected".to_string()),
            Ok(status("wwan0", true)),
            Ok(format!("modem.generic.bearers.value[1] : {BEARER}")),
        ]
    }

    #[test]
    fn create_keeps_explicit_family_profile_apn_and_roaming_policy() {
        for (family, label) in [(4, "ipv4"), (6, "ipv6")] {
            for allowed in [false, true] {
                let mut request = request();
                request.family = family;
                request.allow_roaming = allowed;
                request.apn = "ims.operator.example";
                let args = create_args(&request).unwrap();
                assert_eq!(args[0..2], ["-m", MODEM]);
                assert!(args[2].contains(&format!("ip-type={label},")));
                assert!(args[2].contains("apn=ims.operator.example,"));
                assert!(args[2].ends_with(",profile-id=2"));
                assert!(args[2].contains(if allowed {
                    "allow-roaming=yes"
                } else {
                    "allow-roaming=no"
                }));
                assert!(!args.iter().any(|arg| arg.contains("device-open")));
                assert!(!args.iter().any(|arg| arg.contains("bind-mux")));
            }
        }
        let mut invalid = request();
        invalid.family = 0;
        assert!(create_args(&invalid).is_err());
    }

    #[tokio::test]
    async fn primary_ims_is_created_connected_and_verified_before_namespace_use() {
        let (run, calls) = runner(successful_replies());
        assert_eq!(prepare_with(&request(), run).await.unwrap(), BEARER);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 5);
        assert_eq!(calls[0], ["-m", MODEM, "-K"]);
        assert_eq!(calls[2], ["-b", BEARER, "--connect"]);
        assert_eq!(calls[4], ["-m", MODEM, "-K"]);
    }

    #[tokio::test]
    async fn another_modem_primary_cannot_be_borrowed() {
        let (run, calls) = runner(vec![Ok(
            "modem.generic.primary-port : wwan1qmi0".to_string()
        )]);
        assert!(prepare_with(&request(), run)
            .await
            .unwrap_err()
            .contains("control_endpoint_mismatch"));
        assert_eq!(calls.lock().unwrap().len(), 1);
        let mut secondary = request();
        secondary.device = "/dev/wwan0at1";
        let (run, calls) = runner(vec![]);
        assert!(prepare_with(&secondary, run).await.is_err());
        assert!(calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connect_failure_releases_only_the_new_bearer() {
        let mut replies = successful_replies();
        replies.truncate(2);
        replies.extend([
            Err("ipv4-only-allowed".to_string()),
            Err("already disconnected".to_string()),
            Ok("deleted".to_string()),
        ]);
        let (run, calls) = runner(replies);
        assert_eq!(
            prepare_with(&request(), run).await.unwrap_err(),
            "ipv4-only-allowed"
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls[3], ["-b", BEARER, "--disconnect"]);
        assert_eq!(calls[4][2], format!("--delete-bearer={BEARER}"));
    }

    #[tokio::test]
    async fn existing_connected_primary_interface_is_never_stolen() {
        let mut replies = successful_replies();
        replies[4] = Ok(format!("modem.generic.bearers : {BEARER}, {OTHER}"));
        replies.extend([
            Ok(status("wwan0", true)),
            Ok("disconnected".to_string()),
            Ok("deleted".to_string()),
        ]);
        let (run, calls) = runner(replies);
        assert!(prepare_with(&request(), run)
            .await
            .unwrap_err()
            .contains("interface_already_owned"));
        let calls = calls.lock().unwrap();
        assert_eq!(calls[5], ["-b", OTHER, "-K"]);
        assert_eq!(calls[6], ["-b", BEARER, "--disconnect"]);
        assert_eq!(calls[7][2], format!("--delete-bearer={BEARER}"));
    }

    #[tokio::test]
    async fn unrelated_or_disconnected_bearer_does_not_block_owned_ims() {
        for other in [status("wwan0", false), status("wwan1", true)] {
            let mut replies = successful_replies();
            replies[4] = Ok(format!("modem.generic.bearers : {BEARER}, {OTHER}"));
            replies.push(Ok(other));
            let (run, _) = runner(replies);
            assert_eq!(prepare_with(&request(), run).await.unwrap(), BEARER);
        }
    }

    #[tokio::test]
    async fn interface_or_apn_mismatch_rolls_back_the_created_object() {
        for bad_status in [
            status("wwan1", true),
            status("wwan0", false),
            status("wwan0", true).replace("apn : ims", "apn : internet"),
        ] {
            let mut replies = successful_replies();
            replies.truncate(3);
            replies.extend([
                Ok(bad_status),
                Ok("disconnected".to_string()),
                Ok("deleted".to_string()),
            ]);
            let (run, calls) = runner(replies);
            assert!(prepare_with(&request(), run).await.is_err());
            assert_eq!(
                calls.lock().unwrap().last().unwrap()[2],
                format!("--delete-bearer={BEARER}")
            );
        }
    }

    #[tokio::test]
    async fn a_returned_existing_bearer_is_not_claimed_or_deleted() {
        let (run, calls) = runner(vec![
            Ok(format!(
                "modem.generic.primary-port : wwan0qmi0\nmodem.generic.bearers : {BEARER}"
            )),
            Ok(BEARER.to_string()),
        ]);
        assert!(prepare_with(&request(), run)
            .await
            .unwrap_err()
            .contains("bearer_not_new"));
        assert_eq!(calls.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn an_unreleased_owned_object_is_reported_not_hidden() {
        let mut replies = successful_replies();
        replies.truncate(2);
        replies.extend([
            Err("connect_failed".to_string()),
            Err("disconnect_failed".to_string()),
            Err("delete_failed".to_string()),
        ]);
        let (run, _) = runner(replies);
        let error = prepare_with(&request(), run).await.unwrap_err();
        assert!(error.contains("connect_failed"));
        assert!(error.contains("owned_bearer_cleanup_failed:delete_failed"));
    }

    #[test]
    fn loss_requires_disconnect_missing_owner_or_invalid_provider_state() {
        assert!(observed_loss(&Ok(status("wwan0", true))).is_none());
        assert!(observed_loss(&Ok(status("wwan0", false))).is_some());
        assert!(observed_loss(&Err(OWNER_MISSING.to_string())).is_some());
        assert!(observed_loss(&Err("qca410_primary_mm_command_timeout".to_string())).is_none());
        assert!(observed_loss(&Ok("unparseable status".to_string())).is_some());
    }

    #[test]
    fn only_real_bearer_object_paths_are_accepted() {
        assert_eq!(bearer_paths(&format!("{BEARER}\n'{BEARER}'")), [BEARER]);
        assert!(bearer_paths("/org/freedesktop/ModemManager1/Bearer/../9").is_empty());
        assert!(bearer_paths("/org/freedesktop/ModemManager1/Bearer/").is_empty());
    }

    #[test]
    fn modem_snapshot_excludes_initial_eps_attachment() {
        assert!(modem_bearer_paths("modem.generic.bearers : --").is_empty());
        let snapshot = format!(
            "modem.3gpp.initial-eps-bearer : {OTHER}\n\
             modem.generic.bearers.length : 1\n\
             modem.generic.bearers.value[1] : {BEARER}\n"
        );
        assert_eq!(modem_bearer_paths(&snapshot), [BEARER]);
    }

    #[test]
    fn errors_do_not_expose_subscriber_or_authentication_material() {
        assert_eq!(
            safe_error("Authorization: Digest nonce=private"),
            "sensitive_details_omitted"
        );
        assert!(!safe_error("failed for 123456789012345").contains("123456789012345"));
        assert_eq!(
            safe_error("ipv4-only-allowed (6,50)"),
            "ipv4-only-allowed (6,50)"
        );
    }
}
