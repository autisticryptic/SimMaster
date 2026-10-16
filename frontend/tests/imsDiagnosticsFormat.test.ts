import assert from 'node:assert/strict'
import test from 'node:test'
import type { EffectiveImsBearerPolicy, ImsBearerCapabilities, ImsRegistrationDiagnostics, NativeImsObservationStatus } from '../src/api/contracts.ts'
import { carrierBearerUnchanged, diagnosticIdentity, diagnosticSource, diagnosticTransport, imsBearerCapabilityRows, imsBearerRequirementRows, imsDiagnosticRows, nativeImsErrorMessage, nativeImsObservationRows, parseNativeImsObservation, withoutBearerUpdate } from '../src/pages/sim/imsDiagnosticsFormat.ts'

void test('bearer support is passive capability, unknown is not unsupported or ready', () => {
  assert.ok(imsBearerCapabilityRows().every(({ value }) => value === '支持情况未知'))
  const capabilities = { pap: 'supported', chap: 'unsupported', dhcp_information: 'unknown', auth_none: '__proto__' } as unknown as ImsBearerCapabilities
  const rows = imsBearerCapabilityRows(capabilities)
  assert.equal(rows.find(({ label }) => label === 'PAP')?.value, '适配器支持')
  assert.equal(rows.find(({ label }) => label === 'CHAP')?.value, '适配器不支持')
  assert.equal(rows.find(({ label }) => label === 'DHCP 信息请求')?.value, '支持情况未知')
  assert.doesNotMatch(JSON.stringify(rows), /已注册|已就绪|__proto__/)
})

void test('bearer requirement projection never displays credential values and editor saves omit rather than replay redaction', () => {
  const policy = { authentication: { method: 'pap', username_present: true, password_present: true,
    username: 'SYNTHETIC_USER_DO_NOT_RENDER', password: 'SYNTHETIC_PASSWORD_DO_NOT_RENDER' },
    pcscf_via_pco: 'required', pcscf_via_dhcp: 'disabled', source: 'sim_override' } as EffectiveImsBearerPolicy
  const text = JSON.stringify(imsBearerRequirementRows(policy))
  assert.match(text, /PAP/)
  assert.match(text, /已保存 \/ 已保存/)
  assert.doesNotMatch(text, /SYNTHETIC/)
  const original = { bearer: policy, apn: 'ims', domain: 'ims.example.test' }
  const write = withoutBearerUpdate(original)
  assert.equal(Object.hasOwn(write, 'bearer'), false)
  assert.equal(write.apn, 'ims')
  assert.equal(write.domain, original.domain)
  assert.equal(original.bearer, policy)
  assert.equal(carrierBearerUnchanged(original), true)
  assert.equal(carrierBearerUnchanged({}), undefined)
  assert.doesNotMatch(JSON.stringify(write), /SYNTHETIC|authentication|bearer/)
})

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

void test('missing/unknown versions do not turn configuration into execution evidence', () => {
  assert.deepEqual(imsDiagnosticRows(undefined), [])
  assert.deepEqual(imsDiagnosticRows(facts({ version: 2 })), [])
  assert.deepEqual(imsDiagnosticRows(facts({ observed: false })), [])
})

void test('actual identity, requested and granted families and profile source are independent', () => {
  const data = facts()
  assert.match(row(data, '实际 profile 来源'), /自定义数据库.*actual-profile/)
  assert.equal(row(data, '实际身份来源'), 'ISIM 卡内身份')
  assert.equal(row(data, 'IMS 请求 / 授予 / 信令族'), 'IPv4v6 / IPv6 / IPv6')
  assert.match(row(data, '身份模板能力'), /只声明了模板标签/)
  assert.match(row(data, 'P-CSCF 来源'), /承载 PCO.*1\/2/)
  assert.equal(row(data, '发现端口 / 信令端口'), '5060 / 5098')
})

void test('IPsec offers are not reported as negotiated and optional rspauth is not fabricated', () => {
  const data = facts()
  assert.match(row(data, 'SIP/IPsec 报价'), /AES-CBC/)
  assert.match(row(data, 'SIP/IPsec 报价'), /null/)
  assert.match(row(data, '最近成功的安全协商'), /null/)
  assert.doesNotMatch(row(data, '最近成功的安全协商'), /AES-CBC/)
  assert.match(row(data, '最近成功响应 rspauth'), /未提供可选/)
  assert.equal(row(facts({ negotiated_from_success: false }), '最近成功的安全协商'), '尚未记录成功协商')
})

