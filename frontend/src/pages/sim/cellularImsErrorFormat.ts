import { CELLULAR_IMS_ERROR_CODES, type CellularImsErrorCode } from './cellularImsErrorCodes.ts'

const PROFILE_NOT_READY = /carrier_catalog_profile_not_ready:([^:]+):lte_epc:([^:]+)/
const PROFILE_PLMN = /(?:home_plmn|imsi_prefix):([0-9]{5,6}|unknown):access:lte_epc:no_ready_profile/
const IMS_SERVICE_NOT_SUBSCRIBED = /ServiceOptionNotSubscribed|service-option-not-subscribed|option-unsubscribed|Requested service option not subscribed/i
const SIP_STATUS = /sip_status=(\d{3})/i

// `last_error` is a `code[:detail]` chain, and a detail may embed further codes.
// Codes are matched as whole tokens, never as substrings: the backend table is
// guaranteed free of substring relations, so exact tokens route each failure to
// exactly one hint.
const KNOWN_CODES: ReadonlySet<string> = new Set(CELLULAR_IMS_ERROR_CODES)
const COLON_CODES = CELLULAR_IMS_ERROR_CODES.filter((code) => code.includes(':'))

// Diagnostics that are not table codes: `format!` prefixes and the shared
// connectivity-core code for an unanswered initial REGISTER.
const TRANSIENT_REFRESH_DIAGNOSTICS = [
  'volte_register_refresh_retry',
  'ims_register_initial_receive_failed',
] as const
const TRANSIENT_REFRESH_CODES: readonly CellularImsErrorCode[] = ['volte_register_refresh_receive_failed']

// Former `includes()` prefix families, listed member by member so a new code
// joins a family deliberately instead of by accident of spelling.
const DIGEST_FAILURES: readonly CellularImsErrorCode[] = [
  'volte_digest_algorithm_unsupported',
  'volte_digest_challenge_missing',
  'volte_digest_nonce_decode_failed',
  'volte_digest_nonce_missing',
  'volte_digest_qop_unsupported',
  'volte_digest_realm_missing',
  'volte_register_nonce_not_aka',
]
const IPSEC_FAILURES: readonly CellularImsErrorCode[] = [
  'volte_security_server_missing',
  'volte_ipsec_ik_invalid',
  'volte_ipsec_requires_ipv6',
  'volte_ipsec_udp_bind_failed',
]
const BEARER_NETDEV_FAILURES: readonly CellularImsErrorCode[] = [
  'volte_bearer_netdev_not_ready',
  'volte_bearer_netdev_not_up',
  'volte_bearer_netdev_runtime_error',
]

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
}

function tokensOf(error: string): ReadonlySet<string> {
  const tokens = new Set(error.match(/[a-z][a-z0-9_]*/g) ?? [])
  for (const code of COLON_CODES) {
    if (new RegExp(`(?:^|[^a-z0-9_])${escapeRegExp(code)}(?![a-z0-9_])`).test(error)) tokens.add(code)
  }
  return tokens
}

/** Every backend error code present in `error`, matched exactly. */
export function cellularImsErrorCodes(error: string): ReadonlySet<CellularImsErrorCode> {
  const found = new Set<CellularImsErrorCode>()
  for (const token of tokensOf(error)) {
    if (KNOWN_CODES.has(token)) found.add(token as CellularImsErrorCode)
  }
  return found
}

function hasAny(codes: ReadonlySet<string>, candidates: readonly string[]) {
  return candidates.some((code) => codes.has(code))
}

export function isTransientCellularImsRefreshDiagnostic(error?: string | null) {
  if (!error) return false
  const tokens = tokensOf(error)
  return hasAny(tokens, TRANSIENT_REFRESH_DIAGNOSTICS) || hasAny(tokens, TRANSIENT_REFRESH_CODES)
}

