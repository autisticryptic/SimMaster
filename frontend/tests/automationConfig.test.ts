import assert from 'node:assert/strict'
import test from 'node:test'
import { persistAutomationConfig } from '../src/utils/automationConfig.ts'
import type { AutomationConfig } from '../src/api/contracts.ts'

const config: AutomationConfig = { enabled: false, tasks: [] }

void test('saving a task preserves the scheduler enable flag', async () => {
  const saved = await persistAutomationConfig(config, (sent) => {
    assert.equal(sent.enabled, false)
    assert.deepEqual(sent.tasks, [])
    return Promise.resolve({ status: 'ok', message: 'saved' })
  })
  assert.equal(saved, config)
})

void test('backend rejection reaches the dialog without success or close', async () => {
  let success = false
  let closed = false
  const save = async () => {
    await persistAutomationConfig(config, () => Promise.resolve({ status: 'error', message: 'automation_dial_call_invalid' }))
    success = true
    closed = true
  }
  await assert.rejects(save(), /automation_dial_call_invalid/)
  assert.equal(success, false)
  assert.equal(closed, false)
})

void test('network errors and empty rejection messages are not accepted as saves', async () => {
  await assert.rejects(persistAutomationConfig(config, () => Promise.reject(new Error('offline'))), /offline/)
  await assert.rejects(persistAutomationConfig(config, () => Promise.resolve({ status: 'error', message: '' })), /自动化配置保存失败/)
})