void test('TLS information distinguishes a candidate from a verified connection', () => {
  const pending = facts({ transport: 'tls', socket_ready: false, tls_verification: 'pending', negotiated_from_success: false })
  assert.match(row(pending, '信令传输'), /TLS.*仅候选/)
  assert.match(row(pending, 'TLS 身份与算法'), /尚未完成验证/)
  const verified = facts({ transport: 'tls', tls_verification: 'verified', security_mode: 'tls',
    tls_algorithms: { version: 'TLSv1_3', cipher_suite: 'TLS13_AES_128_GCM_SHA256' } })
  assert.match(row(verified, 'TLS 身份与算法'), /已验证.*TLS 1\.3.*AES-128-GCM/)
})

void test('response requested wait and the actual gate are not conflated', () => {
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

void test('only documented fields and known labels are rendered, never raw secrets or prototype values', () => {
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

function nativeObservation(): NativeImsObservationStatus {
  return { line_id: 'line-a', adapter_available: true, state: 'observed',
    sample: { schema: 'ec20_ce_fag_v0803', registration_reporting: false, service_reporting: true,
      registration: 'unknown_or_unreported', sms: 'unknown_or_unreported', voice: 'limited', ut: 'available' },
    observed_at: '2026-10-13T02:00:00Z', age_seconds: 5, stale: false, last_error: null,
    sample_kind: 'sequential_not_atomic', purpose: 'modem_observation_not_host_ims_readiness' }
}

void test('native IMS zero remains unknown, UT is not video and flags are only historical reports', () => {
  const data = nativeObservation()
  const rows = nativeImsObservationRows(data)
  const text = JSON.stringify(rows)
  assert.match(text, /未知或未报告（0，不能据此判断未注册）/)
  assert.match(text, /未知或未报告（0，不能据此判断不可用）/)
  assert.match(text, /UT（补充业务，非视频）/)
  assert.match(text, /false \/ true（仅返回标志，不证明持久 NV 读取成功）/)
  assert.match(text, /不代表固件已通过资格确认/)
  assert.match(text, /最近样本时间（历史记录）/)
  assert.match(text, /未标记过期（不代表当前状态）/)
  assert.doesNotMatch(text, /倒计时|5 秒/)
  assert.equal(parseNativeImsObservation(data, 'line-a'), data)
})

void test('native IMS responses reject cross-line, incompatible schema and malformed sample fields', () => {
  const data = nativeObservation()
  assert.equal(parseNativeImsObservation(data, 'line-b'), null)
  for (const sample of [{ ...data.sample, schema: 'future_schema' }, { ...data.sample, registration: 0 },
    { ...data.sample, ut: 'video' }, { ...data.sample, service_reporting: 'false' }, []]) {
    assert.equal(parseNativeImsObservation({ ...data, sample }, 'line-a'), null)
  }
  for (const change of [{ purpose: 'host_ready' }, { sample_kind: 'atomic' }, { state: 'constructor' },
    { age_seconds: -1 }, { stale: 'false' }, { adapter_available: 'true' }]) {
    assert.equal(parseNativeImsObservation({ ...data, ...change }, 'line-a'), null)
  }
  assert.equal(parseNativeImsObservation(null, 'line-a'), null)
  assert.ok(parseNativeImsObservation({ ...data, sample: null, state: 'not_probed' }, 'line-a'))
})

void test('native IMS renders only safe error translations and never raw AT or identities', () => {
  const messages = ['unsupported', 'busy', 'binding_changed', 'unavailable', 'malformed_response', 'firmware_not_qualified']
    .map((code) => nativeImsErrorMessage(`native_ims_${code}`))
  assert.equal(new Set(messages).size, 6)
  assert.ok(messages.every((message) => message && !message.includes('未识别')))
  for (const code of ['AT+SECRET_IDENTITY', '__proto__', 'constructor', 'native_ims_busy:SECRET']) {
    assert.match(nativeImsErrorMessage(code) ?? '', /未识别错误/)
    assert.doesNotMatch(nativeImsErrorMessage(code) ?? '', /SECRET|AT\+|__proto__|constructor/)
  }
  const data = { ...nativeObservation(), observed_at: 'SECRET_TIME', last_error: 'SECRET_AT', raw: 'SECRET_IDENTITY' }
  assert.doesNotMatch(JSON.stringify(nativeImsObservationRows(data)), /SECRET/)
  assert.equal(nativeImsErrorMessage(null), null)
})
