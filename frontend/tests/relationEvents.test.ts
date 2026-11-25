import test from 'node:test'
import assert from 'node:assert/strict'
import { relationEventRows, relationEventLabel, RELATION_EVENTS_SCOPE, RELATION_EVENTS_UNKNOWN } from '../src/policies/relationEvents.ts'
const row = { id: 1, target_kind: 'group', updates: 1, removals: 2, base_time: '-9223372036854775808', update_time: '9223372036854775807', continuity: 'unseeded', full_roster_verified: false, received_at: '2026-01-01T00:00:00Z' }
void test('relation events retain signed64 strings as partial inbound data not outbound outcomes', () => {
  const rows = relationEventRows([row]); assert.equal(rows?.length, 1); assert.equal(rows?.[0].baseTime, row.base_time); assert.equal(rows?.[0].updateTime, row.update_time); assert.equal(rows?.[0].fullRosterVerified, false); assert.equal(rows?.[0].targetKind, 'group'); assert.deepEqual(relationEventRows([]), [])
})
void test('relation events require exact own nine fields without private identity or readiness', () => {
  assert.equal(Object.keys(row).length, 9); assert.equal(relationEventRows([Object.create(row) as unknown]), null)
  for (const key of Object.keys(row)) { const value: Record<string, unknown> = { ...row }; delete value[key]; assert.equal(relationEventRows([value]), null) }
  for (const extra of [{ uid: 'private-uid' }, { token: 'private-token' }, { body: 'private-body' }, { nonce: 'a'.repeat(32) }, { ready: true }, { state: 'confirmed' }, { configuration: {} }, { [Symbol('extra')]: 1 }]) assert.equal(relationEventRows([{ ...row, ...extra }]), null)
})
void test('relation events reject unsafe duplicate ids and excessive snapshots rather than coerce values', () => {
  for (const id of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, '1', null, true]) assert.equal(relationEventRows([{ ...row, id }]), null)
  assert.equal(relationEventRows([row, row]), null); assert.equal(relationEventRows(Array.from({ length: 129 }, (_, n) => ({ ...row, id: n + 1 }))), null)
  for (const value of [null, {}, 'private-response', [null], [[]]]) assert.equal(relationEventRows(value), null)
})
void test('relation events count partial updates and removals together without inferring complete membership', () => {
  for (const key of ['updates', 'removals']) for (const value of [-1, 129, 0.5, '1', true, null]) assert.equal(relationEventRows([{ ...row, [key]: value }]), null)
  assert.equal(relationEventRows([{ ...row, updates: 128, removals: 1 }]), null); assert.equal(relationEventRows([{ ...row, updates: 127, removals: 1 }])?.length, 1)
  assert.equal(relationEventRows([{ ...row, updates: 0, removals: 0 }])?.[0].fullRosterVerified, false)
})
void test('relation events require canonical signed64 and conservative increasing local admission times', () => {
  for (const key of ['base_time', 'update_time']) for (const value of ['01', '-0', '+1', ' 1', '9223372036854775808', '-9223372036854775809', 1, null]) assert.equal(relationEventRows([{ ...row, [key]: value }]), null)
  assert.equal(relationEventRows([{ ...row, base_time: '1', update_time: '1' }]), null); assert.equal(relationEventRows([{ ...row, base_time: '2', update_time: '1' }]), null)
  assert.equal(relationEventRows([{ ...row, base_time: '-1', update_time: '0' }])?.[0].updateTime, '0')
})
void test('relation events unseeded partial gap and stale stay false roster and never confirmed WRITE', () => {
  for (const continuity of ['unseeded', 'partial_chain', 'gap', 'stale']) assert.equal(relationEventRows([{ ...row, continuity, target_kind: 'buddy' }])?.[0].continuity, continuity)
  for (const continuity of ['ready', 'confirmed', true, null, 'complete']) assert.equal(relationEventRows([{ ...row, continuity }]), null)
  for (const full_roster_verified of [true, 'false', 0, null]) assert.equal(relationEventRows([{ ...row, full_roster_verified }]), null)
  for (const target_kind of ['p2p', 'current_uid', true, null]) assert.equal(relationEventRows([{ ...row, target_kind }]), null)
})
void test('relation event reception timestamp rejects private text invalid calendar and excessive values', () => {
  for (const received_at of ['', 'private-token', 'x'.repeat(65), null, 1, '2026-02-29T00:00:00Z', '2026-01-32T00:00:00Z', '2026-13-01T00:00:00Z', '2026-01-01T24:00:00Z', '2026-01-01T00:60:00Z', '2026-01-01T00:00:00+24:00']) assert.equal(relationEventRows([{ ...row, received_at }]), null)
  assert.equal(relationEventRows([{ ...row, received_at: '2024-02-29T01:02:03.123456789+08:00' }])?.length, 1)
})
void test('relation event copy owns finite projection and cannot mutate original server data', () => {
  const raw = { ...row }; const result = relationEventRows([raw]); assert.ok(result); result[0].updates = 8; assert.equal(raw.updates, 1); raw.continuity = 'gap'; assert.equal(result[0].continuity, 'unseeded'); assert.equal(Object.keys(result[0]).length, 9)
})
void test('relation event scope explicitly refuses retry nonce clearing Unknown WRITE and roster grants', () => {
  assert.match(RELATION_EVENTS_SCOPE, /局部 DATA/); assert.match(RELATION_EVENTS_SCOPE, /不是完整关系名册/); assert.match(RELATION_EVENTS_UNKNOWN, /均不解决 Unknown-WRITE/); assert.match(RELATION_EVENTS_UNKNOWN, /不会自动查询、重试、重新登录/)
  assert.match(relationEventLabel('gap'), /不自动推进/); assert.match(relationEventLabel('partial_chain'), /不是完整关系名册/); assert.match(relationEventLabel('unseeded'), /未建立基线/); assert.match(relationEventLabel('stale'), /不作为新的当前状态/)
})
