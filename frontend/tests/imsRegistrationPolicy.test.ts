import assert from 'node:assert/strict'
import test from 'node:test'
import {
  humanizeCostPolicyError,
  isSelectableRegistrationPreference,
  orderedVoicePaths,
  registrationModeOptions,
  registrationPreferenceLabel,
  registrationSupportText,
} from '../src/policies/imsRegistration.ts'

await test('only AUTO and single-WLAN are offered, without changing legacy saved values', () => {
  assert.deepEqual(registrationModeOptions.map((item) => item.value), ['concurrent', 'wlan_preferred'])
  assert.equal(isSelectableRegistrationPreference('concurrent'), true)
  assert.equal(isSelectableRegistrationPreference('wlan_preferred'), true)
  assert.equal(isSelectableRegistrationPreference('cellular_preferred'), false)
  assert.equal(isSelectableRegistrationPreference('force_dual'), false)
  assert.match(registrationPreferenceLabel('cellular_preferred'), /旧设置/)
})

await test('a failed additional flow is not reported as successful dual registration', () => {
  assert.match(registrationSupportText({
    requested: 'concurrent', concurrent_support: 'negotiated', multiple_registration_blocked: true,
  }), /单注册/)
  assert.match(registrationSupportText({
    requested: 'concurrent', concurrent_support: 'not_supported',
  }), /当前网络/)
  assert.match(registrationSupportText({
    requested: 'concurrent', concurrent_support: 'not_negotiated',
  }), /不能把等待或超时/)
  assert.match(registrationSupportText({
    requested: 'wlan_preferred', concurrent_support: 'negotiated',
  }), /不追加第二路/)
})

await test('cost restrictions produce clear Chinese errors without hiding unrelated failures', () => {
  assert.match(humanizeCostPolicyError('send failed;sms_vowifi_only_required'), /未回退/)
  assert.match(humanizeCostPolicyError('ims unavailable;voice_vowifi_only_required'), /阻止蜂窝/)
  assert.equal(humanizeCostPolicyError('unrelated_error'), 'unrelated_error')
})

await test('voice controls display effective WiFi priority without changing enabled flags or storage order', () => {
  const stored = [{ kind: 'volte', enabled: true }, { kind: 'vowifi', enabled: false }] as const
  const displayed = orderedVoicePaths(stored)
  assert.deepEqual(displayed, [{ kind: 'vowifi', enabled: false }, { kind: 'volte', enabled: true }])
  assert.equal(stored[0].kind, 'volte')
  assert.equal(stored[1].enabled, false)
})
