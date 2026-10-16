import type { ImsBearerCapabilities, ImsBearerPolicy, EffectiveImsBearerPolicy, ImsFactSource, ImsIpsecAlgorithms, ImsRegistrationDiagnostics, NativeImsObservationStatus } from '../../api/contracts.ts'

const sources: Record<ImsFactSource, string> = {
  unknown: '未确认', database: '自定义数据库', carrier_catalog: '只读运营商库', derived: '3GPP 标准派生',
  sim_override: 'SIM 覆盖', isim: 'ISIM', usim: 'USIM', environment: '显式环境配置',
  bearer_pco: '承载 PCO', profile: '所选 profile', access_configuration: '接入网络配置',
  dns: '承载内 DNS', dhcp: '承载内 DHCP', runtime: '运行时观测',
}
const families: Record<string, string> = { ipv4: 'IPv4', ipv6: 'IPv6', ipv4v6: 'IPv4v6', unknown: '未确认' }
const identities: Record<string, string> = { isim: 'ISIM 卡内身份', usim_derived: 'USIM 标准派生', sim_override_derived: '用户身份覆盖后派生', unknown: '尚未确认' }
const transports: Record<string, string> = { udp: 'UDP', tcp: 'TCP', tls: 'TLS', unknown: '未确认' }
const procedures: Record<string, string> = { initial: '初次注册', refresh: '续期', unregister: '注销', registration_event: '注册事件', not_observed: '未记录' }
const scopes: Record<string, string> = { endpoint: '端点', bearer: '承载', binding: '注册绑定', authentication: '鉴权', negotiation: '注册协商', local: '本地处理', ownership: 'SIM/worker 归属', unknown: '范围未确认' }
const authStages: Record<string, string> = { initial: '交换首包', authentication: '处理挑战', authenticated: '鉴权后交换' }
const digests: Record<string, string> = { akav1_md5: 'AKAv1-MD5', akav2_md5: 'AKAv2-MD5', akav2_sha256: 'AKAv2-SHA-256', plain_md5: '策略允许的 MD5' }
const tlsVersions: Record<string, string> = { TLSv1_2: 'TLS 1.2', TLSv1_3: 'TLS 1.3' }
const tlsCiphers: Record<string, string> = {
  TLS13_AES_128_GCM_SHA256: 'AES-128-GCM / SHA256', TLS13_AES_256_GCM_SHA384: 'AES-256-GCM / SHA384',
  TLS13_CHACHA20_POLY1305_SHA256: 'CHACHA20-POLY1305 / SHA256',
  TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256: 'ECDHE-RSA / AES-128-GCM',
  TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384: 'ECDHE-RSA / AES-256-GCM',
  TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256: 'ECDHE-RSA / CHACHA20-POLY1305',
  TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256: 'ECDHE-ECDSA / AES-128-GCM',
  TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384: 'ECDHE-ECDSA / AES-256-GCM',
  TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256: 'ECDHE-ECDSA / CHACHA20-POLY1305',
}

