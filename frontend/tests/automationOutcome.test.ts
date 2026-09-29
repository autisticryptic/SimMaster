import assert from 'node:assert/strict'
import test from 'node:test'
import { automationOutcomeLabel } from '../src/utils/automationOutcome.ts'

void test('successful no-answer task keeps the unanswered result visible', () => {
  const detail = '拨号任务成功：对方未接听／对端无应答超时；call_outcome=peer_no_answer；answered_observed=false；SIP=408；Q.850=31；不表示已接通'
  const label = automationOutcomeLabel('success', detail)
  assert.match(label, /^上次成功:/)
  assert.match(label, /对方未接听/)
  assert.match(label, /SIP=408/)
  assert.match(label, /不表示已接通/)
  assert.doesNotMatch(label, /上次失败/)
})

void test('generic tasks remain concise and real failures are not converted', () => {
  assert.equal(automationOutcomeLabel('success', '执行成功'), '上次成功')
  assert.equal(automationOutcomeLabel('success', ''), '上次成功')
  assert.match(automationOutcomeLabel('failed', 'voice_registered_home_required'), /^上次失败:.*非漫游/)
  assert.match(automationOutcomeLabel('failed', 'automation_call_delivery_unconfirmed'), /^上次失败:/)
  assert.match(automationOutcomeLabel('failed', '执行超时 (超过120秒限制)'), /^上次失败:/)
})
