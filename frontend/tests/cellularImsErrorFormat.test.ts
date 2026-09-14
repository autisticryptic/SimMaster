import assert from 'node:assert/strict'
import test from 'node:test'
import {
  cellularImsErrorMessage,
  cellularImsErrorStatusLabel,
} from '../src/pages/sim/cellularImsErrorFormat.ts'

await test('provider-declared baseband failure stops the entire retry batch in the UI', () => {
  for (const detail of ['opaque failure', 'prefix-unavailable', 'ServiceOptionNotSubscribed', 'volte_register_refresh_retry']) {
    const error = `volte_runtime_ims_baseband_wedged:${detail}`
    assert.equal(cellularImsErrorStatusLabel(error), '基带异常，已停止重试')
    assert.match(cellularImsErrorMessage(error) ?? '', /停止本轮所有 Profile 和地址族重试/)
  }
})

await test('ordinary bearer failure and transient refresh diagnostics retain their meaning', () => {
  assert.equal(cellularImsErrorStatusLabel('volte_runtime_ims_bearer_start_failed'), 'IMS Bearer 建立失败')
  assert.equal(cellularImsErrorStatusLabel('volte_register_refresh_retry'), null)
  assert.equal(cellularImsErrorMessage('volte_register_refresh_retry'), null)
  assert.equal(cellularImsErrorMessage(null), null)
})