function lookup(table: Record<string, string>, value: unknown, fallback: string) {
  return typeof value === 'string' && Object.hasOwn(table, value) ? table[value] : fallback
}
export function diagnosticSource(value?: string | null) { return lookup(sources, value, '未确认') }
export function diagnosticIdentity(value?: string | null) { return lookup(identities, value, '尚未确认') }
export function diagnosticTransport(value?: string | null) { return lookup(transports, value, '未确认') }
function family(value?: string) { return lookup(families, value, '未确认') }
function integer(value: unknown, max = Number.MAX_SAFE_INTEGER): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value <= max }
function port(value: unknown) { return integer(value, 65535) && value !== 0 ? String(value) : '未确认' }
function time(value?: string | null) {
  if (!value || value.length > 64 || !/^\d{4}-\d{2}-\d{2}T/.test(value)) return '未记录'
  const date = new Date(value)
  return Number.isFinite(date.getTime()) ? date.toLocaleString() : '未记录'
}
function configId(value?: string | null) { return value && value.length <= 256 && !Array.from(value).some((char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127) ? value : '未记录' }
function reasonCode(value?: string | null) {
  return value && /^(?:ims_|cellular_ims_|vowifi_|digest_|sim_|live_|ike_|eap_|epdg_|carrier_|native_|qmi_)[a-z0-9_]{1,110}$/.test(value) ? value : '未记录代码'
}
function algorithms(value: ImsIpsecAlgorithms[] | null | undefined, empty: string) {
  if (!Array.isArray(value)) return '未记录'
  if (!value.length) return empty
  return value.slice(0, 16).map((item) => {
    const integrity = lookup({ hmac_sha1_96: 'HMAC-SHA1-96', hmac_md5_96: 'HMAC-MD5-96' }, item?.integrity, '未知完整性算法')
    const encryption = lookup({ aes_cbc: 'AES-CBC', null: 'null（仍有完整性保护）' }, item?.encryption, '未知加密算法')
    return `${integrity} + ${encryption}`
  }).join('；')
}

export interface ImsDiagnosticRow { label: string; value: string }

/** Editors do not own bearer policy. Omission explicitly preserves it server-side
 * only while profile/SIM/APN binding is unchanged; redaction is not replacement. */
export function withoutBearerUpdate<T extends { bearer?: ImsBearerPolicy | null }>(access: T): T {
  const copy = { ...access }
  delete copy.bearer
  return copy
}

export function carrierBearerUnchanged(access: { bearer?: ImsBearerPolicy }): true | undefined {
  const policy = access.bearer
  return policy && (policy.authentication.method !== 'unspecified'
    || policy.pcscf_via_pco !== 'network_default' || policy.pcscf_via_dhcp !== 'disabled') ? true : undefined
}

export function imsBearerCapabilityRows(capabilities?: ImsBearerCapabilities | null): ImsDiagnosticRow[] {
  const labels: Record<keyof ImsBearerCapabilities, string> = {
    auth_none: '无 APN 认证', pap: 'PAP', chap: 'CHAP', pap_or_chap: 'PAP 或 CHAP',
    pco_request_policy: 'PCO 请求策略', pco_pcscf_addresses: 'PCO P-CSCF 地址',
    pco_pcscf_domains: 'PCO P-CSCF 域名', dhcp_information: 'DHCP 信息请求',
  }
  return (Object.keys(labels) as Array<keyof ImsBearerCapabilities>).map((key) => ({
    label: labels[key], value: lookup({ supported: '适配器支持', unsupported: '适配器不支持', unknown: '支持情况未知' }, capabilities?.[key], '支持情况未知'),
  }))
}

export function imsBearerRequirementRows(policy?: EffectiveImsBearerPolicy | null): ImsDiagnosticRow[] {
  if (!policy) return []
  const present = (value: unknown) => value === true ? '已保存' : value === false ? '未保存' : '未知'
  return [
    { label: '配置的 APN 认证', value: lookup({ unspecified: '网络默认', none: '无认证', pap: 'PAP', chap: 'CHAP', pap_or_chap: 'PAP 或 CHAP' }, policy.authentication?.method, '未知') },
    { label: '用户名 / 密码', value: `${present(policy.authentication?.username_present)} / ${present(policy.authentication?.password_present)}（不回显）` },
    { label: '配置的 PCO 策略', value: lookup({ network_default: '网络默认', required: '要求请求', disabled: '禁用' }, policy.pcscf_via_pco, '未知') },
    { label: '配置的 DHCP 策略', value: lookup({ disabled: '禁用', information_request: '信息请求' }, policy.pcscf_via_dhcp, '未知') },
    { label: '承载要求来源', value: lookup({ catalog: '运营商配置', sim_override: 'SIM 覆盖' }, policy.source, '未确认') },
    { label: '配置校验', value: policy.validation_error ? reasonCode(policy.validation_error) : '未报告配置错误（非就绪证明）' },
  ]
}

const nativeImsErrors: Record<string, string> = {
  native_ims_unsupported: '此线路不支持原生 IMS 观测。',
  native_ims_busy: '线路正被占用或已有观测进行中；未执行新采样。',
  native_ims_binding_changed: 'SIM 或设备绑定已变化；请重新确认线路。',
  native_ims_unavailable: '原生 IMS 观测暂不可用。',
  native_ims_malformed_response: '原生 IMS 观测响应不符合约定；未展示样本。',
  native_ims_firmware_not_qualified: '此固件未通过只读观测资格确认；未执行采样。',
}
export function nativeImsErrorMessage(code: unknown): string | null {
  return code === null || code === undefined ? null
    : lookup(nativeImsErrors, code, '原生 IMS 观测失败（未识别错误；不显示原始响应）。')
}

/** Reject incompatible/cross-line envelopes before storing or rendering them. */
export function parseNativeImsObservation(value: unknown, lineId: string): NativeImsObservationStatus | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const data = value as Record<string, unknown>
  if (data.line_id !== lineId || data.purpose !== 'modem_observation_not_host_ims_readiness'
    || data.sample_kind !== 'sequential_not_atomic' || typeof data.adapter_available !== 'boolean'
    || typeof data.stale !== 'boolean' || typeof data.state !== 'string'
    || !['not_probed', 'probing', 'observed', 'stale', 'unavailable'].includes(data.state)
    || (data.observed_at !== null && typeof data.observed_at !== 'string')
    || (data.age_seconds !== null && !integer(data.age_seconds))
    || (data.last_error !== null && typeof data.last_error !== 'string')) return null
  if (data.sample !== null && data.sample !== undefined) {
    if (typeof data.sample !== 'object' || Array.isArray(data.sample)) return null
    const sample = data.sample as Record<string, unknown>
    if (sample.schema !== 'ec20_ce_fag_v0803' || typeof sample.registration_reporting !== 'boolean'
      || typeof sample.service_reporting !== 'boolean' || typeof sample.registration !== 'string'
      || !['unknown_or_unreported', 'registering', 'registered', 'limited', 'unknown'].includes(sample.registration)
      || !['sms', 'voice', 'ut'].every((key) => typeof sample[key] === 'string'
        && ['unknown_or_unreported', 'limited', 'available', 'unknown'].includes(sample[key]))) return null
  }
  return data as unknown as NativeImsObservationStatus
}

