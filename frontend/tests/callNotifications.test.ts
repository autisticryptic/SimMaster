import assert from 'node:assert/strict'
import test from 'node:test'
import { callNotificationLabel } from '../src/policies/callNotifications.ts'
void test('call notification channel receipt never grants answered or media-ready state', () => {
  for (const state of ['pending', 'sending', 'delivered', 'failed', 'cancelled', 'uncertain', 'expired']) assert.match(callNotificationLabel(state), /不代表来电已接听或媒体就绪/)
})
void test('uncertain original call notification is not represented as a retryable failure', () => {
  assert.match(callNotificationLabel('uncertain'), /不自动重发/)
  assert.match(callNotificationLabel('cancelled'), /原来电或目的地已变化/)
})
void test('unknown and prototype properties cannot reflect private channel values', () => {
  for (const value of ['constructor', '__proto__', 'private-token', null, {}]) {
    assert.match(callNotificationLabel(value), /状态未知/)
    assert.equal(callNotificationLabel(value).includes('private-token'), false)
  }
})
void test('expired call notification is separate from the original SDK account lifetime', () => {
  assert.match(callNotificationLabel('expired'), /发送窗口已到期/)
  assert.equal(callNotificationLabel('expired').includes('SDK已退出'), false)
})
