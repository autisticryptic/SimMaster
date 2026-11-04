import assert from 'node:assert/strict'
import test from 'node:test'
import { appLoginInput, canSubmitAppLogin, jegotripLoginInput, canSubmitJegotripLogin, emptyAppLoginDraft, validJegotripDeviceId, validPhoneAppUuid, deviceTrustQueryLabel, deviceSyncLabel, appLoginError, appBusinessLabel, appLoginPhase, appLoginNeedsPolling, type AppLoginDraft } from '../src/policies/appLogin.ts'
import type { AppLoginAttempt } from '../src/api/current.ts'

const id = '0123456789abcdef0123456789abcdef'
const draft: AppLoginDraft = { mode: 'account', username: ' user@example.invalid ', password: ' p@ss ', country: '44', consent: true, startReceiver: true }

void test('account login keeps password exact and never submits personal SDK credentials', () => {
  assert.deepEqual(appLoginInput(id, draft), { attempt_id: id, username: 'user@example.invalid', password: ' p@ss ', country_code: null, enable_hosting: true, start_receiver: true })
  for (const key of ['phone_login_token', 'sip_password', 'enrollment_confirmed', 'iccid', 'imsi']) assert.equal(key in appLoginInput(id, draft), false)
})
void test('phone login requires an explicit digit country and normalizes only the leading plus', () => {
  assert.equal(appLoginInput(id, { ...draft, mode: 'phone', username: '7700900123', country: '+44' }).country_code, '44')
  for (const country of ['', '++44', '4+4', '44/example', '12345']) assert.equal(canSubmitAppLogin({ ...draft, mode: 'phone', username: '7700900123', country }), false)
})
void test('login consent, nonempty password, nonce and input byte limits are enforced', () => {
  assert.equal(canSubmitAppLogin({ ...draft, consent: false }), false)
  assert.equal(canSubmitAppLogin({ ...draft, password: '' }), false)
  assert.equal(canSubmitAppLogin({ ...draft, username: 'é'.repeat(128) }), false)
  assert.equal(canSubmitAppLogin({ ...draft, password: 'é'.repeat(513) }), false)
  assert.equal(canSubmitAppLogin({ ...draft, password: 'abc\nxyz' }), false)
  assert.throws(() => appLoginInput('not-an-id', draft))
})
void test('unknown server errors cannot echo URLs, passwords or tokens', () => {
  const output = appLoginError(new Error('private-password https://service.invalid/?token=secret'))
  assert.equal(output.includes('private-password'), false)
  assert.equal(output.includes('secret'), false)
  assert.equal(output.includes('service.invalid'), false)
})
void test('verification and existing-business actions are not presented as login success', () => {
  assert.match(appLoginError('app_login_verification_required'), /额外安全验证/)
  assert.match(appLoginError('app_login_existing_business_action_required'), /未自动关闭/)
  assert.match(appLoginError('app_login_migration_required'), /迁移/)
})
void test('remote acceptance and runtime readiness are deliberately different labels', () => {
  assert.equal(appBusinessLabel('accepted_unconfirmed'), '服务已接受，状态待确认')
  assert.equal(appBusinessLabel('request_uncertain'), '开通结果不确定')
  assert.equal(appLoginPhase('completed').includes('就绪'), false)
  assert.equal(appLoginPhase('starting_receiver'), '提交接收器启动')
})
void test('a finalization error stops UI polling without fabricating durable completion', () => {
  const value: AppLoginAttempt = { attempt_id: id, phase: 'creating_account', business_state: 'enabled', completed: false,
    line_id: null, masked_number: '***0123', error: 'app_login_storage_failed', server_code: null, created_at: 1, updated_at: 2 }
  assert.equal(appLoginNeedsPolling(value), false)
  assert.equal(value.completed, false)
  assert.equal(appLoginNeedsPolling({ ...value, error: null }), true)
  assert.equal(appLoginNeedsPolling({ ...value, error: null, completed: true }), false)
})

void test('primary Jegotrip form defaults to login, enable hosting and start receiver with explicit consent', () => {
  const primary = emptyAppLoginDraft()
  assert.equal(primary.intent, 'enable_hosting')
  assert.equal(primary.startReceiver, true)
  assert.equal(primary.consent, false)
  assert.equal(canSubmitJegotripLogin(primary), false)
  const submitted = jegotripLoginInput(id, { ...primary, username: '7700900123', password: ' p@ss ', consent: true })
  assert.equal(submitted.intent, 'enable_hosting')
  assert.equal(submitted.start_receiver, true)
  assert.equal(submitted.password, ' p@ss ')
})
void test('omitted intent stays full hosting while authentication-only requires explicit selection', () => {
  assert.equal(jegotripLoginInput(id, draft).intent, 'enable_hosting')
  assert.equal(jegotripLoginInput(id, draft).start_receiver, true)
  assert.equal(canSubmitJegotripLogin({ ...draft, intent: 'unknown' as AppLoginDraft['intent'] }), false)
})

