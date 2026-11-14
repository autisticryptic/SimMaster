import assert from 'node:assert/strict'
import test from 'node:test'
import { startupFailureLabel } from '../src/policies/appLogin.ts'

const unknown = '来源未知 / 阶段未知：未识别的原生启动错误（仅失败阶段观察，不恢复连接或就绪）'

void test('native startup TLS failure identifies the original bootstrap phase without claiming an exact certificate cause', () => {
  const label = startupFailureLabel({ phase: 'bootstrap', stage: 'router_tls', code: 'jegotrip_tls_handshake_failed' })
  assert.equal(label, 'Bootstrap / Router TLS：TLS握手或证书验证失败（仅失败阶段观察，不恢复连接或就绪）')
  for (const claim of ['名称不匹配', '授信过期', '注册成功', '已恢复', '可以接听']) assert.equal(label.includes(claim), false)
})
void test('native startup account and logical channel stages remain distinct from bootstrap TLS', () => {
  const label = startupFailureLabel({ phase: 'account', stage: 'logical_channel', code: 'jegotrip_logical_channel_timeout' })
  assert.equal(label, '账号连接 / 逻辑通道：逻辑通道超时（仅失败阶段观察，不恢复连接或就绪）')
  assert.match(startupFailureLabel({ phase: 'account', stage: 'router_control', code: 'jegotrip_router_control_rejected' }), /Router控制：Router拒绝连接/)
  assert.match(startupFailureLabel({ phase: 'bootstrap', stage: 'attempt', code: 'jegotrip_startup_timeout' }), /启动流程（具体阶段未确认）：准备超时/)
})
void test('unknown native startup diagnostics never reflect private strings URLs or tokens', () => {
  const label = startupFailureLabel({ phase: 'private-password', stage: 'https://private.invalid', code: 'token=secret' })
  assert.equal(label, unknown)
  for (const value of ['private-password', 'private.invalid', 'secret', 'token=']) assert.equal(label.includes(value), false)
})
void test('native startup diagnostic labels reject malformed and prototype property inputs', () => {
  for (const key of ['constructor', '__proto__', 'toString', 'hasOwnProperty']) {
    assert.equal(startupFailureLabel({ phase: key, stage: key, code: key }), unknown)
  }
  for (const value of [null, undefined, [], 'private-token', 1, {}, { phase: [], stage: {}, code: false }]) {
    assert.equal(startupFailureLabel(value), unknown)
  }
})
void test('native startup diagnostic rendering does not mutate the observation or attach ready and retry fields', () => {
  const value = Object.freeze({ phase: 'bootstrap', stage: 'preparation', code: 'jegotrip_startup_configuration_invalid' })
  const before = JSON.stringify(value)
  assert.match(startupFailureLabel(value), /准备参数：准备参数无效/)
  assert.equal(JSON.stringify(value), before)
  assert.deepEqual(Object.keys(value).sort(), ['code', 'phase', 'stage'])
})
