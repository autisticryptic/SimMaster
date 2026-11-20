import test from 'node:test'
import assert from 'node:assert/strict'
import { directoryBody, directoryGroupId, directorySnapshot, DIRECTORY_READ_SCOPE, type DirectoryInput } from '../src/policies/nativeDirectory.ts'
const self: DirectoryInput = { operation: 'relations', groupId: '', since: '-1' }
const group: DirectoryInput = { operation: 'properties', groupId: '1-2', since: '-1' }
const result = { operation: 'relations', target_kind: 'current_user', accepted: true, reported: false, original_time: '-9223372036854775808', first_relations: 1, second_relations: 2, names: 3, properties: 0 }
void test('native directory body preserves explicit self versus opaque group and signed64 since with no caller SDK identity', () => {
  assert.deepEqual(directoryBody(1, self), { expected_revision: 1, operation: 'relations', since: '-1', consent: true })
  assert.deepEqual(directoryBody(3, group), { expected_revision: 3, operation: 'properties', group_id: '1-2', since: '-1', consent: true })
  assert.equal(directoryBody(1, { ...self, since: '9223372036854775807' })?.since, '9223372036854775807')
  assert.match(DIRECTORY_READ_SCOPE, /不创建关系、群组、短信或通知/); assert.match(DIRECTORY_READ_SCOPE, /不解决未知 WRITE/)
})
void test('native directory group and body validators refuse URI credentials routes inherited and extra fields', () => {
  for (const value of ['', '0-2', '1-0', '01-2', '1-02', '1_2', 'Org/1-2', 'Group:tcp -h host -p 1;', '1-2\0', '1'.repeat(509), null]) assert.equal(directoryGroupId(value), false)
  for (const extra of [{ uid: 'private' }, { token: 'private' }, { authority: 'private' }, { endpoint: 'private' }, { grant: true }]) assert.equal(directoryBody(1, { ...self, ...extra }), null)
  assert.equal(directoryBody(1, Object.create(self) as unknown), null)
  for (const input of [null, [], {}, { ...self, operation: 'send' }, { ...self, groupId: null }, { ...group, groupId: '' }]) assert.equal(directoryBody(1, input), null)
})
void test('native directory revision and operation-specific since reject ambiguous or unsafe input before HTTP', () => {
  for (const revision of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) assert.equal(directoryBody(revision, self), null)
  for (const since of ['01', '+1', '-0', '-2', '9223372036854775808', '-9223372036854775809', '', 1, null]) assert.equal(directoryBody(1, { ...self, since }), null)
  for (const since of ['0', '1', '9223372036854775807']) assert.equal(directoryBody(1, { ...group, since }), null)
})
void test('native directory snapshot is exact requested operation target opaque bool and complete signed64 data', () => {
  const value = directorySnapshot(result, self); assert.equal(value?.time, '-9223372036854775808'); assert.equal(value?.reported, false); assert.equal(value?.accepted, true)
  assert.equal(directorySnapshot({ ...result, accepted: false }, self)?.accepted, false)
  assert.equal(directorySnapshot({ ...result, target_kind: 'group' }, self), null); assert.equal(directorySnapshot(result, group), null)
  const props = directorySnapshot({ operation: 'properties', target_kind: 'group', accepted: true, first_relations: 0, second_relations: 0, names: 0, properties: 128 }, group)
  assert.equal(props?.properties, 128); assert.equal(props?.time, undefined); assert.equal(props?.reported, undefined)
})
void test('native directory snapshot rejects bad counts false metadata and inherited or mismatched variants', () => {
  for (const key of ['first_relations', 'second_relations', 'names', 'properties']) for (const value of [-1, 129, 1.5, '1', null]) assert.equal(directorySnapshot({ ...result, [key]: value }, self), null)
  for (const extra of [{ accepted: 1 }, { reported: 'false' }, { reported: undefined }, { original_time: '+1' }, { original_time: '9223372036854775808' }, { properties: 1 }]) assert.equal(directorySnapshot({ ...result, ...extra }, self), null)
  assert.equal(directorySnapshot(Object.create(result) as unknown, self), null)
  // Matching untrusted strings still must be one of the two original READ variants.
  assert.equal(directorySnapshot({ ...result, operation: 'send' }, { ...self, operation: 'send' } as unknown as DirectoryInput), null)
  const props = { operation: 'properties', target_kind: 'group', accepted: true, first_relations: 0, second_relations: 0, names: 0, properties: 1 }
  for (const extra of [{ reported: false }, { original_time: '0' }, { first_relations: 1 }, { names: 1 }]) assert.equal(directorySnapshot({ ...props, ...extra }, group), null)
})
void test('native directory projection never copies identity properties raw relationships token route or authorization', () => {
  const data = { ...result, token: 'private-token', uid: 'private-uid', raw: 'private-wire', relationship_values: 'private-data', endpoint: 'private-route', native_ready: true, write_grant: true }
  assert.doesNotMatch(JSON.stringify(directorySnapshot(data, self)), /private-|native_ready|write_grant|relationship_values/)
})
