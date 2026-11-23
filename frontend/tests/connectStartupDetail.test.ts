import test from 'node:test'
import assert from 'node:assert/strict'
import { startupFailureLabel, startupConnectDetailLabel, startupTlsDetailLabel } from '../src/policies/appLogin.ts'
const failure = { phase: 'bootstrap', stage: 'router_connect', code: 'jegotrip_connector_timeout' }
void test('connect timeout detail is finite DNS TCP data without changing legacy failure or granting retry', () => {
  assert.match(startupFailureLabel(failure), /连接超时/)
  assert.match(startupConnectDetailLabel(failure, { phase: 'bootstrap', step: 'dns' }), /DNS准备或解析时限到达/)
  assert.match(startupConnectDetailLabel(failure, { phase: 'bootstrap', step: 'tcp' }), /TCP连接时限到达/)
  assert.match(startupConnectDetailLabel(failure, { phase: 'bootstrap', step: 'dns' }), /不授信、不延长时限、不重试或恢复就绪/)
  assert.equal(startupTlsDetailLabel(failure, { phase: 'bootstrap', reason: 'unknown_issuer' }), '')
})
void test('connect timeout detail requires own phase stage code and exact same-phase observation', () => {
  for (const detail of [null, {}, Object.create({ phase: 'bootstrap', step: 'dns' }), { phase: 'account', step: 'tcp' }, { phase: 'private', step: 'dns' }]) assert.equal(startupConnectDetailLabel(failure, detail), '')
  assert.equal(startupConnectDetailLabel(Object.create(failure), { phase: 'bootstrap', step: 'dns' }), '')
  assert.equal(startupConnectDetailLabel({ ...failure, stage: 'router_tls' }, { phase: 'bootstrap', step: 'dns' }), '')
  assert.equal(startupConnectDetailLabel({ ...failure, code: 'jegotrip_connector_io_failed' }, { phase: 'bootstrap', step: 'tcp' }), '')
})
void test('connect timeout detail never coerces unknown steps or echoes raw peer credentials', () => {
  for (const step of ['private-token', 'https://private.invalid', '<img src=x onerror=alert(1)>', '__proto__', { toString: () => { throw new Error('untrusted coercion') } }]) assert.equal(startupConnectDetailLabel(failure, { phase: 'bootstrap', step }), '')
  const label = startupConnectDetailLabel(failure, { phase: 'bootstrap', step: 'tcp', host: 'private-host', token: 'private-token' })
  assert.match(label, /TCP连接/); assert.equal(label.includes('private-'), false)
  assert.equal(startupConnectDetailLabel({ ...failure, phase: { toString: () => { throw new Error('untrusted coercion') } } }, { phase: 'bootstrap', step: 'dns' }), '')
})
void test('connect and TLS details remain independent account or bootstrap bounded observations', () => {
  const account = { ...failure, phase: 'account' }
  assert.match(startupConnectDetailLabel(account, { phase: 'account', step: 'tcp' }), /仅本次超时步骤/)
  const tls = { phase: 'account', stage: 'router_tls', code: 'jegotrip_tls_handshake_failed' }
  assert.equal(startupConnectDetailLabel(tls, { phase: 'account', step: 'tcp' }), '')
  assert.match(startupTlsDetailLabel(tls, { phase: 'account', reason: 'bad_signature' }), /签名校验失败/)
  for (const step of ['dns', 'tcp']) assert.ok(startupConnectDetailLabel(failure, { phase: 'bootstrap', step }).length < 100)
})
