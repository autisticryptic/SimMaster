import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { captureRelationWriteBody, relationWriteBody, relationIntentRows, relationSnapshot, relationSnapshotClear, relationMarker, relationMarkerResolved, relationNonceTags, type RelationDraft, type RelationLocalMarker } from '../src/policies/relationWrite.ts'
const draft: RelationDraft = { target: 'buddy', groupId: '', additions: JSON.stringify([{ label: 'private-label', relation_type: 258, first: '备注', properties: [{ key: [0, 255], value: [128] }] }]), updates: '[]', removals: JSON.stringify([{ label: 'remove-label' }]) }
const intent = { id: 2, nonce_tag: 'a'.repeat(64), state: 'bound', write_id: 4, target_kind: 'buddy', additions: 1, updates: 0, removals: 1, updated_at: '2026-01-01T00:00:00Z' }
const write = { id: 4, nonce_tag: 'b'.repeat(64), state: 'confirmed', target_kind: 'buddy', additions: 1, updates: 0, removals: 1, base_time: '-9223372036854775808', update_time: '9223372036854775807', updated_at: '2026-01-01T00:00:00Z' }
const marker: RelationLocalMarker = { version: 1, intentTag: intent.nonce_tag, writeTag: write.nonce_tag, revision: 1, afterIntent: 1, afterWrite: 3, target: 'buddy', additions: 1, updates: 0, removals: 1 }
void test('relation WRITE body keeps explicit MTC type DATA original false removal and own nonce consent only', () => {
  const body = relationWriteBody(1, 'a'.repeat(32), draft, true, true); assert.ok(body)
  assert.equal(body.group_id, null); assert.equal(body.additions[0].relation_type, 258); assert.equal(body.additions[0].first, '备注'); assert.equal(body.additions[0].second, ''); assert.equal(body.removals[0].flag, false); assert.equal(body.consent, true); assert.equal(body.write_consent, true)
  assert.doesNotMatch(JSON.stringify(body), /token|uid|selector|owner|endpoint|ready|grant/)
  assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, target: 'group', groupId: '1+2' }, true, true)?.group_id, '1+2')
})
void test('relation WRITE validators reject absent or ambiguous consent nonce revision and implicit target conversion', () => {
  for (const args of [[false, true], [true, false], [false, false]]) assert.equal(relationWriteBody(1, 'a'.repeat(32), draft, args[0], args[1]), null)
  for (const revision of [0, -1, 0.5, Number.MAX_SAFE_INTEGER + 1]) assert.equal(relationWriteBody(revision, 'a'.repeat(32), draft, true, true), null)
  for (const nonce of ['', 'a'.repeat(31), 'A'.repeat(32), 'x'.repeat(32)]) assert.equal(relationWriteBody(1, nonce, draft, true, true), null)
  for (const groupId of ['', '1_2', 'Org/1-2', '01-2', '1-02', 'Group:tcp -h private.invalid -p 1;']) assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, target: 'group', groupId }, true, true), null)
  assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, groupId: '1-2' }, true, true), null)
})
void test('relation WRITE JSON shape refuses credentials extra fields malformed bytes and native type guesses', () => {
  for (const entry of [{ label: 'x', relation_type: 1 }, { label: 'x', relation_type: 17 }, { label: 'x', relation_type: 20 }, { label: 'x', relation_type: 258, token: 'private' }, { label: 'x', relation_type: 258, first: null }, { label: 'x', relation_type: 258, properties: [{ key: [256], value: [] }] }, { label: 'x', relation_type: 258, properties: [{ key: [1], value: [-1] }] }, { label: 'x', relation_type: 258, properties: [{ key: [], value: [], grant: true }] }]) assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, additions: JSON.stringify([entry]) }, true, true), null)
  for (const extra of [{ uid: 'private' }, { token: 'private' }, { native_ready: true }, { nonce: 'private' }]) assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, ...extra }, true, true), null)
  assert.equal(relationWriteBody(1, 'a'.repeat(32), Object.create(draft), true, true), null)
  assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, additions: '[broken' }, true, true), null)
})
void test('relation WRITE maps are bounded disjoint immutable and do not normalize exact private labels', () => {
  const input = { ...draft, additions: JSON.stringify([{ label: ' exact\0label ', relation_type: 257 }]) }; const body = relationWriteBody(1, 'a'.repeat(32), input, true, true); assert.ok(body); assert.equal(body.additions[0].label, ' exact\0label ')
  assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, updates: draft.additions }, true, true), null)
  assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, additions: '[]', removals: '[]' }, true, true), null)
  assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, additions: JSON.stringify(Array.from({ length: 129 }, (_, n) => ({ label: `x${n}`, relation_type: 258 }))) }, true, true), null)
  assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, additions: JSON.stringify([{ label: 'x', relation_type: 258, first: 'x'.repeat(65537) }]) }, true, true), null)
  assert.equal(relationWriteBody(1, 'a'.repeat(32), { ...draft, additions: JSON.stringify([{ label: '\ud800', relation_type: 258 }]) }, true, true), null)
  const copy = captureRelationWriteBody(body); assert.ok(copy); body.additions[0].label = 'changed'; assert.equal(copy.additions[0].label, ' exact\0label ')
})
void test('relation WRITE payload cloning refuses authority or consent injected into typed runtime input', () => {
  const body = relationWriteBody(1, 'a'.repeat(32), draft, true, true); assert.ok(body)
  for (const extra of [{ uid: 'forged' }, { endpoint: 'forged' }, { consent: false }, { write_consent: false }, { nonce: 'A'.repeat(32) }]) assert.equal(captureRelationWriteBody({ ...body, ...extra }), null)
  assert.equal(captureRelationWriteBody(Object.create(body)), null)
})
void test('relation preparation projection is finite duplicate-safe and never carries native auth or body secrets', () => {
  assert.equal(relationIntentRows([intent])?.[0].writeId, 4)
  assert.equal(relationIntentRows([{ ...intent, state: 'preparing', write_id: null }])?.[0].state, 'preparing')
  for (const value of [Object.create(intent), { ...intent, id: 0 }, { ...intent, state: 'ready' }, { ...intent, state: 'preparing' }, { ...intent, state: 'not_sent' }, { ...intent, write_id: null }, { ...intent, token: 'private' }, { ...intent, additions: 129 }, { ...intent, updated_at: 'private' }]) assert.equal(relationIntentRows([value]), null)
  assert.equal(relationIntentRows([intent, intent]), null); assert.equal(relationIntentRows([intent, { ...intent, id: 3, nonce_tag: 'c'.repeat(64) }]), null)
})
void test('relation complete snapshot fails closed for missing mismatched bound results and unresolved legacy writes', () => {
  const snapshot = relationSnapshot([intent], [write]); assert.ok(snapshot); assert.equal(relationSnapshotClear(snapshot), true)
  assert.equal(relationSnapshot([intent], []), null); assert.equal(relationSnapshot([intent], [{ ...write, additions: 2 }]), null)
  const legacy = relationSnapshot([], [{ ...write, state: 'uncertain', base_time: null, update_time: null }]); assert.ok(legacy); assert.equal(relationSnapshotClear(legacy), false)
  const preparing = relationSnapshot([{ ...intent, state: 'preparing', write_id: null }], []); assert.ok(preparing); assert.equal(relationSnapshotClear(preparing), false)
})
void test('relation local marker is opaque bounded correlation only and refuses raw nonce labels or generic success', () => {
  assert.deepEqual(relationMarker(marker), marker)
  for (const value of [Object.create(marker), { ...marker, nonce: 'a'.repeat(32) }, { ...marker, label: 'private-label' }, { ...marker, revision: 0 }, { ...marker, intentTag: 'bad' }, { ...marker, afterWrite: -1 }, { ...marker, version: 2 }]) assert.equal(relationMarker(value), null)
  const snapshot = relationSnapshot([intent], [write]); assert.ok(snapshot); assert.equal(relationMarkerResolved(marker, snapshot), true)
  assert.equal(relationMarkerResolved(marker, { intents: [], writes: [] }), false)
  assert.equal(relationMarkerResolved({ ...marker, writeTag: 'c'.repeat(64) }, snapshot), false); assert.equal(relationMarkerResolved({ ...marker, afterIntent: 2 }, snapshot), false); assert.equal(relationMarkerResolved({ ...marker, target: 'group' }, snapshot), false)
})
void test('relation exact not-sent preparing outcome can retire only its own local marker without a fabricated WRITE', () => {
  const snapshot = relationSnapshot([{ ...intent, state: 'not_sent', write_id: null }], []); assert.ok(snapshot); assert.equal(relationMarkerResolved(marker, snapshot), true)
  assert.equal(relationMarkerResolved({ ...marker, intentTag: 'c'.repeat(64) }, snapshot), false)
  const uncertain = relationSnapshot([intent], [{ ...write, state: 'uncertain', base_time: null, update_time: null }]); assert.ok(uncertain); assert.equal(relationMarkerResolved(marker, uncertain), false)
})
void test('relation UI nonce tags match original length-prefixed hash domains not IM string concatenation', async () => {
  const nonce = 'a'.repeat(32); const hash = (label: string) => { const h = createHash('sha256'); for (const text of [label, nonce]) { const data = Buffer.from(text); const prefix = Buffer.alloc(8); prefix.writeBigUInt64BE(BigInt(data.length)); h.update(prefix); h.update(data) } return h.digest('hex') }
  const tags = await relationNonceTags(nonce); assert.equal(tags.intentTag, hash('original-relation-intent-ui-nonce-v1')); assert.equal(tags.writeTag, hash('original-relation-ui-nonce-v1')); assert.notEqual(tags.intentTag, tags.writeTag); assert.doesNotMatch(JSON.stringify(tags), new RegExp(nonce))
  await assert.rejects(relationNonceTags('A'.repeat(32)))
})
