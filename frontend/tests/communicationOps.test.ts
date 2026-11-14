import assert from 'node:assert/strict'
import test from 'node:test'
import { communicationWrites, validCommunicationOperation, communicationActionLabel, communicationObservationLabel, communicationStatusLines, newCommunicationOperation, isCommunicationAction } from '../src/policies/communicationOps.ts'
import { canSubmitJegotripLogin, jegotripLoginInput, emptyAppLoginDraft, appLoginError, appLoginPhase, type AppLoginDraft } from '../src/policies/appLogin.ts'
const id = '0123456789abcdef0123456789abcdef'
const draft: AppLoginDraft = { ...emptyAppLoginDraft(), mode: 'account', username: ' user@example.invalid ', password: ' p@ss ', consent: true, startReceiver: false,
  deviceMode: 'phone_app', deviceId: 'D5C0199B-1644-4D33-8912-B8BD5AB49224', intent: 'communication', communicationOperation: { kind: 'query_profile' }, configurationRevision: 1 }
void test('communication reads remain separate from original default hosting and require the actual original UUID and revision', () => {
  assert.equal(emptyAppLoginDraft().intent, 'enable_hosting'); assert.equal(canSubmitJegotripLogin(draft), true)
  for (const patch of [{ deviceMode: 'auto' as const }, { deviceId: '' }, { startReceiver: true }, { configurationRevision: undefined }, { configurationRevision: -1 }, { configurationRevision: 2 ** 53 }, { communicationOperation: undefined }, { consent: false }]) assert.equal(canSubmitJegotripLogin({ ...draft, ...patch }), false)
})
void test('communication serializer sends only the selected operation, current revision and explicit ownership consent', () => {
  const value = jegotripLoginInput(id, draft)
  assert.deepEqual(value, { attempt_id: id, username: 'user@example.invalid', password: ' p@ss ', country_code: null, intent: 'communication', consent: true, start_receiver: false,
    device_id: draft.deviceId, communication_operation: { kind: 'query_profile' }, write_consent: false, expected_configuration_revision: 1 })
  for (const key of ['url', 'phone_login_token', 'sip_password', 'enrollId', 'trust_granted', 'enable_hosting', 'device_sync_profile']) assert.equal(key in value, false)
})
void test('every business operation requires separate explicit consent and no default parameter is guessed', () => {
  for (const kind of ['set_phone_service', 'set_voip_profile', 'set_jego_m_status', 'close_forward', 'logout'] as const) {
    assert.equal(communicationWrites(kind), true)
    const op = newCommunicationOperation(kind)
    assert.equal(canSubmitJegotripLogin({ ...draft, communicationOperation: op, writeConsent: false }), false)
  }
  assert.equal(validCommunicationOperation(newCommunicationOperation('set_phone_service')), false)
  assert.equal(validCommunicationOperation(newCommunicationOperation('set_voip_profile')), false)
  assert.equal(validCommunicationOperation(newCommunicationOperation('set_jego_m_status')), false)
  const input = { ...draft, communicationOperation: { kind: 'set_phone_service' as const, status: 3 }, writeConsent: true }
  assert.equal(canSubmitJegotripLogin(input), true); assert.equal(jegotripLoginInput(id, input).write_consent, true)
})
void test('communication numeric and string body parameters preserve protocol types and reject truncation or unknown enums', () => {
  assert.equal(validCommunicationOperation({ kind: 'set_phone_service', status: 3 }), true)
  for (const status of [-1, 1000000, 1.5, NaN, Infinity]) assert.equal(validCommunicationOperation({ kind: 'set_phone_service', status }), false)
  for (const cs_forward of ['1', '2']) assert.equal(validCommunicationOperation({ kind: 'set_voip_profile', cs_forward }), true)
  for (const cs_forward of ['01', '', '0', 'true', 'private-token']) assert.equal(validCommunicationOperation({ kind: 'set_voip_profile', cs_forward }), false)
  assert.equal(validCommunicationOperation({ kind: 'set_jego_m_status', jegom_status: '2', region_code: '44', country_code: '44' }), true)
  assert.equal(validCommunicationOperation({ kind: 'set_jego_m_status', jegom_status: '1', region_code: '', country_code: 'GB' }), true)
  assert.equal(validCommunicationOperation({ kind: 'set_jego_m_status', jegom_status: '7', region_code: '44', country_code: '44' }), false)
})
void test('read-only communication kinds never acquire write permission from a status value or prototype property', () => {
  for (const kind of ['query_profile', 'query_phone_service', 'query_display_status', 'query_forwarding_state'] as const) { assert.equal(communicationWrites(kind), false); assert.equal(isCommunicationAction(kind), true) }
  for (const unknown of ['constructor', '__proto__', 'private-token', {}, null]) { assert.equal(isCommunicationAction(unknown), false); assert.equal(communicationWrites(unknown), true); assert.equal(communicationActionLabel(unknown), '未知通信操作') }
})
void test('communication observation is finite and explicitly does not prove hosting, SDK or media readiness', () => {
  for (const state of ['request_uncertain', 'accepted_unconfirmed', 'acknowledged', 'confirmed', 'observed']) {
    const text = communicationObservationLabel({ action: 'close_forward', state })
    assert.match(text, /不代表设备授信、SDK或收件成功/)
  }
  assert.match(communicationObservationLabel({ action: 'logout', state: 'request_uncertain' }), /不自动重发/)
  for (const state of ['constructor', 'toString', 'private-secret', null]) { const text = communicationObservationLabel({ action: 'private-secret', state }); assert.equal(text.includes('private-secret'), false); assert.match(text, /状态未知/) }
})
void test('communication status display rejects raw private data and uses only typed profile values and masked numbers', () => {
  const lines = communicationStatusLines({ status: { phone_server_status: 3, cs_forward: 2, jegom_status: 1, region_code: '44', masked_jegom_phone: '***0123', token: 'private-secret', profile: {
    allow_mo_voice: 1, allow_mt_voice: 0, mo_voice_limit: -1, mt_voice_limit: 900,
    forbidden_countries: [{ country_code: 'GB', region_id: '9223372036854775807', country_cname: 'private-secret' }], answer_forbidden_countries: [] } } })
  assert.match(lines.join('\n'), /电话服务原状态：3/); assert.match(lines.join('\n'), /mo_voice_limit：-1/); assert.match(lines.join('\n'), /来显号码：\*\*\*0123/); assert.equal(lines.join('\n').includes('private-secret'), false)
  assert.deepEqual(communicationStatusLines({ status: { phone_server_status: 'private-secret', region_code: 'private-secret', masked_jegom_phone: '7700900123' } }), [])
})
void test('communication errors and phases distinguish unconfigured, unresolved and unconfirmed without automatic replay', () => {
  assert.match(appLoginError('app_login_communication_unresolved'), /只允许查询/); assert.match(appLoginError('app_login_communication_unconfirmed'), /不会自动重发/)
  assert.match(appLoginError('app_login_communication_writes_disabled'), /未批准/); assert.match(appLoginError('app_login_communication_not_configured'), /不会猜测/)
  assert.match(appLoginPhase('communication_request'), /通信操作/); assert.match(appLoginPhase('communication_confirm'), /读回/)
})
