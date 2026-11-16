import test from 'node:test'
import assert from 'node:assert/strict'
import { IM_RECEIPT_SCOPE, imReceiptLabel, imReceiptRows } from '../src/policies/imReceipts.ts'
const row = { id: 1, box_kind: 'p2p', target_index: '9223372036854775807', state: 'uncertain', updated_at: '2026-01-01T00:00:00Z' }
test('original received receipt keeps full signed64 range as string but never labels user read media or channel readiness', () => {
  const rows = imReceiptRows([row]); assert.equal(rows.length, 1); assert.equal(rows[0].index, '9223372036854775807')
  assert.match(IM_RECEIPT_SCOPE, /不是用户已读、渠道送达/); assert.match(imReceiptLabel('confirmed'), /接收位置回执/)
})
test('unknown original received write is explicitly not replayable and known no IO stays distinct from rejection', () => {
  assert.match(imReceiptLabel('uncertain'), /尚未核实.*不重放/); assert.match(imReceiptLabel('not_sent'), /未写出/); assert.match(imReceiptLabel('rejected'), /拒绝/)
})
test('received receipt projection rejects inherited and malformed labels and does not reflect private metadata', () => {
  const rows = imReceiptRows([{ ...row, token: 'private-token', scope_epoch: 'private-scope', box_id: 'private-box' }, Object.create(row)])
  assert.equal(rows.length, 1); assert.equal(JSON.stringify(rows).includes('private-'), false)
  for (const state of ['ready', 'media', 'constructor', '', true]) assert.equal(imReceiptRows([{ ...row, state }]).length, 0)
})
test('received receipt IDs indices and kind cannot be rounded truncated signed negative or unknown', () => {
  for (const target_index of ['-1', '01', '+1', '9223372036854775808', '', 7]) assert.equal(imReceiptRows([{ ...row, target_index }]).length, 0)
  for (const id of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) assert.equal(imReceiptRows([{ ...row, id }]).length, 0)
  assert.equal(imReceiptRows([{ ...row, box_kind: 'native-raw' }]).length, 0)
})
test('received receipt input output duplicates and time are bounded without raw diagnostic rendering', () => {
  const rows = imReceiptRows(Array.from({ length: 1000 }, (_, i) => ({ ...row, id: i + 1 }))); assert.equal(rows.length, 16)
  assert.equal(imReceiptRows([row, row]).length, 1); assert.equal(imReceiptRows([{ ...row, updated_at: 'private-date' }])[0].updatedAt, '本地观察时间未知')
})