export function nativeImsObservationRows(data: NativeImsObservationStatus): ImsDiagnosticRow[] {
  const rows: ImsDiagnosticRow[] = [
    { label: '缓存记录状态', value: lookup({ not_probed: '尚未采样', probing: '采样进行中', observed: '已缓存历史样本', stale: '缓存已标记过期', unavailable: '观测不可用' }, data.state, '未知') },
    { label: '拓扑 / 已安装适配器', value: data.adapter_available ? '可用（不代表固件已通过资格确认）' : '不可用（不代表网络服务状态）' },
    { label: '缓存过期标记（读取时）', value: data.stale ? '已标记过期' : '未标记过期（不代表当前状态）' },
  ]
  const sample = data.sample
  if (!sample) return rows
  const service = (value: unknown) => lookup({ unknown_or_unreported: '未知或未报告（0，不能据此判断不可用）', limited: '受限（仅样本报告）', available: '可用（仅样本报告）', unknown: '未知' }, value, '未知')
  rows.push(
    { label: '最近样本时间（历史记录）', value: time(data.observed_at) },
    { label: '最近样本 · 基带 IMS 注册', value: lookup({ unknown_or_unreported: '未知或未报告（0，不能据此判断未注册）', registering: '注册中（仅基带报告）', registered: '已注册（仅基带报告）', limited: '受限（仅基带报告）', unknown: '未知' }, sample.registration, '未知') },
    { label: '最近样本 · SMS', value: service(sample.sms) },
    { label: '最近样本 · 语音', value: service(sample.voice) },
    { label: '最近样本 · UT（补充业务，非视频）', value: service(sample.ut) },
    { label: '最近样本 · 注册 / 服务报告标志', value: `${sample.registration_reporting ? 'true' : 'false'} / ${sample.service_reporting ? 'true' : 'false'}（仅返回标志，不证明持久 NV 读取成功）` },
  )
  return rows
}

