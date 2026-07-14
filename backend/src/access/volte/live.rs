//! Live VoLTE IMS registration driver for the Qualcomm target.
//!
//! This layer wires the pure stage-B pieces together: ModemManager owns the
//! dedicated `ims` bearer, Linux owns IP routing/xfrm, the USIM owns AKA, and
//! the shared `ims::register` driver owns the SIP transaction sequence.

use std::{
    net::SocketAddr,
    sync::OnceLock,
    time::Duration,
};

use chrono::Utc;
use tokio::{process::Command, sync::Mutex};

use crate::{
    ims::{
        access::ImsChannel,
        context::{ImsRoute, SipTransport},
        register::{run_register, RegisterAuthenticator},
        ImsError,
    },
    infra::config::VolteConfig,
};

use super::{
    bearer::{
        configure_bearer_network, disconnect_bearer, ensure_ims_bearer, route_pcscf,
        BearerConnection, BearerRequest,
    },
    channel::VolteSipChannel,
    digest_aka,
    errors::{code, VolteError},
    identity,
    ipsec::{self, SecAgree, XfrmInstallPlan},
    pcscf::{discover_pcscf, pcscf_socket},
    runtime::{RegistrationMode, VoltePhase, VolteRuntime, VolteRuntimeStatus, VolteStage},
    sip::{self, ImsIdentity, RequestIds},
};

const MODEM_ID: &str = "0";
const QMI_PROXY_SOCKET: &str = "@qmi-proxy";
const QMI_DEVICE: &str = "/dev/wwan0qmi0";
const UIM_SLOT: u8 = 1;
const REGISTER_EXPIRES: u32 = 3600;
const SECURITY_CLIENT_PORT: u16 = 5062;
const SECURITY_SERVER_PORT: u16 = 5064;

static LIVE_SESSION: OnceLock<Mutex<Option<VolteLiveSession>>> = OnceLock::new();

struct VolteLiveSession {
    channel: VolteSipChannel,
    identity: ImsIdentity,
    bearer: BearerConnection,
    pcscf: SocketAddr,
    xfrm_plan: Option<XfrmInstallPlan>,
}

struct DeviceIdentity {
    ims: ImsIdentity,
}

struct PreparedAuth {
    authorization: String,
    security_client: Option<String>,
    security_verify: Option<String>,
    require_sec_agree: bool,
}

struct VolteRegisterAuthenticator {
    identity: ImsIdentity,
    ids: RequestIds,
    sip_instance: String,
    offered_security: String,
    route: ImsRoute,
    pending: Option<PreparedAuth>,
    mode: RegistrationMode,
    xfrm_plan: Option<XfrmInstallPlan>,
}

impl VolteRegisterAuthenticator {
    fn new(
        identity: ImsIdentity,
        ids: RequestIds,
        sip_instance: String,
        offered_security: String,
        route: ImsRoute,
    ) -> Self {
        Self {
            identity,
            ids,
            sip_instance,
            offered_security,
            route,
            pending: None,
            mode: RegistrationMode::None,
            xfrm_plan: None,
        }
    }
}

