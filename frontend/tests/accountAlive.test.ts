import assert from 'node:assert/strict'
import test from 'node:test'
import { accountAliveLabel } from '../src/policies/appLogin.ts'

void test('independent Alive registration is identified as legacy transport and never messaging or call readiness', () => {
  const text = accountAliveLabel('registered')
  assert.match(text, /独立注册已确认/)
  assert.match(text, /stcp，非TLS/)
  assert.match(text, /不代表短信、来电或媒体就绪/)
})
void test('blocked and unapproved Alive states do not imply any connection or authorization', () => {
  assert.match(accountAliveLabel('blocked'), /未批准.*未连接/)
  assert.match(accountAliveLabel('unapproved'), /未获准.*未连接/)
  for (const state of ['registering', 'retrying', 'disabled', 'unsupported', 'stopped']) {
    assert.equal(accountAliveLabel(state).includes('注册已确认'), false)
  }
})
void test('unknown Alive status strings and prototype names cannot echo credentials or URLs', () => {
  for (const value of ['https://private.invalid/?token=secret', 'constructor', '__proto__', 'toString', null, {}, 3]) {
    assert.equal(accountAliveLabel(value), '独立通道状态未知；不代表短信、来电或媒体就绪')
  }
})
