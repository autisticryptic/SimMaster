import test from 'node:test'
import assert from 'node:assert/strict'
import { relationHistoryRows, relationHistoryLabel, RELATION_HISTORY_SCOPE } from '../src/policies/relationHistory.ts'
const row = { id: 1, nonce_tag: 'a'.repeat(64), target_kind: 'buddy', state: 'confirmed', additions: 1, updates: 2, removals: 3, base_time: '-9223372036854775808', update_time: '9223372036854775807', updated_at: '2026-01-01T00:00:00Z' }
void test('relation history preserves exact signed64 DATA and finite original result counts', () => {
  const data = relationHistoryRows([row]); assert.equal(data?.[0].baseTime, '-9223372036854775808'); assert.equal(data?.[0].updateTime, '9223372036854775807'); assert.equal(data?.[0].targetKind, 'buddy'); assert.equal(data?.[0].removals, 3)
  assert.equal(relationHistoryRows([{ ...row, target_kind: 'group', state: 'rejected' }])?.[0].state, 'rejected')
  assert.deepEqual(relationHistoryRows([]), [])
})
void test('relation history unknown and not_sent outcomes have no fabricated signed times or native permission', () => {
  for (const state of ['uncertain', 'not_sent']) {
    assert.equal(relationHistoryRows([{ ...row, state, base_time: null, update_time: null }])?.[0].state, state)
    assert.equal(relationHistoryRows([{ ...row, state }]), null)
    assert.equal(relationHistoryRows([{ ...row, state, base_time: null }]), null)
  }
  assert.match(relationHistoryLabel('uncertain'), /不重放/); assert.match(relationHistoryLabel('confirmed'), /不是在线或送达/)
})
void test('relation history rejects inherited missing fields extra private values and ready grant projections', () => {
  assert.equal(relationHistoryRows([Object.create(row) as unknown]), null)
  for (const key of Object.keys(row)) { const bad: Record<string, unknown> = { ...row }; delete bad[key]; assert.equal(relationHistoryRows([bad]), null) }
  for (const extra of [{ uid: 'private-uid' }, { token: 'private-token' }, { relations: 'private-label' }, { owner: 'private-owner' }, { ready: true }, { grant: true }, { nonce: 'a'.repeat(32) }]) assert.equal(relationHistoryRows([{ ...row, ...extra }]), null)
})
void test('relation history refuses malformed or unsafe ids nonce tags duplicate identities and unbounded snapshots', () => {
  for (const id of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, '1', null]) assert.equal(relationHistoryRows([{ ...row, id }]), null)
  for (const nonce_tag of ['', 'a'.repeat(63), 'A'.repeat(64), 'g'.repeat(64), null]) assert.equal(relationHistoryRows([{ ...row, nonce_tag }]), null)
  assert.equal(relationHistoryRows([row, { ...row, nonce_tag: 'b'.repeat(64) }]), null)
  assert.equal(relationHistoryRows([row, { ...row, id: 2 }]), null)
  assert.equal(relationHistoryRows(Array.from({ length: 129 }, (_, id) => ({ ...row, id: id + 1 }))), null)
  for (const bad of [null, {}, 'private-response']) assert.equal(relationHistoryRows(bad), null)
})
void test('relation history requires real finite change counts rather than empty permissions or truthy counters', () => {
  for (const key of ['additions', 'updates', 'removals']) for (const value of [-1, 129, 0.5, '1', true, null]) assert.equal(relationHistoryRows([{ ...row, [key]: value }]), null)
  assert.equal(relationHistoryRows([{ ...row, additions: 0, updates: 0, removals: 0 }]), null)
  assert.equal(relationHistoryRows([{ ...row, additions: 128, updates: 128, removals: 128 }])?.length, 1)
})
void test('relation history refuses signed64 coercion rounding partial times and invented result states', () => {
  for (const key of ['base_time', 'update_time']) for (const value of ['01', '-0', '+1', '9223372036854775808', '-9223372036854775809', 1, null]) assert.equal(relationHistoryRows([{ ...row, [key]: value }]), null)
  for (const state of ['online', 'ready', true, null]) assert.equal(relationHistoryRows([{ ...row, state }]), null)
  for (const target_kind of ['p2p', 'current_user', 'endpoint', true, null]) assert.equal(relationHistoryRows([{ ...row, target_kind }]), null)
})
void test('relation history local timestamp is bounded DATA and is not rendered as private or online time', () => {
  for (const updated_at of ['', 'private-token', 'x'.repeat(65), 1, null]) assert.equal(relationHistoryRows([{ ...row, updated_at }]), null)
  const value = relationHistoryRows([{ ...row, updated_at: '2026-01-01T00:00:00.123456789+08:00' }]); assert.equal(value?.length, 1)
  assert.doesNotMatch(JSON.stringify(value), /updated_at|2026-01|ready|grant|uid|token/)
})
void test('relation history scope explicitly separates read-only records from SDK WRITE and nonce authority', () => {
  assert.match(RELATION_HISTORY_SCOPE, /仅查询既有 Group\/Buddy/); assert.match(RELATION_HISTORY_SCOPE, /不授予 SDK/); assert.match(RELATION_HISTORY_SCOPE, /不生成新 nonce/); assert.match(RELATION_HISTORY_SCOPE, /不解决或重放未知 WRITE/)
  assert.match(relationHistoryLabel('rejected'), /拒绝关系修改/); assert.match(relationHistoryLabel('not_sent'), /未写出/)
})
