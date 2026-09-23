import assert from 'node:assert/strict'
import test from 'node:test'
import {
  cellularImsErrorCodes,
  cellularImsErrorMessage,
  cellularImsErrorStatusLabel,
} from '../src/pages/sim/cellularImsErrorFormat.ts'

await test('provider-declared baseband failure stops the entire retry batch in the UI', () => {
  for (const detail of ['opaque failure', 'prefix-unavailable', 'ServiceOptionNotSubscribed', 'cellular_ims_register_refresh_retry']) {
    const error = `cellular_ims_runtime_ims_baseband_wedged:${detail}`
    assert.equal(cellularImsErrorStatusLabel(error), '基带异常，已停止重试')
    assert.match(cellularImsErrorMessage(error) ?? '', /停止本轮所有 Profile 和地址族重试/)
  }
})

await test('ordinary bearer failure and transient refresh diagnostics retain their meaning', () => {
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_runtime_ims_bearer_start_failed'), 'IMS Bearer 建立失败')
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_register_refresh_retry'), null)
  assert.equal(cellularImsErrorMessage('cellular_ims_register_refresh_retry'), null)
  assert.equal(cellularImsErrorMessage(null), null)
})

await test('codes are matched as whole tokens, never as substrings', () => {
  // A known code embedded in a longer identifier is a different value.
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_carrier_profile_missing_extra'), null)
  assert.equal(cellularImsErrorCodes('cellular_ims_carrier_profile_missing_extra').size, 0)
  // Codes nested inside a detail chain are still found.
  assert.equal(
    cellularImsErrorStatusLabel('cellular_ims_register_refresh_failed:expired:cellular_ims_digest_nonce_missing'),
    'IMS 鉴权响应异常',
  )
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_command_failed:ip -6 route replace 2400:9380::1/128'), null)
  // The one code that carries a colon of its own.
  assert.ok(cellularImsErrorCodes('cellular_ims_dependency_missing:ip').has('cellular_ims_dependency_missing:ip'))
  assert.equal(cellularImsErrorCodes('cellular_ims_dependency_missing:ipx').size, 0)
})

await test('former prefix families are enumerated member by member', () => {
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_digest_realm_missing'), 'IMS 鉴权响应异常')
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_register_nonce_not_aka'), 'IMS 鉴权响应异常')
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_ipsec_udp_bind_failed:eaddrinuse'), 'IMS IPsec 建立失败')
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_security_server_missing'), 'IMS IPsec 建立失败')
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_bearer_netdev_not_ready'), '基带数据通道异常')
  assert.equal(cellularImsErrorStatusLabel('cellular_ims_runtime_mm_bearer_roaming_forbidden'), 'IMS 漫游被禁止')
})