impl RegisterAuthenticator<VolteSipChannel> for VolteRegisterAuthenticator {
    async fn prepare_authenticated_channel(
        &mut self,
        challenge_response: &[u8],
        channel: &mut VolteSipChannel,
    ) -> Result<(), ImsError> {
        let challenge = parse_digest_challenge(challenge_response).map_err(to_ims_error)?;
        let aka_challenge = digest_aka::decode_aka_nonce(&challenge.nonce).map_err(to_ims_error)?;
        let aid = identity::resolve_usim_aid(None);
        let rand = aka_challenge.rand;
        let autn = aka_challenge.autn;
        let aka = tokio::task::spawn_blocking(move || {
            identity::run_usim_aka(
                QMI_PROXY_SOCKET,
                QMI_DEVICE,
                UIM_SLOT,
                &aid,
                &rand,
                &autn,
                2,
                Duration::from_secs(5),
                Duration::from_millis(300),
            )
        })
        .await
        .map_err(|_| ImsError::new(code::USIM_AKA_FAILED))?
        .map_err(to_ims_error)?;

        let request_uri = format!("sip:{}", self.identity.home_domain);
        if let Some(auts) = aka.auts.as_deref() {
            self.pending = Some(PreparedAuth {
                authorization: digest_aka::build_resync_authorization_header(
                    &challenge,
                    &self.identity.private_user,
                    &request_uri,
                    auts,
                ),
                security_client: Some(self.offered_security.clone()),
                security_verify: None,
                require_sec_agree: true,
            });
            self.route = channel.route();
            return Ok(());
        }

        let cnonce = sip::hex_token(8);
        let nc = "00000001";
        let proof = digest_aka::compute_aka_response(
            &self.identity.private_user,
            &challenge.realm,
            &aka,
            &challenge.algorithm,
            "REGISTER",
            &request_uri,
            &challenge.nonce,
            challenge.qop.as_deref(),
            &cnonce,
            nc,
        )
        .map_err(to_ims_error)?;
        let authorization = digest_aka::build_authorization_header(
            &challenge,
            &self.identity.private_user,
            &request_uri,
            &proof,
            &cnonce,
            nc,
        );

        let security_server = sip::header_values(challenge_response, "Security-Server")
            .into_iter()
            .find_map(|value| ipsec::parse_security_server(&value).ok().map(|sec| (sec, value)));
        if let Some((selected, verify)) = security_server {
            let route = channel.route();
            let plan = ipsec::build_install_plan(
                route.local_addr.ip(),
                route.pcscf_addr.ip(),
                &selected,
                &aka.ik,
            )
            .map_err(to_ims_error)?;
            ipsec::install_plan(&plan).map_err(to_ims_error)?;
            let protected_route = ImsRoute {
                local_addr: SocketAddr::new(route.local_addr.ip(), selected.port_c),
                pcscf_addr: SocketAddr::new(route.pcscf_addr.ip(), selected.port_s),
                transport: SipTransport::Udp,
            };
            if let Err(error) = channel.rebind(protected_route, Some(verify.clone())) {
                ipsec::uninstall_plan(&plan);
                return Err(error);
            }
            self.xfrm_plan = Some(plan);
            self.mode = RegistrationMode::Ipsec;
            self.pending = Some(PreparedAuth {
                authorization,
                security_client: Some(self.offered_security.clone()),
                security_verify: Some(verify),
                require_sec_agree: true,
            });
        } else {
            self.mode = RegistrationMode::Udp;
            self.pending = Some(PreparedAuth {
                authorization,
                security_client: None,
                security_verify: None,
                require_sec_agree: false,
            });
        }
        self.route = channel.route();
        Ok(())
    }

    async fn authenticated_request(
        &mut self,
        _challenge_response: &[u8],
        cseq: u32,
    ) -> Result<Vec<u8>, ImsError> {
        let prepared = self
            .pending
            .take()
            .ok_or(ImsError::new("volte_register_auth_not_prepared"))?;
        let mut ids = self.ids.clone();
        ids.cseq = cseq;
        Ok(sip::build_register_with_security_policy(
            &self.identity,
            &self.route,
            &ids,
            REGISTER_EXPIRES,
            Some(&prepared.authorization),
            prepared.security_client.as_deref(),
            prepared.security_verify.as_deref(),
            &self.sip_instance,
            prepared.require_sec_agree,
        ))
    }
}

/// Establish the dedicated IMS bearer and REGISTER session. This is serialized
/// by the runtime guard and is safe to call repeatedly.
pub async fn connect_live(
    runtime: &VolteRuntime,
    config: &VolteConfig,
) -> Result<VolteRuntimeStatus, VolteError> {
    if !config.feature_enabled || !config.connection_enabled {
        return Err(VolteError::new(code::RUNTIME_NOT_RUNNING));
    }
    let _advance = runtime.advance_guard().await;
    if LIVE_SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .await
        .is_some()
    {
        return Ok(runtime.status().await);
    }
    let generation = runtime.generation();
    runtime
        .update(|state| {
            state.phase = VoltePhase::Starting;
            state.stage = VolteStage::Starting;
            state.session_started_at = Some(now());
            state.last_error = None;
        })
        .await;

    match connect_inner(runtime, generation).await {
        Ok(session) => {
            let mode = if session.xfrm_plan.is_some() {
                RegistrationMode::Ipsec
            } else {
                RegistrationMode::Udp
            };
            let pcscf = session.pcscf.to_string();
            *LIVE_SESSION.get_or_init(|| Mutex::new(None)).lock().await = Some(session);
            runtime
                .update(|state| {
                    state.phase = VoltePhase::Registered;
                    state.stage = VolteStage::Registered;
                    state.registration_mode = mode;
                    state.pcscf = Some(pcscf);
                    state.registered_at = Some(now());
                    state.data_path_mode = Some("dedicated_ims_bearer".to_string());
                })
                .await;
            Ok(runtime.status().await)
        }
        Err(error) => {
            let message = error.to_string();
            runtime
                .update(|state| {
                    state.phase = VoltePhase::Degraded;
                    state.last_error = Some(message);
                    state.last_failure_at = Some(now());
                })
                .await;
            Err(error)
        }
    }
}

