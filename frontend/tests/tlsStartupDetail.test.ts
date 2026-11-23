import test from 'node:test'
import assert from 'node:assert/strict'
import { startupFailureLabel, startupTlsDetailLabel } from '../src/policies/appLogin.ts'
const failure = { phase: 'bootstrap', stage: 'router_tls', code: 'jegotrip_tls_handshake_failed' }
void test('finite TLS detail preserves legacy startup label and grants no readiness or trust', () => {
  assert.match(startupFailureLabel(failure), /TLS握手或证书验证失败/)
  assert.match(startupTlsDetailLabel(failure, { phase: 'bootstrap', reason: 'name_mismatch' }), /验证名称不匹配/)
  assert.match(startupTlsDetailLabel(failure, { phase: 'bootstrap', reason: 'unknown_issuer' }), /发行方未知/)
  assert.match(startupTlsDetailLabel(failure, { phase: 'bootstrap', reason: 'exact_name_missing' }), /精确SAN/)
  assert.match(startupTlsDetailLabel(failure, { phase: 'bootstrap', reason: 'no_protocol_version' }), /不授信、不恢复连接或就绪/)
})
void test('finite TLS detail rejects missing inherited cross-phase or non-TLS source evidence', () => {
  for (const data of [null, {}, Object.create({ phase: 'bootstrap', reason: 'name_mismatch' }), { phase: 'account', reason: 'name_mismatch' }]) assert.equal(startupTlsDetailLabel(failure, data), '')
  assert.equal(startupTlsDetailLabel({ ...failure, stage: 'router_connect' }, { phase: 'bootstrap', reason: 'name_mismatch' }), '')
  assert.equal(startupTlsDetailLabel({ ...failure, code: 'jegotrip_tls_deadline_expired' }, { phase: 'bootstrap', reason: 'name_mismatch' }), '')
  assert.equal(startupTlsDetailLabel(Object.create(failure), { phase: 'bootstrap', reason: 'name_mismatch' }), '')
})
void test('finite TLS detail never displays private reason text or unknown property names', () => {
  for (const reason of ['private-token', 'https://private-uid.invalid', '<img src=x onerror=alert(1)>', '__proto__', { toString: () => { throw new Error('untrusted coercion') } }]) assert.equal(startupTlsDetailLabel(failure, { phase: 'bootstrap', reason }), '')
  assert.equal(startupTlsDetailLabel({ ...failure, phase: { toString: () => { throw new Error('untrusted coercion') } } }, { phase: 'bootstrap', reason: 'name_mismatch' }), '')
  const label = startupTlsDetailLabel(failure, { phase: 'bootstrap', reason: 'bad_signature', token: 'private-token', certificate: '<img>', uid: 'private-uid' })
  assert.match(label, /签名校验失败/); assert.equal(label.includes('private-'), false); assert.equal(label.includes('<img>'), false)
})
void test('finite TLS detail current categories stay bounded with no automatic retry or downgrade', () => {
  for (const reason of ['malformed_record', 'read_failed', 'no_peer_chain', 'name_mismatch', 'exact_name_missing', 'unknown_issuer', 'expired', 'not_yet_valid', 'revoked', 'bad_encoding', 'bad_signature', 'unsupported_signature', 'invalid_purpose', 'invalid_chain', 'no_protocol_version', 'no_cipher_suite', 'peer_alert', 'peer_protocol', 'decryption_failed', 'protocol_other']) {
    const label = startupTlsDetailLabel(failure, { phase: 'bootstrap', reason }); assert.match(label, /仅本次失败分类/); assert.ok(label.length < 100)
  }
})
