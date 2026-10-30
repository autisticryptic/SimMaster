import assert from 'node:assert/strict'
import test from 'node:test'
import { appLoginInput, canSubmitAppLogin, jegotripLoginInput, canSubmitJegotripLogin, emptyAppLoginDraft, appLoginError, appBusinessLabel, appLoginPhase, appLoginNeedsPolling, type AppLoginDraft } from '../src/policies/appLogin.ts'
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