void test('dedicated Jegotrip authentication does not silently request business enablement', () => {
  const auth: AppLoginDraft = { ...draft, intent: 'authenticate', startReceiver: false }
  assert.deepEqual(jegotripLoginInput(id, auth), { attempt_id: id, username: 'user@example.invalid', password: ' p@ss ', country_code: null,
    intent: 'authenticate', consent: true, start_receiver: false })
  assert.equal('enable_hosting' in jegotripLoginInput(id, auth), false)
  assert.equal(canSubmitJegotripLogin({ ...auth, startReceiver: true }), false)
  assert.equal(canSubmitJegotripLogin({ ...auth, consent: false }), false)
})
void test('dedicated hosting intent remains explicit and does not accept personal SDK inputs', () => {
  const hosting: AppLoginDraft = { ...draft, intent: 'enable_hosting' }
  assert.equal(jegotripLoginInput(id, hosting).intent, 'enable_hosting')
  for (const key of ['phone_login_token', 'sip_password', 'deployment', 'callback']) assert.equal(key in jegotripLoginInput(id, hosting), false)
  assert.throws(() => jegotripLoginInput('bad-id', hosting))
})
void test('database configuration and App-only summary labels do not claim phone readiness', () => {
  assert.match(appLoginError('app_login_configuration_revision_conflict'), /版本已变化/)
  assert.match(appLoginError('app_login_migration_conflict'), /保留原记录/)
  assert.match(appLoginError('app_login_native_configuration_missing'), /未开启托管/)
  assert.match(appLoginPhase('saving_app_account'), /最近 App 认证/)
  assert.equal(appLoginPhase('saving_app_account').includes('就绪'), false)
})

void test('device selection defaults to persisted automatic reuse and does not submit an accidental override', () => {
  assert.equal(emptyAppLoginDraft().deviceMode, 'auto')
  assert.equal('device_id' in jegotripLoginInput(id, draft), false)
  assert.equal('device_id' in jegotripLoginInput(id, { ...draft, deviceMode: 'auto', deviceId: 'STALE-DEVICE-01' }), false)
})
void test('manual App device identity is bounded and sent only through the dedicated Jegotrip form', () => {
  const manual: AppLoginDraft = { ...draft, deviceMode: 'manual', deviceId: ' OWN-DEVICE-0001 ' }
  assert.equal(canSubmitJegotripLogin(manual), true)
  assert.equal(jegotripLoginInput(id, manual).device_id, 'OWN-DEVICE-0001')
  assert.equal('device_id' in appLoginInput(id, manual), false)
  for (const value of ['', 'short', 'has space', 'not:a:device', 'https://other.invalid/', 'x'.repeat(65), 'OWN-\n-DEVICE']) {
    assert.equal(validJegotripDeviceId(value), false)
    assert.equal(canSubmitJegotripLogin({ ...manual, deviceId: value }), false)
    assert.throws(() => jegotripLoginInput(id, { ...manual, deviceId: value }))
  }
  assert.equal(canSubmitJegotripLogin({ ...manual, consent: false }), false)
  assert.equal(canSubmitJegotripLogin({ ...manual, deviceMode: 'unknown' as AppLoginDraft['deviceMode'] }), false)
})
void test('device conflicts, stale revisions and active leases have Chinese messages without secret reflection', () => {
  assert.match(appLoginError('app_login_device_identity_conflict'), /另一身份/)
  assert.match(appLoginError('app_login_device_identity_revision_conflict'), /刷新/)
  assert.match(appLoginError('app_login_device_identity_in_use'), /停止接收器/)
  const message = appLoginError(new Error('app_login_device_identity_conflict OWN-PRIVATE-DEVICE https://other.invalid/?token=secret'))
  assert.equal(message.includes('OWN-PRIVATE-DEVICE'), false)
  assert.equal(message.includes('secret'), false)
  assert.match(appLoginError('app_login_device_untrusted'), /不等于通过一键验证/)
})
void test('manual device selection cannot change the explicit attach-hosting or authentication intent', () => {
  const attach = jegotripLoginInput(id, { ...draft, intent: 'attach_hosting', deviceMode: 'manual', deviceId: 'OWN-DEVICE-01' })
  assert.equal(attach.intent, 'attach_hosting')
  assert.equal(attach.device_id, 'OWN-DEVICE-01')
  assert.equal('enable_hosting' in attach, false)
  assert.equal(jegotripLoginInput(id, { ...draft, intent: 'authenticate', startReceiver: false, deviceMode: 'manual', deviceId: 'OWN-DEVICE-01' }).intent, 'authenticate')
})
void test('phone trust transfer accepts only the App UUID shape and never turns it into an authorization field', () => {
  const value: AppLoginDraft = { ...draft, intent: 'reuse_hosting', deviceMode: 'phone_app', deviceId: ' D5C0199B-1644-4D33-8912-B8BD5AB49224 ' }
  const input = jegotripLoginInput(id, value)
  assert.equal(input.device_id, 'D5C0199B-1644-4D33-8912-B8BD5AB49224')
  assert.equal(input.intent, 'reuse_hosting')
  for (const field of ['device_trust', 'trust_granted', 'sso_token', 'enroll_id', 'hardware_imei', 'sdk_device_id']) assert.equal(field in input, false)
  for (const invalid of ['0123456789abcdef0123456789abcdef', '123456789012345', 'fixture-enroll-id', 'UNKNOWN_DEVICE', '', 'OWN-MANUAL-01']) {
    assert.equal(validPhoneAppUuid(invalid), false)
    assert.equal(canSubmitJegotripLogin({ ...value, deviceId: invalid }), false)
  }
})
void test('checking phone device trust is explicit read-only consent and cannot start a receiver or silently enable hosting', () => {
  const checking: AppLoginDraft = { ...draft, intent: 'check_device_trust', startReceiver: false, deviceMode: 'phone_app',
    deviceId: 'D5C0199B-1644-4D33-8912-B8BD5AB49224' }
  const input = jegotripLoginInput(id, checking)
  assert.equal(input.intent, 'check_device_trust'); assert.equal(input.start_receiver, false); assert.equal(input.consent, true)
  assert.equal('enable_hosting' in input, false)
  assert.equal(canSubmitJegotripLogin({ ...checking, startReceiver: true }), false)
  assert.equal(canSubmitJegotripLogin({ ...checking, consent: false }), false)
  assert.equal(emptyAppLoginDraft().intent, 'enable_hosting')
})
void test('device trust labels describe a server observation and preserve missing failed and untrusted outcomes', () => {
  const observed = { state: 'observed' as const, authentication_code: 1, error: null, server_code: null }
  assert.match(deviceTrustQueryLabel(observed), /本次返回 1/)
  assert.equal(deviceTrustQueryLabel(observed).includes('就绪'), false)
  assert.match(deviceTrustQueryLabel({ ...observed, authentication_code: 2 }), /未认可/)
  assert.match(deviceTrustQueryLabel({ ...observed, authentication_code: null }), /未返回/)
  assert.match(deviceTrustQueryLabel({ ...observed, authentication_code: 999999999 }), /未返回/)
  assert.match(deviceTrustQueryLabel({ ...observed, state: 'not_configured', authentication_code: null }), /状态未知/)
  const failed = deviceTrustQueryLabel({ ...observed, state: 'failed', authentication_code: null, error: 'https://private.invalid/?token=secret' })
  assert.match(failed, /查询失败/); assert.equal(failed.includes('secret'), false); assert.equal(failed.includes('private.invalid'), false)
})

