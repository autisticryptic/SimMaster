import assert from 'node:assert/strict'
import test from 'node:test'
import {
  humanizeCostPolicyError,
  hasConfirmedDualRegistration,
  orderedVoicePaths,
  registrationSupportText,
} from '../src/policies/imsRegistration.ts'

await test('dual registration requires both switches, both successful flows and negotiation', () => {
  const ready = { cellularEnabled: true, wlanEnabled: true, cellularRegistered: true, wlanRegistered: true,
    cellularValidated: true, wlanValidated: true, negotiated: true, blocked: false }
  assert.equal(hasConfirmedDualRegistration(ready), true)
  for (const key of ['cellularEnabled', 'wlanEnabled', 'cellularRegistered', 'wlanRegistered', 'cellularValidated', 'wlanValidated', 'negotiated'] as const) {
    assert.equal(hasConfirmedDualRegistration({ ...ready, [key]: false }), false)
  }
  assert.equal(hasConfirmedDualRegistration({ ...ready, blocked: true }), false)
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
  }), /两路开关都开启/)
})

await test('cost restrictions produce clear Chinese errors without hiding unrelated failures', () => {
  assert.match(humanizeCostPolicyError('send failed;sms_vowifi_only_required'), /未回退/)
  assert.match(humanizeCostPolicyError('ims unavailable;voice_vowifi_only_required'), /阻止蜂窝/)
  assert.match(humanizeCostPolicyError('voice_registered_home_required'), /非漫游/)
  assert.match(humanizeCostPolicyError('voice_call_binding_changed'), /SIM 已变化/)
  assert.match(humanizeCostPolicyError('ims_registration_mode_automatic_only'), /不再支持/)
  assert.equal(humanizeCostPolicyError('unrelated_error'), 'unrelated_error')
})

await test('voice controls display effective WiFi priority without changing enabled flags or storage order', () => {
  const stored = [{ kind: 'cellular_ims', enabled: true }, { kind: 'vowifi', enabled: false }] as const
  const displayed = orderedVoicePaths(stored)
  assert.deepEqual(displayed, [{ kind: 'vowifi', enabled: false }, { kind: 'cellular_ims', enabled: true }])
  assert.equal(stored[0].kind, 'cellular_ims')
  assert.equal(stored[1].enabled, false)
})
