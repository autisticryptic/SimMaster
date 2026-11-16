import test from 'node:test'
import assert from 'node:assert/strict'
import { USER_READ_SCOPE, userReadRows, userReadLabel } from '../src/policies/imUserRead.ts'
const row = { id: 2, message_id: 1, target_index: '9223372036854775807', state: 'confirmed', updated_at: '2026-01-01T00:00:00Z' }
void test('user read outcome preserves complete index and distinguishes explicit decision from received position or media', () => {
  assert.equal(userReadRows([row])[0].index, '9223372036854775807'); assert.match(USER_READ_SCOPE, /不是接收位置回执/); assert.match(userReadLabel('confirmed'), /用户已读操作/)
})
void test('unknown user read remains query only and never a replayable write or received grant', () => {
  assert.match(userReadLabel('uncertain'), /仅查询，不重放/); assert.match(userReadLabel('rejected'), /拒绝/); assert.match(userReadLabel('not_sent'), /未写出/)
})
void test('user read metadata has own properties and never reflects nonce raw box UID or response text', () => {
  const rows = userReadRows([{ ...row, nonce: 'private-nonce', box_id: 'private-box', uid: 'private-user' }, Object.create(row)]); assert.equal(rows.length, 1); assert.equal(JSON.stringify(rows).includes('private-'), false)
})
void test('user read unsafe IDs ambiguous indices and unknown states cannot masquerade as acknowledged', () => {
  for (const id of [0, -1, 1.2, Number.MAX_SAFE_INTEGER + 1]) assert.equal(userReadRows([{ ...row, id }]).length, 0)
  for (const target_index of ['-1', '01', '+1', '9223372036854775808', 7, '']) assert.equal(userReadRows([{ ...row, target_index }]).length, 0)
  for (const state of ['ready', 'delivered', 'constructor', true]) assert.equal(userReadRows([{ ...row, state }]).length, 0)
})
void test('user read input output duplicate IDs and unknown timestamps are bounded independently of message text', () => {
  assert.equal(userReadRows(Array.from({ length: 1000 }, (_, i) => ({ ...row, id: i + 1 }))).length, 16); assert.equal(userReadRows([row, row]).length, 1); assert.equal(userReadRows([{ ...row, updated_at: 'private-date' }])[0].updatedAt, '本地观察时间未知')
})