async fn connect_inner(
    runtime: &VolteRuntime,
    generation: u64,
) -> Result<VolteLiveSession, VolteError> {
    runtime.update(|state| state.stage = VolteStage::Identity).await;
    let device_identity = load_device_identity().await?;
    ensure_generation(runtime, generation)?;

    runtime.update(|state| state.stage = VolteStage::Bearer).await;
    let bearer = ensure_ims_bearer(MODEM_ID, &BearerRequest::default()).await?;
    let result = async {
        configure_bearer_network(&bearer).await?;
        ensure_generation(runtime, generation)?;
        runtime.update(|state| state.stage = VolteStage::Pcscf).await;
        let pcscf = discover_pcscf(&bearer.settings, &device_identity.ims.home_domain).await?;
        route_pcscf(&bearer, pcscf).await?;
        runtime
            .update(|state| {
                state.stage = VolteStage::RegisterIpsec;
                state.pcscf = Some(pcscf.to_string());
            })
            .await;

        let route = ImsRoute {
            local_addr: SocketAddr::new(bearer.local_addr()?, 0),
            pcscf_addr: pcscf_socket(pcscf),
            transport: SipTransport::Udp,
        };
        let mut channel = VolteSipChannel::bind(route, Some(&bearer.interface), None)
            .map_err(map_channel_error)?;
        let ids = RequestIds::fresh(1);
        let sip_instance = new_sip_instance();
        let offered = offered_security();
        let initial_authorization = digest_aka::build_initial_authorization_header(
            &device_identity.ims.private_user,
            &device_identity.ims.home_domain,
            &format!("sip:{}", device_identity.ims.home_domain),
        );
        let initial = sip::build_register_with_security_policy(
            &device_identity.ims,
            &channel.route(),
            &ids,
            REGISTER_EXPIRES,
            Some(&initial_authorization),
            Some(&offered),
            None,
            &sip_instance,
            true,
        );
        let mut authenticator = VolteRegisterAuthenticator::new(
            device_identity.ims.clone(),
            ids,
            sip_instance,
            offered,
            channel.route(),
        );
        let registration = run_register(&mut channel, &initial, &mut authenticator).await;
        if let Err(error) = registration {
            if let Some(plan) = authenticator.xfrm_plan.as_ref() {
                ipsec::uninstall_plan(plan);
            }
            return Err(map_register_error(error));
        }
        if authenticator.mode == RegistrationMode::Udp {
            runtime.update(|state| state.stage = VolteStage::RegisterUdp).await;
        }
        Ok(VolteLiveSession {
            channel,
            identity: device_identity.ims,
            bearer: bearer.clone(),
            pcscf: pcscf_socket(pcscf),
            xfrm_plan: authenticator.xfrm_plan,
        })
    }
    .await;
    if result.is_err() {
        disconnect_bearer(&bearer.path).await;
    }
    result
}

/// Tear down only resources owned by the current VoLTE session.
pub async fn disconnect_live(runtime: &VolteRuntime, reason: &str) -> VolteRuntimeStatus {
    let session = LIVE_SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .await
        .take();
    if let Some(session) = session {
        if let Some(plan) = session.xfrm_plan.as_ref() {
            ipsec::uninstall_plan(plan);
        }
        disconnect_bearer(&session.bearer.path).await;
    }
    runtime.reset_runtime(reason).await;
    runtime.status().await
}

fn parse_digest_challenge(frame: &[u8]) -> Result<digest_aka::DigestChallenge, VolteError> {
    if let Some(value) = sip::header_value(frame, "WWW-Authenticate") {
        return digest_aka::parse_digest_challenge(&value, false);
    }
    if let Some(value) = sip::header_value(frame, "Proxy-Authenticate") {
        return digest_aka::parse_digest_challenge(&value, true);
    }
    Err(VolteError::new(code::DIGEST_CHALLENGE_MISSING))
}