export function imsDiagnosticRows(facts?: ImsRegistrationDiagnostics | null): ImsDiagnosticRow[] {
  if (!facts || facts.version !== 1 || !facts.observed) return []
  const rows: ImsDiagnosticRow[] = [
    { label: '实际 profile 来源', value: `${diagnosticSource(facts.profile_source)} · ${configId(facts.profile_id)}` },
    { label: '实际身份来源', value: diagnosticIdentity(facts.identity_source) },
    { label: '域名 / realm 来源', value: `${diagnosticSource(facts.domain_source)} / ${diagnosticSource(facts.realm_source)}` },
    { label: 'IMS 请求 / 授予 / 信令族', value: `${family(facts.requested_family)} / ${family(facts.granted_family)} / ${family(facts.signaling_family)}` },
    { label: 'P-CSCF 来源', value: `${diagnosticSource(facts.pcscf_source)}${integer(facts.endpoint_index, 64) && integer(facts.endpoint_count, 64) ? ` · 候选 ${facts.endpoint_index}/${facts.endpoint_count}` : ''}` },
    { label: '信令传输', value: `${diagnosticTransport(facts.transport)}${facts.socket_ready ? ' · 已创建过自有 socket（非存活证明）' : ' · 仅候选，尚未创建通道'}` },
    { label: '发现端口 / 信令端口', value: `${port(facts.pcscf_port)} / ${port(facts.signaling_port)}` },
    { label: 'SIP/IPsec 报价', value: algorithms(facts.offered_ipsec, '本次未报价 IPsec') },
    { label: '最近成功的安全协商', value: !facts.negotiated_from_success ? '尚未记录成功协商' : facts.security_mode === 'ipsec'
      ? `IPsec · ${algorithms(facts.negotiated_ipsec, '算法未确认')}` : facts.security_mode === 'tls' ? 'TLS（不替代明确的 IPsec 要求）'
        : facts.security_mode === 'plain' ? 'SIP 层未协商独立 IPsec；接入隧道状态另列' : '未确认' },
    { label: '请求 Digest 方案', value: lookup(digests, facts.digest_algorithm, '未记录') },
    { label: '最近成功响应 rspauth', value: !facts.negotiated_from_success || (facts.rspauth_verified === null || facts.rspauth_verified === undefined) ? '未记录'
      : facts.rspauth_verified ? '已验证服务器响应证明' : '未提供可选 rspauth，不冒称已验' },
  ]
  const tlsLabel = lookup({ not_observed: '未记录', not_applicable: '不适用', pending: '尚未完成验证', verified: '建立连接时已验证', failed: '验证/握手失败' }, facts.tls_verification, '未确认')
  rows.push({ label: 'TLS 身份与算法', value: `${tlsLabel}${facts.tls_algorithms ? ` · ${lookup(tlsVersions, facts.tls_algorithms.version, '版本未确认')} · ${lookup(tlsCiphers, facts.tls_algorithms.cipher_suite, '算法未确认')}` : ''}` })
  if (facts.failure) {
    const failure = facts.failure
    rows.push({ label: '最近失败范围', value: `${lookup(scopes, failure.scope, '范围未确认')} · ${lookup(procedures, failure.procedure, '过程未确认')}${failure.auth_stage ? ` / ${lookup(authStages, failure.auth_stage, '阶段未确认')}` : ''}` })
    rows.push({ label: '失败依据', value: `${reasonCode(failure.reason_code)}${integer(failure.sip_status, 699) && failure.sip_status >= 100 ? ` · SIP ${failure.sip_status}` : ''}${failure.lease_negotiation_observed ? ' · 已发生 423 协商' : ''}` })
    rows.push({ label: '响应中的等待要求', value: failure.response_retry_after_invalid ? 'Retry-After 无效，不能当作零等待'
      : integer(failure.response_retry_after_seconds, 4294967295) ? `${failure.response_retry_after_seconds} 秒（实际门禁另列）` : '未提供/未观察到' })
  }
  const wait = facts.wait
  rows.push({ label: '实际重试门禁', value: wait?.status === 'waiting' ? `等待${integer(wait.retry_after_seconds) ? ` ${wait.retry_after_seconds} 秒` : ''} · 不早于 ${time(wait.not_before)}`
    : wait?.status === 'stopped' ? '已停止，等待重新核验条件；不能清预算绕过'
      : wait?.status === 'ready' ? '门禁允许重试（不代表已经注册）' : '尚未观测门禁' })
  if (facts.identity_rule_status === 'declared_only') rows.push({ label: '身份模板能力', value: '配置只声明了模板标签；本次未执行未实现的模板规则，以实际身份来源为准' })
  rows.push({ label: '最近执行记录', value: `${lookup(procedures, facts.procedure, '未记录')} · ${time(facts.updated_at)}` })
  return rows
}