function networkFailureStatusLabel(error: string, codes: ReadonlySet<CellularImsErrorCode>) {
  if (IMS_SERVICE_NOT_SUBSCRIBED.test(error)) return '未订阅 IMS 服务'
  if (/operator-determined-barring|operator determined barring/i.test(error)) return '运营商禁止 IMS'
  if (codes.has('volte_runtime_mm_bearer_roaming_forbidden') || /roaming(?:-|\s)*(?:not-allowed|forbidden)|roaming not allowed/i.test(error)) return 'IMS 漫游被禁止'
  if (/missing-(?:or-)?unknown-apn|missing or unknown apn|unknown-apn/i.test(error)) return 'IMS APN 不可用'
  if (/ServiceOptionNotSupported|service-option-not-supported|option-not-supported/i.test(error)) return '运营商不支持 IMS'
  if (/service-option-out-of-order|option-out-of-order/i.test(error)) return 'IMS 服务暂不可用'
  if (/user-authentication(?:-|\s)*(?:failed|failure)|authentication-failed/i.test(error)) return 'IMS APN 鉴权失败'
  if (/insufficient-resources|max-active-pdp-context-reached/i.test(error)) return 'IMS 网络资源不足'
  if (/multiple-pdn-connections-not-allowed/i.test(error)) return 'IMS PDN 受限'
  if (/qos-not-accepted/i.test(error)) return 'IMS QoS 被拒绝'
  if (/(?:\[3gpp\]\s*)?network-failure/i.test(error)) return '运营商 IMS 故障'
  if (/unknown-pdp-address-or-type/i.test(error)) return 'IMS 地址类型被拒绝'
  if (/ipv4-only-allowed/i.test(error)) return '网络要求 IPv4 IMS'
  if (/ipv6-only-allowed/i.test(error)) return '网络要求 IPv6 IMS'
  if (/esm-proc-timeout/i.test(error)) return 'IMS Bearer 超时'
  if (/esm-lower-layer-failure/i.test(error)) return '蜂窝承载链路失败'

  const sipStatus = Number(error.match(SIP_STATUS)?.[1])
  if (!sipStatus) return null
  if (sipStatus === 403) return 'IMS 注册被拒绝（403）'
  if (sipStatus === 404) return 'IMS 用户不存在（404）'
  if (sipStatus === 408) return 'IMS 网络无响应（408）'
  if (sipStatus === 423) return '注册周期被拒绝（423）'
  if (sipStatus === 480) return 'IMS 服务暂不可用（480）'
  if (sipStatus === 488 || sipStatus === 494) return `IMS 安全协商失败（${sipStatus}）`
  if ([500, 502, 503, 504].includes(sipStatus)) return `运营商 IMS 故障（${sipStatus}）`
  if (sipStatus === 401 || sipStatus === 407) return `IMS 鉴权未完成（${sipStatus}）`
  if (sipStatus >= 400) return `IMS 注册被拒绝（${sipStatus}）`
  return null
}

function profileStatusLabel(status: string) {
  switch (status) {
    case 'partial': return '配置不完整'
    case 'unknown': return '尚无可信配置'
    case 'disabled': return '已停用'
    default: return status
  }
}

function plmnLabel(plmn: string) {
  if (plmn === 'unknown') return '未知 PLMN'
  if (['46000', '46002', '46004', '46007', '46008'].includes(plmn)) return `中国移动（PLMN ${plmn}）`
  if (['46001', '46006', '46009'].includes(plmn)) return `中国联通（PLMN ${plmn}）`
  if (['46003', '46005', '46011'].includes(plmn)) return `中国电信（PLMN ${plmn}）`
  if (plmn === '46015') return `中国广电（PLMN ${plmn}）`
  return `PLMN ${plmn}`
}

export function standardDerivedProfileMessage(
  source?: string | null,
  fallbackReason?: string | null,
) {
  if (source !== 'derived') return null

  const profileNotReady = fallbackReason?.match(PROFILE_NOT_READY)
  if (profileNotReady) {
    return `运营商数据库没有可用配置（已有条目${profileStatusLabel(profileNotReady[2])}），当前使用未经运营商验证的 3GPP 标准自动推断。`
  }
  if (fallbackReason?.includes('carrier_catalog_open_failed')) {
    return '运营商数据库无法读取，当前使用未经运营商验证的 3GPP 标准自动推断。'
  }
  return '运营商数据库没有可用配置，当前使用未经运营商验证的 3GPP 标准自动推断。'
}

