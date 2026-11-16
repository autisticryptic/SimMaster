import test from 'node:test'
import assert from 'node:assert/strict'
import { userReadBaselineReady, userReadKnownUnknown, userReadUnknownResolved, type UserReadSnapshot } from '../src/policies/imUserReadState.ts'
import type { UserReadRow } from '../src/policies/imUserRead.ts'
const row: UserReadRow = { id: 8, messageId: 1, index: '9223372036854775807', state: 'confirmed', updatedAt: 'test-local-observation' }
const snapshot: UserReadSnapshot = { line: 'current-account', generation: 2, context: 'current-revision-hosted-context', rows: [row] }
const unknown = { line: 'current-account', id: 1, index: row.index, after: 8 }
void test('user read requires current-account current-generation baseline instead of an absent or prior outcome', () => {
  assert.equal(userReadBaselineReady(null, snapshot.line, 2), false); assert.equal(userReadBaselineReady(snapshot, 'other-account', 2), false); assert.equal(userReadBaselineReady(snapshot, snapshot.line, 3), false); assert.equal(userReadBaselineReady(snapshot, snapshot.line, 2), true)
})
void test('an older confirmed user-read row cannot resolve a new lost write response', () => {
  assert.equal(userReadUnknownResolved(unknown, snapshot, snapshot.line, 2), false)
})
void test('a matching unresolved row forbids clearing uncertainty even beside a newer terminal observation', () => {
  const latest: UserReadSnapshot = { ...snapshot, rows: [{ ...row, id: 10 }, { ...row, id: 9, state: 'uncertain' }] }
  assert.equal(userReadKnownUnknown(latest, snapshot.line, 2, 1, row.index), true); assert.equal(userReadUnknownResolved(unknown, latest, snapshot.line, 2), false)
})
void test('only a newer matching terminal without a live unknown can resolve the same local user action observation', () => {
  assert.equal(userReadUnknownResolved(unknown, { ...snapshot, rows: [{ ...row, id: 9 }] }, snapshot.line, 2), true)
  assert.equal(userReadUnknownResolved(unknown, { ...snapshot, rows: [{ ...row, id: 9, messageId: 2 }] }, snapshot.line, 2), false)
  assert.equal(userReadUnknownResolved(unknown, { ...snapshot, rows: [{ ...row, id: 9, index: '7' }] }, snapshot.line, 2), false)
})
void test('late account generation and reload state remain observations rather than native write permission', () => {
  const pending: UserReadSnapshot = { ...snapshot, rows: [{ ...row, id: 9, state: 'uncertain' }] }
  assert.equal(userReadKnownUnknown(pending, snapshot.line, 2, 1, row.index), true); assert.equal(userReadKnownUnknown(pending, 'new-account', 3, 1, row.index), false); assert.equal(userReadUnknownResolved(unknown, { ...snapshot, rows: [{ ...row, id: 99 }] }, snapshot.line, 3), false)
})
