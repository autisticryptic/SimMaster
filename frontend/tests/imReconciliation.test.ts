import test from 'node:test'
import assert from 'node:assert/strict'
import { imReconciliationView, imReconciliationLabel, IM_RECONCILIATION_SCOPE } from '../src/policies/imReconciliation.ts'
const raw = { receipt_id: 1, message_id: 2, target_index: '9223372036854775807', local_state: 'uncertain', report: 'reported', reported_last_read: '9223372036854775807', reported_last_received: '8', reported_latest: '9223372036854775807', server_update_time: '4294967297', observed_at: '2026-01-01T00:00:00Z' }
void test('IM reconciliation preserves full index and reports observation without changing unknown original write', () => {
  const value = imReconciliationView(raw); assert.ok(value); assert.equal(value.targetIndex, raw.target_index); assert.equal(value.localState, 'uncertain'); assert.match(imReconciliationLabel(value), /不是原写入匹配回执/); assert.match(IM_RECONCILIATION_SCOPE, /不清空未知写入、不重放/)
})
void test('missing lower negative and rejected observations are never evidence of original no IO', () => {
  for (const last of [null, '-1', '7']) { const value = imReconciliationView({ ...raw, reported_last_read: last }); assert.ok(value); assert.match(imReconciliationLabel(value), /不等于原写入未执行/); assert.equal(value.localState, 'uncertain') }
  const value = imReconciliationView({ ...raw, report: 'rejected', reported_last_read: null, reported_last_received: null, reported_latest: null, server_update_time: null }); assert.ok(value); assert.match(imReconciliationLabel(value), /不能判断原写入是否执行/)
})
void test('reconciliation refuses unsafe IDs ambiguous indices missing fields and inconsistent reports', () => {
  for (const receipt_id of [0, -1, 1.2, Number.MAX_SAFE_INTEGER + 1]) assert.equal(imReconciliationView({ ...raw, receipt_id }), null)
  for (const target_index of ['-1', '01', '+1', '9223372036854775808', 7]) assert.equal(imReconciliationView({ ...raw, target_index }), null)
  assert.equal(imReconciliationView({ ...raw, reported_last_read: undefined }), null); assert.equal(imReconciliationView({ ...raw, report: 'unreported' }), null)
})
void test('only own finite observation fields render and private context or prototype values do not leak', () => {
  assert.equal(imReconciliationView(Object.create(raw)), null); assert.equal(imReconciliationView({ ...raw, report: 'constructor' }), null)
  const value = imReconciliationView({ ...raw, uid: 'private-user', token: 'private-token', box_id: 'private-box', user_nonce: 'private-nonce' }); assert.ok(value); assert.equal(JSON.stringify(value).includes('private-'), false)
})
void test('reconciliation observation time is bounded and readiness or write fields cannot turn into an ACK', () => {
  for (const observed_at of ['private-time', 'x'.repeat(65), '2026-01-01\0']) assert.equal(imReconciliationView({ ...raw, observed_at }), null)
  assert.equal(imReconciliationView({ ...raw, local_state: 'ready' }), null); assert.equal(imReconciliationView({ ...raw, reported_last_read: true }), null)
})
