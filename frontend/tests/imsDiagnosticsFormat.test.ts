import assert from 'node:assert/strict'
import test from 'node:test'
import type { ImsRegistrationDiagnostics } from '../src/api/contracts.ts'
import { diagnosticIdentity, diagnosticSource, diagnosticTransport, imsDiagnosticRows } from '../src/pages/sim/imsDiagnosticsFormat.ts'

function facts(overrides: Partial<ImsRegistrationDiagnostics> = {}): ImsRegistrationDiagnostics {
  return {
    version: 1, observed: true, procedure: 'initial', profile_id: 'actual-profile', profile_source: 'database',
    identity_source: 'isim', declared_identity_source: 'carrier_device_model', identity_rule_status: 'declared_only',
    domain_source: 'isim', realm_source: 'sim_override', requested_family: 'ipv4v6', granted_family: 'ipv6', signaling_family: 'ipv6',
    pcscf_source: 'bearer_pco', transport: 'tcp', pcscf_port: 5060, signaling_port: 5098, socket_ready: true,
    endpoint_index: 1, endpoint_count: 2, tls_verification: 'not_applicable', tls_algorithms: null,
    offered_ipsec: [{ integrity: 'hmac_sha1_96', encryption: 'aes_cbc' }, { integrity: 'hmac_sha1_96', encryption: 'null' }],
    negotiated_ipsec: [{ integrity: 'hmac_sha1_96', encryption: 'null' }], security_mode: 'ipsec',
    digest_algorithm: 'akav1_md5', rspauth_verified: false, negotiated_from_success: true, failure: null,
    wait: { status: 'ready' }, updated_at: '2026-10-13T02:00:00Z', ...overrides,
  }
}
function row(data: ImsRegistrationDiagnostics, label: string) {
  return imsDiagnosticRows(data).find((row) => row.label === label)?.value ?? ''
}

test('missing/unknown versions do not turn configuration into execution evidence', () => {
  assert.deepEqual(imsDiagnosticRows(undefined), [])
  assert.deepEqual(imsDiagnosticRows(facts({ version: 2 })), [])
  assert.deepEqual(imsDiagnosticRows(facts({ observed: false })), [])
})

test('actual identity, requested and granted families and profile source are independent', () => {
  const data = facts()
  assert.match(row(data, '实际 profile 来源'), /自定义数据库.*actual-profile/)
  assert.equal(row(data, '实际身份来源'), 'ISIM 卡内身份')
  assert.equal(row(data, 'IMS 请求 / 授予 / 信令族'), 'IPv4v6 / IPv6 / IPv6')
  assert.match(row(data, '身份模板能力'), /只声明了模板标签/)
  assert.match(row(data, 'P-CSCF 来源'), /承载 PCO.*1\/2/)
  assert.equal(row(data, '发现端口 / 信令端口'), '5060 / 5098')
})

test('IPsec offers are not reported as negotiated and optional rspauth is not fabricated', () => {
  const data = facts()
  assert.match(row(data, 'SIP/IPsec 报价'), /AES-CBC/)
  assert.match(row(data, 'SIP/IPsec 报价'), /null/)
  assert.match(row(data, '最近成功的安全协商'), /null/)
  assert.doesNotMatch(row(data, '最近成功的安全协商'), /AES-CBC/)
  assert.match(row(data, '最近成功响应 rspauth'), /未提供可选/)
  assert.equal(row(facts({ negotiated_from_success: false }), '最近成功的安全协商'), '尚未记录成功协商')
})

test('TLS information distinguishes a candidate from a verified connection', () => {
  const pending = facts({ transport: 'tls', socket_ready: false, tls_verification: 'pending', negotiated_from_success: false })
  assert.match(row(pending, '信令传输'), /TLS.*仅候选/)
  assert.match(row(pending, 'TLS 身份与算法'), /尚未完成验证/)
  const verified = facts({ transport: 'tls', tls_verification: 'verified', security_mode: 'tls',
    tls_algorithms: { version: 'TLSv1_3', cipher_suite: 'TLS13_AES_128_GCM_SHA256' } })
  assert.match(row(verified, 'TLS 身份与算法'), /已验证.*TLS 1\.3.*AES-128-GCM/)
})

test('response requested wait and the actual gate are not conflated', () => {
  const data = facts({ procedure: 'refresh', failure: { scope: 'endpoint', reason_code: 'ims_register_authenticated_unexpected_status',
    sip_status: 503, auth_stage: 'authenticated', procedure: 'refresh', endpoint_index: 1,
    lease_negotiation_observed: false, response_retry_after_seconds: 300, response_retry_after_invalid: false },
    wait: { status: 'waiting', retry_after_seconds: 29, not_before: '2026-10-13T02:05:00Z' } })
  assert.match(row(data, '响应中的等待要求'), /^300 秒/)
  assert.match(row(data, '实际重试门禁'), /等待 29 秒/)
  assert.match(row(data, '失败依据'), /SIP 503/)
  const ready = facts({ ...data, wait: { status: 'ready' } })
  assert.match(row(ready, '实际重试门禁'), /门禁允许重试.*不代表已经注册/)
  assert.match(row(facts({ wait: { status: 'stopped' } }), '实际重试门禁'), /不能清预算绕过/)
})

test('only documented fields and known labels are rendered, never raw secrets or prototype values', () => {
  const forged = { ...facts(), profile_source: '__proto__', identity_source: 'constructor',
    authorization: 'SECRET_AUTH', raw_sip: 'SECRET_SIP', nonce: 'SECRET_NONCE',
    tls_verification: 'verified', tls_algorithms: { version: 'SECRET_VERSION', cipher_suite: 'SECRET_CIPHER' },
    failure: { scope: 'constructor', procedure: '__proto__', reason_code: 'ims_error:SECRET_IMPI',
      response_retry_after_invalid: false, lease_negotiation_observed: false } } as unknown as ImsRegistrationDiagnostics
  const text = JSON.stringify(imsDiagnosticRows(forged))
  assert.doesNotMatch(text, /SECRET|constructor|__proto__|function Object/)
  assert.equal(diagnosticSource('constructor'), '未确认')
  assert.equal(diagnosticIdentity('__proto__'), '尚未确认')
  assert.equal(diagnosticTransport('toString'), '未确认')
})