export function cellularImsErrorMessage(error?: string | null) {
  if (!error) return null
  const codes = cellularImsErrorCodes(error)
  if (codes.has('volte_runtime_ims_baseband_wedged')) {
    return '设备后端报告基带状态异常，已停止本轮所有 Profile 和地址族重试。请先核对基带状态与承载归属，再安排受控恢复。'
  }
  if (isTransientCellularImsRefreshDiagnostic(error)) return null

  const profileNotReady = error.match(PROFILE_NOT_READY)
  if (profileNotReady) {
    return `SIM 身份已读取，但运营商 VoLTE profile 不可用（${profileStatusLabel(profileNotReady[2])}）。请更新 carrier catalog，或为当前线路导入经过验证的运营商配置。`
  }

  const profilePlmn = error.match(PROFILE_PLMN)
  if (profilePlmn) {
    return `SIM 身份已读取，但 ${plmnLabel(profilePlmn[1])} 没有可用的 VoLTE profile。请更新 carrier catalog，或为当前线路导入经过验证的运营商配置。`
  }

  if (error.includes('carrier_catalog_open_failed')) {
    return 'SIM 身份已读取，但运营商配置库无法打开。请在运营商 Profile 页面安装或重新安装 carrier catalog。'
  }
  if (error.includes('carrier_catalog_schema_') || error.includes('carrier_catalog_config_contract_')) {
    return 'SIM 身份已读取，但运营商配置库版本与当前程序不兼容。请更新 carrier catalog。'
  }
  if (codes.has('volte_carrier_profile_missing')) {
    return 'SIM 身份已读取，但未匹配到可用的运营商 VoLTE profile。请检查线路 Profile 或更新 carrier catalog。'
  }
  if (codes.has('volte_carrier_ims_apn_missing')) {
    return '已匹配运营商 Profile，但其中缺少 IMS APN，无法建立 VoLTE Bearer。'
  }
  if (codes.has('volte_runtime_cellular_network_not_registered')) {
    return '蜂窝网络未注册'
  }
  if (IMS_SERVICE_NOT_SUBSCRIBED.test(error)) {
    return '当前 SIM 未订阅 IMS 服务，或当前漫游网络不允许该 SIM 建立 IMS APN。该拒绝发生在运营商网络/套餐侧，尚未进入 P-CSCF、AKA 或 SIP REGISTER。'
  }
  if (codes.has('volte_mm_imsi_missing') || codes.has('volte_imsi_missing')) {
    return 'ModemManager SIM 属性与 AT+CIMI 均未返回有效 IMSI。请确认 SIM 已就绪，并检查基带 AT 端口状态。'
  }
  if (codes.has('volte_usim_aka_failed')) {
    return 'SIM 身份已读取，但 USIM AKA 鉴权失败。请检查 UIM 通道、卡槽映射和运营商鉴权响应。'
  }
  if (codes.has('volte_runtime_all_pcscf_failed')) {
    return 'IMS Bearer 已建立，但所有 P-CSCF 候选均连接失败。请检查运营商 Profile、PCO/DNS 返回和 IMS 路由。'
  }
  if (codes.has('volte_bearer_netdev_runtime_error')) {
    return 'IMS Bearer 已建立，但设备数据通道报告不可恢复错误。系统已停止继续重试；请检查基带日志或重启设备。'
  }
  if (codes.has('volte_bearer_netdev_not_up') || codes.has('volte_bearer_netdev_not_ready')) {
    return 'IMS Bearer 已建立，但其网卡没有完成 OPEN/UP 握手。系统已停止继续安装路由和重复重试，避免把底层链路故障误报成 P-CSCF 失败。'
  }
  return error
}

export function cellularImsErrorStatusLabel(error?: string | null) {
  if (!error) return null
  const codes = cellularImsErrorCodes(error)
  if (codes.has('volte_runtime_ims_baseband_wedged')) return '基带异常，已停止重试'
  if (isTransientCellularImsRefreshDiagnostic(error)) return null
  if (codes.has('volte_runtime_cellular_network_not_registered')) return '蜂窝网络未注册'
  const networkFailure = networkFailureStatusLabel(error, codes)
  if (networkFailure) return networkFailure
  if (codes.has('volte_carrier_profile_missing') || PROFILE_NOT_READY.test(error) || PROFILE_PLMN.test(error)) return '缺少运营商配置'
  if (codes.has('volte_carrier_ims_apn_missing')) return '缺少 IMS APN'
  if (codes.has('volte_mm_imsi_missing') || codes.has('volte_imsi_missing')) return '无法读取 SIM 身份'
  if (codes.has('volte_runtime_mm_modem_wait_timeout')) return '等待基带超时'
  if (codes.has('volte_runtime_ims_endpoint_unavailable')) return 'IMS 数据端口不可用'
  if (codes.has('volte_runtime_ims_bearer_start_failed')) return 'IMS Bearer 建立失败'
  if (codes.has('volte_runtime_ims_family_unsupported') || codes.has('volte_pcscf_family_mismatch')) return 'IMS 地址族不兼容'
  if (codes.has('volte_runtime_all_pcscf_failed')) return 'P-CSCF 不可达'
  if (codes.has('volte_usim_aka_failed') || codes.has('volte_aka_material_invalid') || codes.has('volte_aka_res_empty')) return 'SIM AKA 鉴权失败'
  if (hasAny(codes, DIGEST_FAILURES)) return 'IMS 鉴权响应异常'
  if (hasAny(codes, IPSEC_FAILURES)) return 'IMS IPsec 建立失败'
  if (hasAny(codes, BEARER_NETDEV_FAILURES)) return '基带数据通道异常'
  if (codes.has('volte_register_initial_unexpected_status')) return 'IMS 注册响应异常'
  if (codes.has('volte_register_auth_unexpected_status')) return 'IMS 鉴权注册失败'
  if (codes.has('volte_register_send_failed') || codes.has('volte_register_auth_send_failed')) return 'IMS 请求发送失败'
  return null
}
