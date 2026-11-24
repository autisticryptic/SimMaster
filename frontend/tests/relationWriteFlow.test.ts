import test from 'node:test'
import assert from 'node:assert/strict'
import { relationWriteBody, relationNonceTags, type RelationWriteBody } from '../src/policies/relationWrite.ts'
import { submitRelationOnce, readRelationOutcome, type RelationWritePorts } from '../src/policies/relationWriteFlow.ts'
import { loadRelationMarker, relationStorageKey, storeRelationMarker, clearMatchedRelationMarker, type RelationMarkerStore } from '../src/policies/relationWriteStorage.ts'
class Memory implements RelationMarkerStore {
  values = new Map<string, string>()
  getItem(key: string) { return this.values.get(key) ?? null }
  setItem(key: string, value: string) { this.values.set(key, value) }
  removeItem(key: string) { this.values.delete(key) }
}
function body(): RelationWriteBody { const result = relationWriteBody(1, 'a'.repeat(32), { target: 'buddy', groupId: '', additions: '[{"label":"private-label","relation_type":258}]', updates: '[]', removals: '[]' }, true, true); assert.ok(result); return result }
async function outcomes() {
  const tags = await relationNonceTags('a'.repeat(32)); return { intents: [{ id: 1, nonce_tag: tags.intentTag, state: 'bound', write_id: 1, target_kind: 'buddy', additions: 1, updates: 0, removals: 0, updated_at: '2026-01-01T00:00:00Z' }], writes: [{ id: 1, nonce_tag: tags.writeTag, state: 'confirmed', target_kind: 'buddy', additions: 1, updates: 0, removals: 0, base_time: '0', update_time: '1', updated_at: '2026-01-01T00:00:00Z' }] }
}
void test('relation browser flow durably stores opaque intent before exactly one POST and only paired history clears it', async () => {
  const storage = new Memory(); let posts = 0; let reads = 0; const terminal = await outcomes()
  const ports: RelationWritePorts = { storage, current: () => true, read: async () => { reads += 1; return reads === 1 ? { intents: [], writes: [] } : terminal }, post: async (value) => { posts += 1; assert.ok(loadRelationMarker(storage, 'line')); const persisted = storage.getItem(relationStorageKey('line')); assert.ok(persisted); assert.doesNotMatch(persisted, /private-label|nonce|aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/); assert.deepEqual(value, body()); return { state: 'ready' } } }
  const result = await submitRelationOnce(ports, 'line', body()); assert.equal(result.status, 'observed_terminal'); assert.equal(posts, 1); assert.equal(reads, 2); assert.equal(loadRelationMarker(storage, 'line'), null)
})
void test('relation browser empty history or generic HTTP errors cannot clear unknown or generate a second POST', async () => {
  const storage = new Memory(); let posts = 0; const ports: RelationWritePorts = { storage, current: () => true, read: async () => ({ intents: [], writes: [] }), post: async () => { posts += 1; throw new Error('app_account_relation_request_invalid') } }
  assert.equal((await submitRelationOnce(ports, 'line', body())).status, 'unresolved'); assert.ok(loadRelationMarker(storage, 'line'))
  assert.equal((await readRelationOutcome(ports, 'line')).status, 'unresolved'); assert.equal((await submitRelationOnce(ports, 'line', body())).status, 'blocked'); assert.equal(posts, 1)
})
void test('relation browser navigation during POST keeps marker for reload and neither positive response nor foreign terminal resolves it', async () => {
  const storage = new Memory(); let live = true; let posts = 0; const terminal = await outcomes()
  const ports: RelationWritePorts = { storage, current: () => live, read: async () => ({ intents: [], writes: [] }), post: async () => { posts += 1; live = false; return terminal.writes[0] } }
  assert.equal((await submitRelationOnce(ports, 'line', body())).status, 'stale'); assert.ok(loadRelationMarker(storage, 'line')); live = true
  const foreign: RelationWritePorts = { ...ports, read: async () => ({ intents: [{ ...terminal.intents[0], nonce_tag: 'f'.repeat(64) }], writes: terminal.writes }) }
  assert.equal((await readRelationOutcome(foreign, 'line')).status, 'unresolved'); assert.ok(loadRelationMarker(storage, 'line'))
  const exact: RelationWritePorts = { ...ports, read: async () => terminal }; assert.equal((await readRelationOutcome(exact, 'line')).status, 'observed_terminal'); assert.equal(loadRelationMarker(storage, 'line'), null); assert.equal(posts, 1)
})
void test('relation browser invalid blocked or unavailable storage prevents HTTP POST without fallback', async () => {
  let posts = 0; const storage = new Memory(); storage.setItem(relationStorageKey('line'), '{broken')
  const ports: RelationWritePorts = { storage, current: () => true, read: async () => ({ intents: [], writes: [] }), post: async () => { posts += 1 } }
  await assert.rejects(submitRelationOnce(ports, 'line', body())); assert.equal(posts, 0)
  const unavailable: RelationMarkerStore = { getItem: () => null, setItem: () => { throw new Error('storage blocked') }, removeItem: () => {} }
  await assert.rejects(submitRelationOnce({ ...ports, storage: unavailable }, 'other', body())); assert.equal(posts, 0)
})
void test('relation browser draft is cloned before async preparation and later edits cannot alter immutable body', async () => {
  const storage = new Memory(); const input = body(); let reads = 0; let posts = 0; const terminal = await outcomes()
  const ports: RelationWritePorts = { storage, current: () => true, read: async () => { reads += 1; if (reads === 1) input.additions[0].label = 'late-mutated'; return reads === 1 ? { intents: [], writes: [] } : terminal }, post: async (value) => { posts += 1; assert.equal(value.additions[0].label, 'private-label') } }
  assert.equal((await submitRelationOnce(ports, 'line', input)).status, 'observed_terminal'); assert.equal(posts, 1)
})
void test('relation browser preparing and bound-unknown records reject a new nonce despite empty other history', async () => {
  const storage = new Memory(); let posts = 0; const terminal = await outcomes()
  const ports: RelationWritePorts = { storage, current: () => true, read: async () => ({ intents: [{ ...terminal.intents[0], state: 'preparing', write_id: null }], writes: [] }), post: async () => { posts += 1 } }
  assert.equal((await submitRelationOnce(ports, 'line', body())).status, 'blocked'); assert.equal(posts, 0)
  ports.read = async () => ({ intents: terminal.intents, writes: [{ ...terminal.writes[0], state: 'uncertain', base_time: null, update_time: null }] })
  assert.equal((await submitRelationOnce(ports, 'line', body())).status, 'blocked'); assert.equal(posts, 0)
})
void test('relation browser nonce and baseline duplicate detection never adopts a recorded nonce or an old context', async () => {
  const storage = new Memory(); let posts = 0; const terminal = await outcomes()
  const ports: RelationWritePorts = { storage, current: () => true, read: async () => terminal, post: async () => { posts += 1 } }
  assert.equal((await submitRelationOnce(ports, 'line', body())).status, 'blocked'); assert.equal(posts, 0)
  assert.equal((await submitRelationOnce({ ...ports, current: () => false }, 'line', body())).status, 'stale'); assert.equal(posts, 0)
})
void test('relation browser marker CAS refuses overwrite retirement of a different intent and noncanonical stored fields', async () => {
  const storage = new Memory(); const tags = await relationNonceTags('a'.repeat(32)); const marker = { version: 1 as const, ...tags, revision: 1, afterIntent: 0, afterWrite: 0, target: 'buddy' as const, additions: 1, updates: 0, removals: 0 }
  storeRelationMarker(storage, 'line', marker); assert.throws(() => storeRelationMarker(storage, 'line', marker)); assert.throws(() => clearMatchedRelationMarker(storage, 'line', { ...marker, intentTag: 'f'.repeat(64) })); assert.ok(loadRelationMarker(storage, 'line'))
  clearMatchedRelationMarker(storage, 'line', marker); assert.equal(loadRelationMarker(storage, 'line'), null)
  storage.setItem(relationStorageKey('line'), JSON.stringify({ ...marker, rawBody: 'private' })); assert.throws(() => loadRelationMarker(storage, 'line'))
})
