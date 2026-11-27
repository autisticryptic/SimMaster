import test from 'node:test'
import assert from 'node:assert/strict'
import { captureGroupCreateBody, groupCreateBody, groupCreateOutcome } from '../src/policies/groupCreate.ts'
import { submitGroupCreateOnce, type CreatePorts } from '../src/policies/groupCreateFlow.ts'
import { relationNonceTags, relationSnapshot } from '../src/policies/relationWrite.ts'
import { loadRelationMarker, relationStorageKey, type RelationMarkerStore } from '../src/policies/relationWriteStorage.ts'
const nonce = 'e'.repeat(32); const line = 'one-line'; const owner = 'explicit-owner'
const member = { label: owner, relation_type: 257, first: 'provided', second: '', properties: [{ key: [1], value: [7] }] }
const ordinary = { label: '0_1', relation_type: 258, first: '', second: '' }
function make(members: unknown = [member, ordinary], props: unknown = [], read = true) { return groupCreateBody(1, nonce, 'private-name', 1, owner, JSON.stringify(members), JSON.stringify(props), true, true, read) }
function body(read = true) { const data = make(undefined, undefined, read); assert.ok(data); return data }
function storage(): RelationMarkerStore { const cells = new Map<string, string>(); return { getItem: (k) => cells.get(k) ?? null, setItem: (k, v) => { cells.set(k, v) }, removeItem: (k) => { cells.delete(k) } } }
const empty = { intents: [], writes: [] }; const unseen = { state: 'unverified', accepted: null, properties: null }
async function rows() { const tags = await relationNonceTags(nonce); const meta = { management: { operation: 'create', properties: 2 }, target_kind: 'group', additions: 0, updates: 0, removals: 0, updated_at: '2026-01-03T00:00:00Z' }; return { intents: [{ ...meta, id: 5, nonce_tag: tags.intentTag, state: 'bound', write_id: 8 }], writes: [{ ...meta, id: 8, nonce_tag: tags.writeTag, state: 'confirmed', base_time: '-9223372036854775808', update_time: '9223372036854775807' }] } }
void test('create needs an explicit current-owner candidate with both strings and no implied member or URI conversion', () => {
  const value = body(); assert.deepEqual(value.members[0], { uid: owner, relation_type: 257, first: 'provided', second: '', properties: member.properties }); assert.ok(!Object.hasOwn(value, 'group_id')); assert.equal(value.properties_read_consent, true)
  for (const members of [[], [ordinary], [member, member], [{ label: owner, relation_type: 257 }], [{ ...member, second: undefined }], [member, { ...ordinary, label: '[phone:+12345@foreign.invalid]' }], [{ ...member, relation_type: 20 }], [{ ...member, relation_type: 258 }], [{ ...member, properties: [{ key: [1], value: [0] }] }]]) assert.equal(make(members), null)
})
void test('create rejects Name and Type alias overrides including duplicates after real key conversion', () => {
  for (const key of ['Name', 'Type', 'Group.Org.Name', 'Group.Org.Type']) assert.equal(make([member], [{ key, value: key === 'Type' ? 1 : 'text' }]), null)
  assert.equal(make([member], [{ key: 'UpdateMethod', value: 1 }, { key: 'Group.Org.Notify', value: 'Refresh' }]), null); assert.equal(make([member], [{ key: 'Permission', value: 4 }]), null); assert.deepEqual(make([member], [{ key: 'Permission', value: 2 }])?.properties, [{ key: 'Permission', value: 2 }])
})
void test('create body is copied before awaits and cannot carry a group result or a hidden permission', () => {
  const original = body(false); const copy = captureGroupCreateBody(original); assert.ok(copy); original.members[0].properties[0].value[0] = 9; assert.equal(copy.members[0].properties[0].value[0], 7); assert.equal(copy.properties_read_consent, false)
  for (const injected of [{ ...original, write_consent: false }, { ...original, consent: false }, { ...original, group_id: '1-2' }, { ...original, nonce: nonce + '\n' }, { ...original, properties_read_consent: 1 }, { ...original, members: [{ ...original.members[0], token: 'x' }] }]) assert.equal(captureGroupCreateBody(injected), null)
})
void test('create write times and read verdict are distinct and private reply fields are never projected', async () => {
  const r = await rows(); const write = r.writes[0]; assert.ok(relationSnapshot(r.intents, r.writes)); assert.equal(groupCreateOutcome({ write, properties_read: unseen })?.write.state, 'confirmed'); assert.equal(groupCreateOutcome({ write, properties_read: { state: 'rejected', accepted: false, properties: 0 } })?.read.accepted, false)
  for (const invalid of [{ write: { ...write, state: 'rejected' }, properties_read: { state: 'observed', accepted: true, properties: 1 } }, { write, properties_read: { ...unseen, group_id: '1-2' } }, { write, properties_read: { state: 'observed', accepted: false, properties: 1 } }, { write: { ...write, base_time: null }, properties_read: unseen }, { write, properties_read: unseen, receipt: true }]) assert.equal(groupCreateOutcome(invalid), null)
})
void test('create terminal history independently retires the shared marker when the property read failed', async () => {
  const r = await rows(); const s = storage(); let reads = 0; let posts = 0
  const p: CreatePorts = { current: () => true, storage: s, read: () => Promise.resolve(++reads === 1 ? empty : r), post: (data) => { posts++; assert.deepEqual(data, body()); const raw = s.getItem(relationStorageKey(line)); assert.ok(raw); for (const secret of [owner, 'private-name', nonce, 'provided']) assert.ok(!raw.includes(secret)); assert.deepEqual(loadRelationMarker(s, line)?.management, { operation: 'create', properties: 2 }); return Promise.resolve({ write: r.writes[0], properties_read: unseen }) } }
  const out = await submitGroupCreateOnce(p, line, body()); assert.equal(out.status, 'observed_terminal'); assert.equal(out.create?.write.id, 8); assert.equal(out.create?.read.state, 'unverified'); assert.equal(loadRelationMarker(s, line), null); assert.equal(posts, 1)
})
void test('create POST result or empty history cannot retire an ambiguous operation or allow another nonce', async () => {
  const r = await rows(); const s = storage(); let posts = 0; const p: CreatePorts = { current: () => true, storage: s, read: () => Promise.resolve(empty), post: () => { posts++; return Promise.resolve({ write: r.writes[0], properties_read: { state: 'observed', accepted: true, properties: 1 } }) } }
  const out = await submitGroupCreateOnce(p, line, body()); assert.equal(out.status, 'unresolved'); assert.equal(out.create, undefined); assert.ok(loadRelationMarker(s, line)); assert.equal((await submitGroupCreateOnce(p, line, { ...body(), nonce: 'f'.repeat(32) })).status, 'blocked'); assert.equal(posts, 1)
})
void test('create refuses stale POST read projection even when paired history proves the right write', async () => {
  const r = await rows(); const w = r.writes[0]; const observed = { state: 'observed', accepted: true, properties: 1 }
  for (const [readConsent, write] of [[true, { ...w, id: 9 }], [true, { ...w, nonce_tag: 'c'.repeat(64) }], [true, { ...w, base_time: '1' }], [true, { ...w, management: { operation: 'create', properties: 3 } }], [false, w]] as const) {
    let reads = 0; const s = storage(); const out = await submitGroupCreateOnce({ current: () => true, storage: s, read: () => Promise.resolve(++reads === 1 ? empty : r), post: () => Promise.resolve({ write, properties_read: observed }) }, line, body(readConsent)); assert.equal(out.status, 'observed_terminal'); assert.equal(out.create, undefined); assert.equal(loadRelationMarker(s, line), null)
  }
})
void test('create blocks pending membership or disband and stale generation cannot remove the marker', async () => {
  const r = await rows(); let posts = 0; const w = { ...r.writes[0], state: 'uncertain', base_time: null, update_time: null }; const s = storage(); let active = true
  for (const past of [{ ...w, management: undefined, additions: 1 }, { ...w, management: { operation: 'disband', properties: 0 } }]) { const out = await submitGroupCreateOnce({ current: () => true, storage: s, read: () => Promise.resolve({ intents: [], writes: [past] }), post: () => { posts++; return Promise.resolve(null) } }, line, body()); assert.equal(out.status, 'blocked') }; assert.equal(posts, 0)
  const out = await submitGroupCreateOnce({ current: () => active, storage: s, read: () => Promise.resolve(empty), post: () => { active = false; return Promise.reject(new Error('network changed')) } }, line, body()); assert.equal(out.status, 'stale'); assert.ok(loadRelationMarker(s, line))
})