void test('trusted reuse intents require the explicit phone UUID mode and never silently generate or substitute an identity', () => {
  for (const intent of ['reuse_hosting', 'enable_trusted_hosting'] as const) {
    const value: AppLoginDraft = { ...draft, intent, deviceMode: 'phone_app', deviceId: 'D5C0199B-1644-4D33-8912-B8BD5AB49224' }
    assert.equal(canSubmitJegotripLogin(value), true)
    const input = jegotripLoginInput(id, value)
    assert.equal(input.intent, intent); assert.equal(input.device_id, value.deviceId)
    for (const field of ['trust_granted', 'sso_token', 'enrollId', 'device_sync_profile', 'sdk_device_id']) assert.equal(field in input, false)
    for (const deviceMode of ['auto', 'manual'] as const) assert.equal(canSubmitJegotripLogin({ ...value, deviceMode }), false)
    assert.equal(canSubmitJegotripLogin({ ...value, consent: false }), false)
    assert.equal(canSubmitJegotripLogin({ ...value, deviceId: '' }), false)
    assert.throws(() => jegotripLoginInput(id, { ...value, deviceId: 'fixture-sdk-device' }))
  }
})
void test('phone UUID selection cannot accidentally keep the ordinary enrolling enable or attach intent', () => {
  const value: AppLoginDraft = { ...draft, deviceMode: 'phone_app', deviceId: 'D5C0199B-1644-4D33-8912-B8BD5AB49224' }
  assert.equal(canSubmitJegotripLogin(value), false)
  assert.equal(canSubmitJegotripLogin({ ...value, intent: 'enable_hosting' }), false)
  assert.equal(canSubmitJegotripLogin({ ...value, intent: 'attach_hosting' }), false)
  assert.equal(canSubmitJegotripLogin({ ...value, intent: 'check_device_trust', startReceiver: false }), true)
})
void test('device sync and skipped endpoint labels never present report acceptance as a trust or live receiver grant', () => {
  const skipped = { state: 'not_requested' as const, authentication_code: null, error: null, server_code: null }
  assert.match(deviceTrustQueryLabel(skipped), /未请求/)
  assert.match(deviceSyncLabel({ state: 'accepted', error: null, server_code: null }), /不是授信或收件成功/)
  assert.match(deviceSyncLabel({ state: 'not_configured', error: null, server_code: null }), /未发送/)
  assert.match(deviceSyncLabel({ state: 'request_uncertain', error: null, server_code: null }), /不会自动重放/)
  const rejected = deviceSyncLabel({ state: 'rejected', error: 'https://secret.invalid/?token=PRIVATE-TOKEN', server_code: 506 })
  assert.equal(rejected.includes('PRIVATE-TOKEN'), false); assert.equal(rejected.includes('secret.invalid'), false)
  assert.equal(appLoginPhase('syncing_device').includes('就绪'), false)
})
