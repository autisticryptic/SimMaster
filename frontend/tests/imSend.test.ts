import test from 'node:test'
import assert from 'node:assert/strict'
import { imSendRows, imSendLabel, imSendNonceTag, imSendFailureEnvelope, imSendNotAdmitted, ImSendNotAdmittedError, imSendMetadataBody, IM_SEND_SCOPE } from '../src/policies/imSend.ts'
void test('original IM send metadata keeps full signed64 message and time without SMS or channel grant', () => { const rows = imSendRows([{ id: 1, nonce_tag: 'a'.repeat(64), kind: 'text', state: 'confirmed', message_index: '9223372036854775807', original_time: '-9223372036854775808' }]); assert.ok(rows); assert.equal(rows[0].index, '9223372036854775807'); assert.equal(rows[0].time, '-9223372036854775808'); assert.match(imSendLabel(rows[0].state), /不是短信/); assert.match(IM_SEND_SCOPE, /SDK\/媒体授信/) })
void test('original IM send rejects inherited unsafe malformed and overflow metadata', () => { const row = { id: 1, nonce_tag: 'a'.repeat(64), kind: 'text', state: 'uncertain', message_index: null, original_time: null }; assert.equal(imSendRows([Object.create(row)]), null); for (const bad of [{ id: Number.MAX_SAFE_INTEGER + 1 }, { message_index: '01' }, { message_index: '9223372036854775808' }, { original_time: '-9223372036854775809' }, { nonce_tag: 'bad' }, { state: 'ready' }]) assert.equal(imSendRows([{ ...row, ...bad }]), null) })
void test('original IM send whole history is bounded and duplicate IDs fail baseline closed', () => { const rows = Array.from({ length: 128 }, (_, i) => ({ id: i + 1, nonce_tag: (i + 1).toString(16).padStart(64, '0'), kind: 'info', state: i === 0 ? 'uncertain' : 'rejected', message_index: null, original_time: null })); assert.equal(imSendRows(rows)?.length, 128); assert.equal(imSendRows([...rows, rows[0]]), null); assert.equal(imSendRows([rows[0], rows[0]]), null); assert.deepEqual(imSendRows([]), []) })
void test('original IM send projection never reflects sender URI recipient body token or private nonce', () => { const rows = imSendRows([{ id: 1, nonce_tag: 'a'.repeat(64), kind: 'text', state: 'rejected', message_index: null, original_time: null, sender: 'private-uri', recipient: 'private-recipient', body: 'private-body', token: 'private-token', nonce: 'private-nonce' }]); assert.ok(rows); assert.equal(JSON.stringify(rows).includes('private-'), false); assert.match(imSendLabel('uncertain'), /不重放/) })
void test('original IM nonce correlation tag is stable and cannot reveal or authorize native data', async () => { const tag = await imSendNonceTag('a'.repeat(32)); assert.match(tag, /^[0-9a-f]{64}$/); assert.equal(tag, await imSendNonceTag('a'.repeat(32))); assert.notEqual(tag, await imSendNonceTag('b'.repeat(32))); assert.equal(tag.includes('a'.repeat(32)), false) })
void test('original sender no-intent observation is own-property nonce-bound and never a generic error whitelist', () => {
  const tag = 'a'.repeat(64); const value = { status: 'error', message: 'app_account_im_send_capacity', im_send_admission: 'not_admitted', nonce_tag: tag }
  assert.equal(imSendFailureEnvelope(value, tag), true)
  assert.equal(imSendFailureEnvelope(Object.create(value), tag), false)
  for (const bad of [{ nonce_tag: 'b'.repeat(64) }, { im_send_admission: 'admitted_or_unknown' }, { im_send_admission: true }, { status: 'ok' }, { message: 'raw private error' }]) assert.equal(imSendFailureEnvelope({ ...value, ...bad }, tag), false)
  assert.equal(imSendNotAdmitted(new Error('app_account_im_send_capacity'), tag), false)
  assert.equal(imSendNotAdmitted({ nonceTag: tag, not_admitted: true }, tag), false)
  assert.equal(imSendNotAdmitted(new ImSendNotAdmittedError(value.message, tag), tag), true)
  assert.equal(imSendNotAdmitted(new ImSendNotAdmittedError(value.message, tag), 'b'.repeat(64)), false)
})
void test('original sender ambiguous timeout and scope have no no-intent promise without exact staged envelope', () => {
  for (const message of ['app_account_im_send_timeout', 'app_account_im_send_scope_invalid', 'app_account_im_send_uncertain']) {
    const value = { status: 'error', message, nonce_tag: 'a'.repeat(64) }
    assert.equal(imSendFailureEnvelope(value, 'a'.repeat(64)), false)
    assert.equal(imSendFailureEnvelope({ ...value, im_send_admission: 'admitted_or_unknown' }, 'a'.repeat(64)), false)
  }
})
void test('original sender unknown or known not-sent cannot carry fabricated result IDs and duplicate tags fail whole baseline', () => {
  const row = { id: 1, nonce_tag: 'a'.repeat(64), kind: 'text', state: 'uncertain', message_index: null, original_time: null }
  for (const state of ['uncertain', 'not_sent']) assert.equal(imSendRows([{ ...row, state, message_index: '1' }]), null)
  assert.equal(imSendRows([{ ...row, state: 'confirmed' }]), null)
  assert.equal(imSendRows([row, { ...row, id: 2 }]), null)
})
void test('original sender nonce hashing rejects noncanonical input before any HTTP request', async () => {
  for (const nonce of ['', 'a'.repeat(31), 'A'.repeat(32), 'a'.repeat(33), 'x'.repeat(32)]) await assert.rejects(imSendNonceTag(nonce), /Original UI nonce invalid/)
})
void test('original notification metadata preserves exact present string bytes and digest is only DATA', () => {
  assert.deepEqual(imSendMetadataBody(undefined), {})
  const encoder = new TextEncoder(); const bytes = (text: string) => Array.from(encoder.encode(text))
  assert.deepEqual(imSendMetadataBody({ digest: 'display 摘要', userData: '', onlineMsg: '01', imdnId: 'opaque-id' }), { notification_digest: 'display 摘要', parameters: [{ key: bytes('userData'), value: [] }, { key: bytes('OnlineMsg'), value: bytes('01') }, { key: bytes('imdnId'), value: bytes('opaque-id') }] })
  assert.deepEqual(imSendMetadataBody({ digest: '' }), { notification_digest: '' })
})
void test('original notification metadata rejects inherited fields endpoints grants or boolean OnlineMsg', () => {
  assert.equal(imSendMetadataBody(Object.create({ digest: 'inherited' })), null)
  for (const value of [null, [], { onlineMsg: true }, { userData: 1 }, { uid: 'forged' }, { token: 'private' }, { endpoint: 'https://private.invalid' }, { ready: true }]) assert.equal(imSendMetadataBody(value), null)
})
void test('original notification metadata uses bounded UTF8 bytes and no NUL for all optional fields', () => {
  for (const key of ['digest', 'userData', 'onlineMsg', 'imdnId']) { assert.equal(imSendMetadataBody({ [key]: 'x\0y' }), null); assert.equal(imSendMetadataBody({ [key]: 'x'.repeat(65537) }), null); assert.ok(imSendMetadataBody({ [key]: 'x'.repeat(65536) })); assert.equal(imSendMetadataBody({ [key]: '🥘'.repeat(16385) }), null) }
})
void test('original notification metadata projection cannot forward SDK identity nonce or native reply', () => {
  for (const key of ['account', 'mainAccount', 'nonce', 'reply', 'scope_epoch', 'selector', 'recipient', 'Statistics.AppId']) assert.equal(imSendMetadataBody({ digest: 'display', [key]: 'private' }), null)
  const value = imSendMetadataBody({ userData: 'string not a grant', onlineMsg: 'false' }); assert.ok(value); assert.equal(JSON.stringify(value).includes('ready'), false)
})
