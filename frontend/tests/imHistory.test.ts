import test from 'node:test'
import assert from 'node:assert/strict'
import { IM_HISTORY_SCOPE, imHistoryRows } from '../src/policies/imHistory.ts'
const row = { id: 1, box_kind: 'p2p', message_index: '9223372036854775807', text: 'original IM', attachment_count: 1, received_at: '2026-01-01T00:00:00Z' }
void test('IM history keeps complete signed64 indices as strings and identifies non-SMS consumption', () => {
  assert.equal(imHistoryRows([row])[0]?.index, '9223372036854775807')
  assert.equal(imHistoryRows([{ ...row, message_index: '-9223372036854775808' }])[0]?.index, '-9223372036854775808')
  assert.match(IM_HISTORY_SCOPE, /不是运营商短信/); assert.match(IM_HISTORY_SCOPE, /不等于服务端已读、渠道送达、通话或媒体就绪/)
})
void test('IM projection does not reflect native identity raw body attachments or inherited metadata', () => {
  const result = imHistoryRows([{ ...row, body: 'private-raw', sender: 'private-sender', identity_key: 'private-key', fingerprint: 'private-fingerprint' }])
  assert.doesNotMatch(JSON.stringify(result), /private-raw|private-sender|private-key|private-fingerprint/)
  assert.deepEqual(imHistoryRows([Object.create(row) as unknown]), [])
  assert.equal(imHistoryRows([{ ...row, received_at: 'private-time-value' }])[0]?.receivedAt, '本地接收时间未知')
})
void test('IM fields reject ambiguous indices unsafe IDs unknown kinds and malformed text or attachment counts', () => {
  for (const message_index of ['01', '+1', '-0', '9223372036854775808', '-9223372036854775809', 1, null]) assert.deepEqual(imHistoryRows([{ ...row, message_index }]), [])
  for (const id of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, '1']) assert.deepEqual(imHistoryRows([{ ...row, id }]), [])
  for (const attachment_count of [-1, 129, '1', null]) assert.deepEqual(imHistoryRows([{ ...row, attachment_count }]), [])
  assert.deepEqual(imHistoryRows([{ ...row, box_kind: 'constructor' }, { ...row, text: 'a\0b' }, { ...row, text: {} }]), [])
})
void test('IM history input output and UTF8 content are bounded and duplicate local IDs do not render twice', () => {
  assert.equal(imHistoryRows([row, row]).length, 1)
  assert.equal(imHistoryRows(Array.from({ length: 1000 }, (_, id) => ({ ...row, id: id + 1 }))).length, 16)
  assert.deepEqual(imHistoryRows([{ ...row, text: '字'.repeat(22000) }]), [])
  assert.deepEqual(imHistoryRows([{ ...row, text: 'a'.repeat(65537) }]), [])
  for (const input of [null, {}, 'private', 7]) assert.deepEqual(imHistoryRows(input), [])
})
