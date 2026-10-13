import type { ImsFactSource, ImsIpsecAlgorithms, ImsRegistrationDiagnostics } from '../../api/contracts.ts'

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
