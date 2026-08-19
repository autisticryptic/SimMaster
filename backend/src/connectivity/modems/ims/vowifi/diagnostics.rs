use serde::Serialize;

use crate::platform::db::{
    VowifiEsimRestoreEntry, VowifiRuntimeEventsResponse, VowifiRuntimeSnapshotEntry,
    VowifiSmsDeliveriesResponse, VowifiSoakRunsResponse,
};

use super::aka;
use super::dataplane;
use super::epdg;
use super::executor::RuntimeExecutorReport;
use super::flow::RuntimeFlowStatus;
use super::identity::{MaskedSimIdentity, VowifiSimIdentity};
use super::ike;
use super::ims;
use super::profiles::{self, CarrierProfile, CarrierProfileMeta};
use super::stability::{self, VowifiReadinessAuditReport};

#[derive(Debug, Clone, Serialize)]
pub struct PublicCarrierProfile {
    pub profile_id: &'static str,
    pub mcc: &'static str,
    pub mnc: &'static str,
    pub mnc_len: u8,
    pub plmn: &'static str,
    pub country_iso2: &'static str,
    pub brand: &'static str,
    pub operator_legal_name: &'static str,
    pub aliases: &'static [&'static str],
    pub source_refs: &'static [&'static str],
    pub last_verified: &'static str,
    pub supported: bool,
    pub support_stage: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct VowifiProfilesResponse {
    pub profiles: Vec<PublicCarrierProfile>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct VowifiProfileMatchResponse {
    pub matched: bool,
    pub matched_prefix: Option<String>,
    pub profile_source: Option<String>,
    pub profile_fallback_reason: Option<String>,
    pub profile: Option<PublicCarrierProfile>,
    pub sim_auth: Option<PublicAkaAdapterPlan>,
    pub epdg: Option<PublicEpdgPlan>,
    pub ike: Option<PublicIkePlan>,
    pub dataplane: Option<PublicDataplanePlan>,
    pub ims: Option<PublicImsPlan>,
    pub sim: MaskedSimIdentity,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicEpdgPlan {
    pub host: String,
    pub port: u16,
    pub ip_stack: String,
    pub apn: Option<String>,
    pub dns_server: Option<String>,
    pub route_kind: String,
    pub route_policy_id: String,
    pub route_note: String,
    pub source: String,
    pub proxy_endpoint_configured: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicIkeProposalPlan {
    pub proposal: &'static str,
    pub encryption: &'static str,
    pub integrity: &'static str,
    pub prf: &'static str,
    pub dh_group: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicEspProposalPlan {
    pub proposal: &'static str,
    pub encryption: &'static str,
    pub integrity: &'static str,
    pub mode: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicChildSaPlan {
    pub mode: &'static str,
    pub anti_replay_window: u16,
    pub mtu_strategy: &'static str,
    pub esp_proposals: Vec<PublicEspProposalPlan>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicIkePlan {
    pub exchange_phases: &'static [&'static str],
    pub aka_challenge_mode: &'static str,
    pub nat_keepalive_seconds: u16,
    pub dpd_interval_seconds: u16,
    pub reauth_interval_seconds: Option<u16>,
    pub retransmit_policy: &'static str,
    pub mobike_policy: &'static str,
    pub ike_proposals: Vec<PublicIkeProposalPlan>,
    pub child_sa: PublicChildSaPlan,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicSecAgreeMechanismPlan {
    pub mechanism: &'static str,
    pub integrity: &'static str,
    pub encryption: &'static str,
    pub protocol: &'static str,
    pub mode: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicImsPlan {
    pub domain: &'static str,
    pub realm: &'static str,
    pub registrar: Option<&'static str>,
    pub pcscf: Option<&'static str>,
    pub transport: &'static str,
    pub local_port: u16,
    pub user_agent_family: &'static str,
    pub identity_source: &'static str,
    pub supported_header: &'static str,
    pub include_pani_authenticated: bool,
    pub strict_security_server_offer: bool,
    pub enable_initial_reject_fallback: bool,
    pub security_client_mechanisms: Vec<PublicSecAgreeMechanismPlan>,
    pub sms_receiver_transport: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicDataplaneEspProposalPlan {
    pub proposal: &'static str,
    pub encryption: &'static str,
    pub integrity: &'static str,
    pub encapsulation: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicTrafficSelectorPlan {
    pub local_selector: &'static str,
    pub remote_selector: &'static str,
    pub address_assignment: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicSmoltcpGatewayPlan {
    pub stack: &'static str,
    pub gateway_mode: &'static str,
    pub ip_stack: &'static str,
    pub tcp_enabled: bool,
    pub udp_enabled: bool,
    pub icmp_enabled: bool,
    pub socket_policy: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicDataplanePlan {
    pub outer_encapsulation: &'static str,
    pub nat_t_port: u16,
    pub nat_keepalive_seconds: u16,
    pub anti_replay_window: u16,
    pub mtu_strategy: &'static str,
    pub mtu: dataplane::MtuPlan,
    pub traffic_selectors: PublicTrafficSelectorPlan,
    pub smoltcp: PublicSmoltcpGatewayPlan,
    pub esp_proposals: Vec<PublicDataplaneEspProposalPlan>,
    pub plaintext_capture_policy: &'static str,
    pub sensitive_values_policy: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicLogicalChannelPlan {
    pub application_priority: &'static [&'static str],
    pub channel_scope: &'static str,
    pub open_policy: &'static str,
    pub close_policy: &'static str,
    pub profile_switch_cleanup: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicAkaChallengePlan {
    pub method: &'static str,
    pub challenge_source: &'static str,
    pub resync_supported: bool,
    pub failure_mapping: &'static str,
    pub secret_values_policy: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct PublicAkaAdapterPlan {
    pub identity_source: &'static str,
    pub sim_access: &'static str,
    pub qmi_proxy_policy: &'static str,
    pub logical_channel: PublicLogicalChannelPlan,
    pub challenge: PublicAkaChallengePlan,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct VowifiReadiness {
    pub identity_ready: bool,
    pub sim_auth_ready: bool,
    pub profile_matched: bool,
    pub epdg_ready: bool,
    pub ike_ready: bool,
    pub child_sa_ready: bool,
    pub esp_ready: bool,
    pub ims_registered: bool,
    pub sms_ready: bool,
    pub voice_ready: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VowifiStatusResponse {
    pub phase: &'static str,
    pub dataplane_mode: &'static str,
    pub controlplane_mode: &'static str,
    pub readiness: VowifiReadiness,
    pub flow: RuntimeFlowStatus,
    pub executor: RuntimeExecutorReport,
    pub profile: VowifiProfileMatchResponse,
    pub degraded_reason: Option<String>,
    pub switch_phase: Option<String>,
    pub switch_token: Option<String>,
    pub phase_ms: Option<u64>,
    pub switch_identity_ready: bool,
    pub switch_sim_auth_ready: bool,
    pub switch_retry_count: u8,
}

impl Default for VowifiStatusResponse {
    fn default() -> Self {
        Self {
            phase: "not_started",
            dataplane_mode: "none",
            controlplane_mode: "scaffold_only",
            readiness: VowifiReadiness::default(),
            flow: super::flow::build_runtime_flow(&VowifiReadiness::default(), None, false),
            executor: RuntimeExecutorReport::default(),
            profile: VowifiProfileMatchResponse::default(),
            degraded_reason: None,
            switch_phase: None,
            switch_token: None,
            phase_ms: None,
            switch_identity_ready: false,
            switch_sim_auth_ready: false,
            switch_retry_count: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VowifiDiagnosticsSummary {
    pub runtime_phase: String,
    pub profile_id: Option<String>,
    pub plmn: Option<String>,
    pub ready_stage_count: usize,
    pub total_stage_count: usize,
    pub pending_sms_deliveries: usize,
    pub failed_sms_deliveries: usize,
    pub running_soak_runs: usize,
    pub failed_soak_runs: usize,
    pub last_event_at: Option<String>,
    pub active_trace_id: Option<String>,
    pub degraded: bool,
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VowifiDiagnosticsPrivacy {
    pub redaction_policy: &'static str,
    pub sensitive_fields_returned: bool,
    pub event_detail_policy: &'static str,
    pub trace_filter_policy: &'static str,
    pub action_interfaces_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VowifiDiagnosticsTimelineEntry {
    pub kind: String,
    pub timestamp: Option<String>,
    pub trace_id: Option<String>,
    pub level: String,
    pub phase: String,
    pub title: String,
    pub detail: String,
    pub state: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VowifiDiagnosticsResponse {
    pub line_id: Option<String>,
    pub status: VowifiStatusResponse,
    pub persisted_snapshot: Option<VowifiRuntimeSnapshotEntry>,
    pub events: VowifiRuntimeEventsResponse,
    pub sms_deliveries: VowifiSmsDeliveriesResponse,
    pub soak_runs: VowifiSoakRunsResponse,
    pub restore: Option<VowifiEsimRestoreEntry>,
    pub summary: VowifiDiagnosticsSummary,
    pub timeline: Vec<VowifiDiagnosticsTimelineEntry>,
    pub trace_filter: Option<String>,
    pub privacy: VowifiDiagnosticsPrivacy,
    pub m10_audit: VowifiReadinessAuditReport,
}
impl Default for VowifiDiagnosticsResponse {
    fn default() -> Self {
        Self {
            line_id: None,
            status: VowifiStatusResponse::default(),
            persisted_snapshot: None,
            events: VowifiRuntimeEventsResponse::default(),
            sms_deliveries: VowifiSmsDeliveriesResponse::default(),
            soak_runs: VowifiSoakRunsResponse::default(),
            restore: None,
            summary: VowifiDiagnosticsSummary {
                runtime_phase: "not_started".to_string(),
                profile_id: None,
                plmn: None,
                ready_stage_count: 0,
                total_stage_count: 0,
                pending_sms_deliveries: 0,
                failed_sms_deliveries: 0,
                running_soak_runs: 0,
                failed_soak_runs: 0,
                last_event_at: None,
                active_trace_id: None,
                degraded: false,
                read_only: true,
            },
            timeline: Vec::new(),
            trace_filter: None,
            privacy: VowifiDiagnosticsPrivacy {
                redaction_policy: "masked_identity_and_metadata_only",
                sensitive_fields_returned: false,
                event_detail_policy: "raw_event_detail_is_expected_to_be_redacted_by_writer",
                trace_filter_policy: "exact_trace_id_match_when_supplied",
                action_interfaces_enabled: false,
            },
            m10_audit: stability::build_readiness_audit_report(&RuntimeExecutorReport::default()),
        }
    }
}

pub fn build_diagnostics_response(
    line_id: Option<String>,
    status: VowifiStatusResponse,
    persisted_snapshot: Option<VowifiRuntimeSnapshotEntry>,
    events: VowifiRuntimeEventsResponse,
    sms_deliveries: VowifiSmsDeliveriesResponse,
    soak_runs: VowifiSoakRunsResponse,
    restore: Option<VowifiEsimRestoreEntry>,
    trace_filter: Option<String>,
) -> VowifiDiagnosticsResponse {
    let summary = diagnostics_summary(
        &status,
        &events,
        &sms_deliveries,
        &soak_runs,
        restore.as_ref(),
        &trace_filter,
    );
    let timeline = diagnostics_timeline(persisted_snapshot.as_ref(), &events, restore.as_ref());
    let m10_audit = stability::build_readiness_audit_report(&status.executor);

    VowifiDiagnosticsResponse {
        line_id,
        status,
        persisted_snapshot,
        events,
        sms_deliveries,
        soak_runs,
        restore,
        summary,
        timeline,
        trace_filter,
        privacy: VowifiDiagnosticsPrivacy {
            redaction_policy: "masked_identity_and_metadata_only",
            sensitive_fields_returned: false,
            event_detail_policy: "raw_event_detail_is_expected_to_be_redacted_by_writer",
            trace_filter_policy: "exact_trace_id_match_when_supplied",
            action_interfaces_enabled: false,
        },
        m10_audit,
    }
}

fn diagnostics_summary(
    status: &VowifiStatusResponse,
    events: &VowifiRuntimeEventsResponse,
    sms_deliveries: &VowifiSmsDeliveriesResponse,
    soak_runs: &VowifiSoakRunsResponse,
    restore: Option<&VowifiEsimRestoreEntry>,
    trace_filter: &Option<String>,
) -> VowifiDiagnosticsSummary {
    let profile = status.profile.profile.as_ref();
    let ready_stage_count = status
        .flow
        .steps
        .iter()
        .filter(|step| step.state == "done" || step.state == "ready")
        .count();
    let pending_sms_deliveries = sms_deliveries
        .deliveries
        .iter()
        .filter(|delivery| {
            matches!(
                delivery.state.as_str(),
                "queued" | "submitted" | "sip_accepted" | "pending"
            )
        })
        .count();
    let failed_sms_deliveries = sms_deliveries
        .deliveries
        .iter()
        .filter(|delivery| delivery.state == "failed" || delivery.failure_cause.is_some())
        .count();
    let running_soak_runs = soak_runs
        .runs
        .iter()
        .filter(|run| run.status == "running")
        .count();
    let failed_soak_runs = soak_runs
        .runs
        .iter()
        .filter(|run| run.status == "failed" || run.failure_count > 0)
        .count();
    let active_trace_id = trace_filter.clone().or_else(|| {
        events.events.iter().find_map(|event| {
            event
                .trace_id
                .as_ref()
                .filter(|trace| !trace.is_empty())
                .cloned()
        })
    });
    let degraded = status.degraded_reason.is_some()
        || restore
            .and_then(|restore| restore.degraded_reason.as_ref())
            .is_some();

    VowifiDiagnosticsSummary {
        runtime_phase: status.phase.to_string(),
        profile_id: profile.map(|profile| profile.profile_id.to_string()),
        plmn: profile.map(|profile| profile.plmn.to_string()),
        ready_stage_count,
        total_stage_count: status.flow.steps.len(),
        pending_sms_deliveries,
        failed_sms_deliveries,
        running_soak_runs,
        failed_soak_runs,
        last_event_at: events.events.first().map(|event| event.created_at.clone()),
        active_trace_id,
        degraded,
        read_only: true,
    }
}

fn diagnostics_timeline(
    _persisted_snapshot: Option<&VowifiRuntimeSnapshotEntry>,
    events: &VowifiRuntimeEventsResponse,
    _restore: Option<&VowifiEsimRestoreEntry>,
) -> Vec<VowifiDiagnosticsTimelineEntry> {
    let mut timeline = Vec::new();

    // ── Translated runtime events ──
    for event in &events.events {
        let (kind, title, detail) = translate_runtime_event(
            &event.event_type,
            &event.phase,
            event.profile_id.as_deref(),
            &event.detail_json,
        );
        timeline.push(VowifiDiagnosticsTimelineEntry {
            kind,
            timestamp: Some(event.created_at.clone()),
            trace_id: event.trace_id.clone(),
            level: event.level.clone(),
            phase: event.phase.clone(),
            title,
            detail,
            state: event.level.clone(),
        });
    }

    timeline
}

/// Maps a technical `event_type` + `phase` pair to a user‐facing
/// (kind_tag, title, detail) triple following the reference UI in
/// `docs/ui/wifi_calling_ui.html`.
fn translate_runtime_event(
    event_type: &str,
    phase: &str,
    profile_id: Option<&str>,
    detail_json: &str,
) -> (String, String, String) {
    let reason = serde_json::from_str::<serde_json::Value>(detail_json)
        .ok()
        .and_then(|v| {
            v.get("reason")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string())
        });

    match event_type {
        // ── SIM / Identity ──────────────────────────────────────
        "identity_refresh" => match phase {
            "identity_ready" | "profile_matched" => (
                "IMSI".into(),
                "载入卡片物理标识完成".into(),
                profile_id
                    .map(|id| format!("已匹配运营商配置模板 ({id})。"))
                    .unwrap_or_else(|| "已读取 PLMN 码和运营商归属信息。".into()),
            ),
            _ => (
                "IMSI".into(),
                "读取卡片物理存储区 (USIM EF_IMSI)".into(),
                "正在读取 SIM 卡物理标识...".into(),
            ),
        },
        "identity_failed" | "identity_timeout" => {
            if let Some(ref r) = reason {
                match r.as_str() {
                    "live_network_executor_disabled" => (
                        "IMSI".into(),
                        "安全门控拦截：未启用网络访问授权".into(),
                        "当前配置禁用了实时网络拨号授权。请在服务配置或环境变量中开启 SIMADMIN_VOWIFI_LIVE_NETWORK_ALLOWED = 1。".into(),
                    ),
                    "device_state_change_executor_disabled" => (
                        "IMSI".into(),
                        "安全门控拦截：未启用状态修改授权".into(),
                        "当前配置禁用了实时设备状态修改授权。请在服务配置或环境变量中开启 SIMADMIN_VOWIFI_DEVICE_CHANGES_ALLOWED = 1。".into(),
                    ),
                    "sim_auth_logical_channel_failed" => (
                        "IMSI".into(),
                        "分配 USIM 逻辑通道失败".into(),
                        "无法为 USIM 鉴权分配逻辑通道。原因可能是基带辅助逻辑通道数被占满，请尝试重启设备或调制解调器。".into(),
                    ),
                    "sim_auth_platform_unsupported" => (
                        "IMSI".into(),
                        "平台不支持物理卡鉴权".into(),
                        "当前运行平台不支持 QMI 物理卡鉴权交互（仅在 Linux/QMI 模式下支持）。".into(),
                    ),
                    "sim_auth_proxy_connect_failed" => (
                        "IMSI".into(),
                        "连接 QMI Proxy 代理失败".into(),
                        "无法连接到 QMI 代理套接字。请检查 qmi-proxy 服务是否正在运行。".into(),
                    ),
                    "sim_auth_proxy_open_failed" => (
                        "IMSI".into(),
                        "打开 QMI 设备节点失败".into(),
                        "无法通过 QMI 代理打开指定的调制解调器设备节点。请确认设备路径正确且具有访问权限。".into(),
                    ),
                    "sim_auth_uim_client_failed" => (
                        "IMSI".into(),
                        "初始化 UIM 客户端失败".into(),
                        "初始化 QMI UIM 服务客户端失败，Modem 可能处于未就绪状态。".into(),
                    ),
                    "sim_auth_apdu_build_failed" | "sim_auth_apdu_exchange_failed" | "sim_auth_runtime_failed" => (
                        "IMSI".into(),
                        "USIM 物理卡鉴权交互失败".into(),
                        "向 SIM 卡发送 AKA 鉴权 APDU 指令失败。请确认 SIM 卡是否松动、损坏，或者是否支持 USIM AKA 鉴权算法。".into(),
                    ),
                    _ => (
                        "IMSI".into(),
                        "物理卡识别或鉴权失败".into(),
                        format!("读取物理区失败，错误原因: {}", r),
                    )
                }
            } else {
                (
                    "IMSI".into(),
                    "未检测到物理 SIM 卡或 eSIM 卡".into(),
                    "读取物理区失败，USIM 物理接口层评估超时。".into(),
                )
            }
        }

        // ── Profile Matching ────────────────────────────────────
        "profile_match" | "profile_matched" => (
            "PROFILE".into(),
            format!("匹配配置模板: ID={}", profile_id.unwrap_or("unknown")),
            "ePDG 安全接入网关与 IMS 域载入成功。".into(),
        ),
        "profile_search" | "profile_lookup" => (
            "PROFILE".into(),
            "检索内置归属网络运营商映射元数据库".into(),
            "正在检索运营商互操作参数...".into(),
        ),
        "profile_not_found" | "profile_unsupported" => (
            "PROFILE".into(),
            "PLMN 未匹配到对应的 IKEv2 / IMS 接入参数".into(),
            "当前运营商暂不支持 WiFi Calling。".into(),
        ),

        // ── DNS ─────────────────────────────────────────────────
        "dns_resolve" | "dns_lookup" => (
            "DNS".into(),
            "解析运营商接入边界域名".into(),
            "正在解析 ePDG 运营商网关域名...".into(),
        ),
        "dns_resolved" | "dns_success" => (
            "DNS".into(),
            "DNS 解析成功".into(),
            "已成功解析 ePDG 网关 IP 地址。".into(),
        ),
        "dns_failed" | "dns_timeout" => {
            if let Some(ref r) = reason {
                match r.as_str() {
                    "epdg_dns_resolution_failed" => (
                        "DNS".into(),
                        "DNS 解析失败".into(),
                        "解析运营商 ePDG 域名失败，当前网络可能无法访问公网，或者 DNS 服务器不支持解析运营商专用域名。".into(),
                    ),
                    "epdg_dns_resolution_timeout" => (
                        "DNS".into(),
                        "DNS 解析超时".into(),
                        "解析运营商 ePDG 域名超时，请检查网络连接或更换更稳定的 DNS 服务器。".into(),
                    ),
                    "epdg_no_address" => (
                        "DNS".into(),
                        "未解析到有效网关 IP".into(),
                        "域名解析成功，但未返回任何可用的 ePDG 网关 IPv4/IPv6 地址。".into(),
                    ),
                    _ => (
                        "DNS".into(),
                        "DNS 解析失败".into(),
                        format!("无法解析运营商 ePDG 域名，错误原因: {}", r),
                    )
                }
            } else {
                (
                    "DNS".into(),
                    "DNS 解析失败".into(),
                    "无法解析运营商 ePDG 域名，请检查网络连接。".into(),
                )
            }
        }

        // ── IPsec / IKEv2 / Tunnel ─────────────────────────────
        "ike_sa_init" | "sa_init_request_built" => (
            "IPSEC".into(),
            "发送 IKE_SA_INIT 请求".into(),
            "正在与运营商网关协商 IKEv2 安全关联...".into(),
        ),
        "sa_init_response_accepted" => (
            "IPSEC".into(),
            "IKE_SA_INIT 响应已接受".into(),
            "已接收并验证网关 SA_INIT 响应报文。".into(),
        ),
        "auth_eap_start_request_built" | "auth_eap_start_packet_built" => (
            "IPSEC".into(),
            "发起 EAP-AKA 交互鉴权".into(),
            "正在通过 EAP-AKA 完成物理卡鉴权...".into(),
        ),
        "ike_auth" | "ike_auth_complete" | "ike_established" => (
            "IPSEC".into(),
            "建立 IKEv2 安全关联".into(),
            "通过 EAP-AKA 交互完成物理卡鉴权。".into(),
        ),
        "child_sa_established" | "child_sa_ready" => (
            "IPSEC".into(),
            "IPsec Child SA 建立成功".into(),
            "已协商 IPsec ESP 子安全关联参数。".into(),
        ),
        "esp_tunnel_up" | "esp_ready" | "userspace_esp_ready" | "tunnel_ready" => (
            "IPSEC".into(),
            "IPsec ESP 加密隧道建立成功".into(),
            "安全通道已就绪，数据面加密保护已启用。".into(),
        ),
        "ike_timeout" | "ike_failed" | "tunnel_failed" => {
            if let Some(ref r) = reason {
                match r.as_str() {
                    "ike_sa_init_response_rejected" => (
                        "IPSEC".into(),
                        "IKE_SA_INIT 网关拒绝连接".into(),
                        "网关拒绝了初始安全关联请求。请确认运营商是否封禁了您的 IP 地址，或当前网络是否封锁了 UDP 500 端口。".into(),
                    ),
                    "ike_auth_notify_authentication_failed" => (
                        "IPSEC".into(),
                        "EAP-AKA 身份鉴权被网关拒绝".into(),
                        "运营商网关拒绝了您的 SIM 卡鉴权。排查建议：请确认当前 IP 是否与该卡归属国一致，或确认该卡是否已开通并启用了 VoWiFi 业务。".into(),
                    ),
                    "ike_auth_final_request_build_failed" | "eap_aka_success_not_reached" => (
                        "IPSEC".into(),
                        "EAP-AKA 身份鉴权未通过".into(),
                        "SIM 卡 EAP-AKA 鉴权凭证被运营商网关拒绝。可能原因：当前卡未开通 VoWiFi 服务、套餐封禁、或者基带中固定拨号（FDN/PIN2）导致鉴权失效。".into(),
                    ),
                    "eap_aka_msk_unavailable" => (
                        "IPSEC".into(),
                        "AKA 主会话密钥生成失败".into(),
                        "鉴权虽然成功，但无法派生主会话密钥 (MSK)，无法建立后续安全隧道。".into(),
                    ),
                    "ike_dh_shared_secret_failed" | "ike_session_key_derivation_failed" => (
                        "IPSEC".into(),
                        "IKE 密钥协商派生失败".into(),
                        "Diffie-Hellman 共享密钥计算或会话密钥派生失败。".into(),
                    ),
                    "ike_sa_init_timeout" | "ike_auth_challenge_timeout" => (
                        "IPSEC".into(),
                        "IKE 协商握手超时".into(),
                        "未收到网关响应，请检查上游网络连接，确认是否已开启代理，并确保网络未阻断 UDP 500/4500 端口。".into(),
                    ),
                    _ => (
                        "IPSEC".into(),
                        "IKEv2 协商失败".into(),
                        format!("与网关的安全协商失败，错误原因: {}", r),
                    )
                }
            } else {
                (
                    "IPSEC".into(),
                    "发送 IKE_SA_INIT 后未收到响应报文".into(),
                    "重试后超时，运营商边界防火墙可能封禁了 UDP 500/4500 端口。".into(),
                )
            }
        }
        "ike_teardown" | "tunnel_teardown" | "ike_informational" => (
            "IPSEC".into(),
            "发送 IKEv2 INFORMATIONAL 报文，拆除全部 ESP 安全关联并注销会话。".into(),
            "".into(),
        ),

        // ── IMS / SIP ───────────────────────────────────────────
        "ims_register" | "sip_register" => (
            "SIP".into(),
            "发起 SIP REGISTER 注册申请".into(),
            "正在连接核心网 P-CSCF 并注册 IMS 会话...".into(),
        ),
        "ims_registered" | "ims_register_ok" | "sip_200_ok" => (
            "SIP".into(),
            "核心网响应: SIP/2.0 200 OK".into(),
            "IMS 会话就绪，已分配 SIP 接入关联路由。".into(),
        ),
        "ims_register_rejected" | "ims_403" | "sip_403" => {
            if let Some(ref r) = reason {
                match r.as_str() {
                    "ims_register_auth_rejected" => (
                        "SIP".into(),
                        "IMS 注册鉴权失败".into(),
                        "SIP 注册时鉴权响应被拒绝。请确保您的 SIM 卡具备合法的 VoWiFi 权限，并联系运营商开通此项服务。".into(),
                    ),
                    "ims_register_response_parse_failed" => (
                        "SIP".into(),
                        "IMS 注册响应解析失败".into(),
                        "无法解析来自 IMS 核心网的 SIP 响应报文，可能协议版本不兼容。".into(),
                    ),
                    "ims_register_unexpected_status" => (
                        "SIP".into(),
                        "IMS 注册返回异常状态".into(),
                        "IMS 注册请求被运营商返回了非预期的 SIP 错误状态码。".into(),
                    ),
                    _ => (
                        "SIP".into(),
                        "IMS 注册失败".into(),
                        format!("运营商 SIP 核心网拒绝了注册请求，错误原因: {}", r),
                    )
                }
            } else {
                (
                    "SIP".into(),
                    "SIP/2.0 403 Forbidden".into(),
                    "IMS 鉴权未通过 (AKA response mismatch)。".into(),
                )
            }
        }
        "ims_deregistered" | "ims_unregister" => (
            "SIP".into(),
            "IMS 会话已注销".into(),
            "已向核心网发送 SIP 注销请求。".into(),
        ),

        // ── SMS over IMS ────────────────────────────────────────
        "sms_binding" | "sms_negotiation" | "sms_activate" => (
            "SMS".into(),
            "发起 3GPP SMS over IMS 短信传输层接口协商".into(),
            "正在绑定短信传输层会话...".into(),
        ),
        "sms_ready" | "sms_bound" | "sms_activated" => (
            "SMS".into(),
            "SMS over IMS 接口鉴权完成".into(),
            "短信通路绑定就绪，蜂窝网短信路由切出。".into(),
        ),
        "sms_binding_failed" | "sms_binding_timeout" => (
            "SMS".into(),
            "SMS over IMS 接口绑定响应超时".into(),
            "短信链路映射失败，请尝试重新连接。".into(),
        ),
        "sms_path_released" | "sms_deactivated" => (
            "SMS".into(),
            "短信路径已释放，成功退回到蜂窝基站数据链路层。".into(),
            "".into(),
        ),

        // ── Voice over IMS ──────────────────────────────────────
        "voice_binding" | "voice_negotiation" | "voice_activate" => (
            "VOICE".into(),
            "发起 3GPP MMTel 语音承载能力协商".into(),
            "正在校验 IMS 注册与语音腿可用性...".into(),
        ),
        "voice_ready" | "voice_bound" | "voice_activated" => (
            "VOICE".into(),
            "语音 over IMS 能力就绪".into(),
            "语音承载已就绪，可发起 VoWiFi 通话。".into(),
        ),
        "voice_binding_failed" | "voice_binding_timeout" => (
            "VOICE".into(),
            "语音 over IMS 能力校验失败".into(),
            "语音承载未就绪，请尝试重新连接。".into(),
        ),

        // ── SIM Auth ────────────────────────────────────────────
        "sim_auth_retry" | "sim_auth_gate" => (
            "IMSI".into(),
            "SIM 鉴权重试".into(),
            "正在重新执行 USIM 身份验证流程...".into(),
        ),
        "sim_auth_ready" | "sim_auth_passed" => (
            "IMSI".into(),
            "SIM 鉴权通过".into(),
            "USIM 身份验证成功，SIM Auth Gate 已就绪。".into(),
        ),

        // ── System Events ───────────────────────────────────────
        "runtime_start" | "connect_start" | "connection_start" => (
            "SYS".into(),
            "WiFi Calling 连接守护进程启动".into(),
            format!(
                "信令流程开始{}",
                profile_id
                    .map(|id| format!(" [Profile: {id}]"))
                    .unwrap_or_default()
            ),
        ),
        "runtime_stop" | "connection_stop" | "runtime_teardown" => (
            "SYS".into(),
            "WiFi Calling 核心服务运行时已停止。".into(),
            "".into(),
        ),
        "runtime_ready" | "connection_ready" | "all_ready" => (
            "SYS".into(),
            "WiFi Calling 信令流握手完全成功".into(),
            "链路切换工作就绪。".into(),
        ),
        "connection_failed" | "runtime_failed" => (
            "SYS".into(),
            "WiFi Calling 连接异常终止".into(),
            "已保持基站蜂窝网短信路由。".into(),
        ),
        "cellular_fallback" => (
            "SYS".into(),
            "WiFi Calling 连接尝试耗尽".into(),
            "回退到蜂窝数据链路。".into(),
        ),

        // ── Fallback: unrecognised event types ──────────────────
        other => {
            let kind = classify_event_kind(other, phase);
            let title = other.replace('_', " ");
            let detail = profile_id
                .map(|id| format!("profile_id={id}"))
                .unwrap_or_else(|| "detail_redacted".into());
            (kind, title, detail)
        }
    }
}

/// Best‐effort tag classification for unknown event types based on
/// naming convention and phase context.
fn classify_event_kind(event_type: &str, phase: &str) -> String {
    let lower = event_type.to_ascii_lowercase();
    if lower.contains("imsi") || lower.contains("identity") || lower.contains("sim") {
        return "IMSI".into();
    }
    if lower.contains("profile") {
        return "PROFILE".into();
    }
    if lower.contains("dns") {
        return "DNS".into();
    }
    if lower.contains("ike")
        || lower.contains("ipsec")
        || lower.contains("esp")
        || lower.contains("tunnel")
        || lower.contains("sa_")
        || lower.contains("child_sa")
    {
        return "IPSEC".into();
    }
    if lower.contains("ims") || lower.contains("sip") || lower.contains("register") {
        return "SIP".into();
    }
    if lower.contains("sms") {
        return "SMS".into();
    }
    // Phase-based fallback
    let phase_lower = phase.to_ascii_lowercase();
    if phase_lower.contains("identity") || phase_lower.contains("sim") {
        return "IMSI".into();
    }
    if phase_lower.contains("profile") {
        return "PROFILE".into();
    }
    if phase_lower.contains("epdg") || phase_lower.contains("ike") || phase_lower.contains("esp") {
        return "IPSEC".into();
    }
    if phase_lower.contains("ims") || phase_lower.contains("register") {
        return "SIP".into();
    }
    if phase_lower.contains("sms") {
        return "SMS".into();
    }
    "SYS".into()
}

impl PublicCarrierProfile {
    pub fn from_profile(profile: &'static CarrierProfile) -> Self {
        Self::from_meta(&profile.meta)
    }

    fn from_meta(meta: &'static CarrierProfileMeta) -> Self {
        Self {
            profile_id: meta.profile_id,
            mcc: meta.mcc,
            mnc: meta.mnc,
            mnc_len: meta.mnc_len,
            plmn: meta.plmn,
            country_iso2: meta.country_iso2,
            brand: meta.brand,
            operator_legal_name: meta.operator_legal_name,
            aliases: meta.aliases,
            source_refs: meta.source_refs,
            last_verified: meta.last_verified,
            supported: true,
            support_stage: "profile_registry",
        }
    }
}

impl PublicEpdgPlan {
    pub fn from_profile(profile: &'static CarrierProfile) -> Self {
        let plan = epdg::build_connection_plan(profile, None);
        Self {
            host: plan.host.to_string(),
            port: plan.port,
            ip_stack: plan.ip_stack.to_string(),
            apn: plan.apn.map(str::to_string),
            dns_server: plan.dns_server.map(str::to_string),
            route_kind: plan.route_policy.kind.as_str().to_string(),
            route_policy_id: plan.route_policy.policy_id.to_string(),
            route_note: plan.route_policy.note.to_string(),
            source: "carrier_profile".to_string(),
            proxy_endpoint_configured: false,
        }
    }
}

impl PublicIkePlan {
    pub fn from_profile(profile: &'static CarrierProfile) -> Self {
        let plan = ike::build_session_plan(profile);
        Self {
            exchange_phases: plan.exchange_phases,
            aka_challenge_mode: plan.aka_challenge_mode,
            nat_keepalive_seconds: plan.nat_keepalive_seconds,
            dpd_interval_seconds: plan.dpd_interval_seconds,
            reauth_interval_seconds: plan.reauth_interval_seconds,
            retransmit_policy: plan.retransmit_policy,
            mobike_policy: plan.mobike_policy,
            ike_proposals: plan
                .ike_proposals
                .into_iter()
                .map(|proposal| PublicIkeProposalPlan {
                    proposal: proposal.proposal,
                    encryption: proposal.encryption,
                    integrity: proposal.integrity,
                    prf: proposal.prf,
                    dh_group: proposal.dh_group,
                })
                .collect(),
            child_sa: PublicChildSaPlan {
                mode: plan.child_sa.mode,
                anti_replay_window: plan.child_sa.anti_replay_window,
                mtu_strategy: plan.child_sa.mtu_strategy,
                esp_proposals: plan
                    .child_sa
                    .esp_proposals
                    .into_iter()
                    .map(|proposal| PublicEspProposalPlan {
                        proposal: proposal.proposal,
                        encryption: proposal.encryption,
                        integrity: proposal.integrity,
                        mode: proposal.mode,
                    })
                    .collect(),
            },
            sensitive_values_policy: plan.sensitive_values_policy,
        }
    }
}

impl PublicImsPlan {
    pub fn from_profile(profile: &'static CarrierProfile) -> Self {
        let plan = ims::build_register_plan(profile);
        Self {
            domain: plan.domain,
            realm: plan.realm,
            registrar: plan.registrar,
            pcscf: plan.pcscf,
            transport: plan.transport,
            local_port: plan.local_port,
            user_agent_family: plan.user_agent_family,
            identity_source: plan.identity_source,
            supported_header: plan.supported_header,
            include_pani_authenticated: plan.include_pani_authenticated,
            strict_security_server_offer: plan.strict_security_server_offer,
            enable_initial_reject_fallback: plan.enable_initial_reject_fallback,
            security_client_mechanisms: plan
                .security_client_mechanisms
                .into_iter()
                .map(|mechanism| PublicSecAgreeMechanismPlan {
                    mechanism: mechanism.mechanism,
                    integrity: mechanism.integrity,
                    encryption: mechanism.encryption,
                    protocol: mechanism.protocol,
                    mode: mechanism.mode,
                })
                .collect(),
            sms_receiver_transport: plan.sms_receiver_transport,
            sensitive_values_policy: plan.sensitive_values_policy,
        }
    }
}

impl PublicDataplanePlan {
    pub fn from_profile(profile: &'static CarrierProfile) -> Self {
        let plan = dataplane::build_dataplane_plan(profile);
        Self {
            outer_encapsulation: plan.outer_encapsulation,
            nat_t_port: plan.nat_t_port,
            nat_keepalive_seconds: plan.nat_keepalive_seconds,
            anti_replay_window: plan.anti_replay_window,
            mtu_strategy: plan.mtu_strategy,
            mtu: plan.mtu,
            traffic_selectors: PublicTrafficSelectorPlan {
                local_selector: plan.traffic_selectors.local_selector,
                remote_selector: plan.traffic_selectors.remote_selector,
                address_assignment: plan.traffic_selectors.address_assignment,
            },
            smoltcp: PublicSmoltcpGatewayPlan {
                stack: plan.smoltcp.stack,
                gateway_mode: plan.smoltcp.gateway_mode,
                ip_stack: plan.smoltcp.ip_stack,
                tcp_enabled: plan.smoltcp.tcp_enabled,
                udp_enabled: plan.smoltcp.udp_enabled,
                icmp_enabled: plan.smoltcp.icmp_enabled,
                socket_policy: plan.smoltcp.socket_policy,
            },
            esp_proposals: plan
                .esp_proposals
                .into_iter()
                .map(|proposal| PublicDataplaneEspProposalPlan {
                    proposal: proposal.proposal,
                    encryption: proposal.encryption,
                    integrity: proposal.integrity,
                    encapsulation: proposal.encapsulation,
                })
                .collect(),
            plaintext_capture_policy: plan.plaintext_capture_policy,
            sensitive_values_policy: plan.sensitive_values_policy,
        }
    }
}

impl PublicAkaAdapterPlan {
    pub fn from_profile(profile: &'static CarrierProfile) -> Self {
        let plan = aka::build_adapter_plan(profile);
        Self {
            identity_source: plan.identity_source,
            sim_access: plan.sim_access,
            qmi_proxy_policy: plan.qmi_proxy_policy,
            logical_channel: PublicLogicalChannelPlan {
                application_priority: plan.logical_channel.application_priority,
                channel_scope: plan.logical_channel.channel_scope,
                open_policy: plan.logical_channel.open_policy,
                close_policy: plan.logical_channel.close_policy,
                profile_switch_cleanup: plan.logical_channel.profile_switch_cleanup,
            },
            challenge: PublicAkaChallengePlan {
                method: plan.challenge.method,
                challenge_source: plan.challenge.challenge_source,
                resync_supported: plan.challenge.resync_supported,
                failure_mapping: plan.challenge.failure_mapping,
                secret_values_policy: plan.challenge.secret_values_policy,
            },
            timeout_ms: plan.timeout_ms,
        }
    }
}

pub fn list_profiles() -> VowifiProfilesResponse {
    let profiles = profiles::published_database_profiles()
        .into_iter()
        .map(PublicCarrierProfile::from_profile)
        .collect::<Vec<_>>();
    VowifiProfilesResponse {
        count: profiles.len(),
        profiles,
    }
}

/// Build the full plan response for one resolved carrier profile.
fn matched_response(
    profile: &'static profiles::CarrierProfile,
    matched_prefix: String,
    sim: MaskedSimIdentity,
) -> VowifiProfileMatchResponse {
    let derived = profiles::is_standard_derived_profile(profile);
    VowifiProfileMatchResponse {
        matched: true,
        matched_prefix: Some(matched_prefix),
        profile_source: Some(if derived { "derived" } else { "database" }.to_string()),
        profile_fallback_reason: derived
            .then(|| "carrier_database_no_usable_profile:access:wifi_epdg".to_string()),
        profile: Some(PublicCarrierProfile::from_profile(profile)),
        sim_auth: Some(PublicAkaAdapterPlan::from_profile(profile)),
        epdg: Some(PublicEpdgPlan::from_profile(profile)),
        ike: Some(PublicIkePlan::from_profile(profile)),
        dataplane: Some(PublicDataplanePlan::from_profile(profile)),
        ims: Some(PublicImsPlan::from_profile(profile)),
        sim,
    }
}

fn match_profile_from_parts(
    sim: MaskedSimIdentity,
    pinned_profile_id: Option<&str>,
    imsi_for_matching: &str,
    operator_id_for_matching: &str,
) -> VowifiProfileMatchResponse {
    // A SIM-bound pin published into this line's connection snapshot is the
    // operator's explicit choice and wins over automatic matching. An invalid
    // pin remains unmatched so inferred settings never replace a user choice.
    if let Some(profile_id) = pinned_profile_id {
        if let Some(profile) = profiles::resolve_pinned_database_profile(profile_id) {
            return matched_response(profile, profile.meta.plmn.to_string(), sim);
        }
        return VowifiProfileMatchResponse {
            sim,
            ..Default::default()
        };
    }

    let operator_digits: String = operator_id_for_matching
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect();
    if operator_digits.len() == 5 || operator_digits.len() == 6 {
        if let Some(matched) =
            profiles::resolve_for_line(None, imsi_for_matching, Some(&operator_digits))
        {
            return matched_response(matched.profile, matched.matched_prefix, sim);
        }
    }

    if let Some(matched) = profiles::resolve_for_line(None, imsi_for_matching, None) {
        return matched_response(matched.profile, matched.matched_prefix, sim);
    }

    VowifiProfileMatchResponse {
        sim,
        ..Default::default()
    }
}

/// Match honoring the current SIM snapshot's pinned carrier `profile_id`.
/// `None` uses automatic identity matching.
pub fn match_profile_for_line(
    identity: &VowifiSimIdentity,
    pinned_profile_id: Option<&str>,
) -> VowifiProfileMatchResponse {
    match_profile_from_parts(
        identity.masked(),
        pinned_profile_id,
        identity.imsi(),
        identity.operator_id(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_two_digit_mnc_from_operator_id_without_losing_leading_zero() {
        let sim = MaskedSimIdentity {
            present: true,
            operator_id: "20404".to_string(),
            ..Default::default()
        };

        let matched = match_profile_from_parts(sim, None, "204041234567890", "20404");

        assert!(matched.matched);
        assert_eq!(
            matched.profile.as_ref().map(|profile| profile.profile_id),
            Some("nl_vodafone_20404")
        );
        assert_eq!(
            matched.profile.as_ref().map(|profile| profile.mnc),
            Some("04")
        );
        assert_eq!(
            matched.epdg.as_ref().map(|epdg| epdg.host.as_str()),
            Some("epdg.epc.mnc004.mcc204.pub.3gppnetwork.org")
        );
        assert_eq!(
            matched.ike.as_ref().map(|ike| ike.ike_proposals[0].prf),
            Some("sha512")
        );
        assert_eq!(
            matched.dataplane.as_ref().map(|plan| plan.smoltcp.stack),
            Some("smoltcp")
        );
        assert_eq!(
            matched.sim_auth.as_ref().map(|plan| plan.sim_access),
            Some("qmi_uim_first_at_csim_fallback")
        );
        assert_eq!(matched.ims.as_ref().map(|ims| ims.transport), Some("tcp"));
        assert_eq!(matched.matched_prefix.as_deref(), Some("20404"));
    }

    #[test]
    fn visited_network_operator_does_not_replace_cmlink_home_profile() {
        let sim = MaskedSimIdentity {
            present: true,
            operator_id: "46000".to_string(),
            ..Default::default()
        };

        let matched = match_profile_from_parts(sim, None, "234331234567890", "46000");

        assert!(matched.matched);
        assert_eq!(matched.matched_prefix.as_deref(), Some("23433"));
        assert_eq!(
            matched.profile.as_ref().map(|profile| profile.profile_id),
            Some("gb_ee_23433")
        );
    }

    #[test]
    fn missing_database_profile_is_exposed_as_standard_derived() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let sim = MaskedSimIdentity {
            present: true,
            ..Default::default()
        };

        let matched = match_profile_from_parts(sim, None, "460001234567890", "");

        assert!(matched.matched);
        assert_eq!(matched.profile_source.as_deref(), Some("derived"));
        assert_eq!(
            matched.profile_fallback_reason.as_deref(),
            Some("carrier_database_no_usable_profile:access:wifi_epdg")
        );
        assert_eq!(
            matched.profile.as_ref().map(|profile| profile.profile_id),
            Some("derived_3gpp_vowifi_46000")
        );
    }

    #[test]
    fn invalid_explicit_pin_does_not_use_standard_derivation() {
        let _resolver_guard = profiles::profile_resolver_test_guard();
        let response = match_profile_from_parts(
            MaskedSimIdentity::default(),
            Some("missing-explicit-profile"),
            "460001234567890",
            "46000",
        );

        assert!(!response.matched);
        assert!(response.profile_source.is_none());
    }
}