async fn load_device_identity() -> Result<DeviceIdentity, VolteError> {
    let modem = command_output("mmcli", &["-m", MODEM_ID, "--output-keyvalue"]).await?;
    let operator = key_value(&modem, "modem.3gpp.operator-code")
        .filter(|value| value.len() == 5 || value.len() == 6)
        .ok_or_else(|| VolteError::new(code::MM_IMSI_MISSING))?;
    let sim_path = key_value(&modem, "modem.generic.sim")
        .ok_or_else(|| VolteError::new(code::MM_IMSI_MISSING))?;
    let sim = command_output("mmcli", &["-i", &sim_path, "--output-keyvalue"]).await?;
    let imsi = key_value(&sim, "sim.properties.imsi")
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .ok_or_else(|| VolteError::new(code::IMSI_MISSING))?;
    let (mcc, mnc) = operator.split_at(3);
    Ok(DeviceIdentity {
        ims: identity::derive_identity(&imsi, mcc, mnc),
    })
}

async fn command_output(program: &str, args: &[&str]) -> Result<String, VolteError> {
    let output = Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|error| {
            VolteError::with_detail(code::COMMAND_SPAWN_FAILED, format!("{program}:{error}"))
        })?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(VolteError::with_detail(
            code::COMMAND_FAILED,
            format!("{program}:{}", output.status.code().unwrap_or(-1)),
        ))
    }
}

fn key_value(output: &str, key: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let (candidate, value) = line.split_once(':')?;
        (candidate.trim() == key).then(|| value.trim().to_string())
    })
}

fn offered_security() -> String {
    let spi = || {
        u32::from_str_radix(&sip::hex_token(4), 16)
            .ok()
            .filter(|value| *value != 0)
            .unwrap_or(1)
    };
    SecAgree {
        spi_c: spi(),
        spi_s: spi(),
        port_c: SECURITY_CLIENT_PORT,
        port_s: SECURITY_SERVER_PORT,
    }
    .security_client_value()
}

fn new_sip_instance() -> String {
    let token = sip::hex_token(16);
    format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &token[0..8],
        &token[8..12],
        &token[12..16],
        &token[16..20],
        &token[20..32]
    )
}

fn ensure_generation(runtime: &VolteRuntime, expected: u64) -> Result<(), VolteError> {
    if runtime.generation() == expected {
        Ok(())
    } else {
        Err(VolteError::new(code::RUNTIME_NOT_RUNNING))
    }
}

fn now() -> String {
    Utc::now().to_rfc3339()
}

fn to_ims_error(error: VolteError) -> ImsError {
    ImsError::new(error.code())
}

fn map_channel_error(error: ImsError) -> VolteError {
    VolteError::with_detail(code::IPSEC_UDP_BIND_FAILED, error.code())
}

fn map_register_error(error: ImsError) -> VolteError {
    VolteError::with_detail(code::REGISTER_AUTH_UNEXPECTED_STATUS, error.code())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_identity_without_serializing_it() {
        let modem = "modem.generic.sim : /org/freedesktop/ModemManager1/SIM/0\nmodem.3gpp.operator-code : 46011\n";
        assert_eq!(
            key_value(modem, "modem.3gpp.operator-code").as_deref(),
            Some("46011")
        );
        assert!(!format!("{modem:?}").contains("460111234567890"));
    }

    #[test]
    fn security_offer_contains_nonzero_spis_and_expected_ports() {
        let offer = offered_security();
        let parsed = ipsec::parse_security_server(&offer).unwrap();
        assert_ne!(parsed.spi_c, 0);
        assert_ne!(parsed.spi_s, 0);
        assert_eq!(parsed.port_c, SECURITY_CLIENT_PORT);
        assert_eq!(parsed.port_s, SECURITY_SERVER_PORT);
    }

    #[test]
    fn digest_challenge_prefers_www_then_proxy() {
        let frame = b"SIP/2.0 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"ims.example\",nonce=\"YWJj\",algorithm=AKAv1-MD5\r\nContent-Length: 0\r\n\r\n";
        let challenge = parse_digest_challenge(frame).unwrap();
        assert_eq!(challenge.realm, "ims.example");
        assert!(!challenge.proxy);
    }
}
