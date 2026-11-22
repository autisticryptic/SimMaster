import test from 'node:test'
import assert from 'node:assert/strict'
import { userEntryView, userEntryLabel, USER_ENTRY_ROUTING_SCOPE } from '../src/policies/userEntry.ts'
void test('Client UserEntry routing description preserves current SDK and refuses new TLS or Unknown WRITE authority', () => {
  assert.match(USER_ENTRY_ROUTING_SCOPE, /当前 SDK 原会话批次/)
  assert.match(USER_ENTRY_ROUTING_SCOPE, /不提供新路由、TLS 授信/)
  assert.match(USER_ENTRY_ROUTING_SCOPE, /未知写入重试权限/)
})
void test('Client batch metadata or a parsed reference cannot become current query observations or WRITE receipts', () => {
  for (const data of [{ reference: 'private-factory', accepted: true }, { kind: 'phone', report: 'ready', observed_at: '2026-01-01T00:00:00Z' }, Object.create({ kind: 'phone', report: 'reported', observed_at: '2026-01-01T00:00:00Z' })]) assert.equal(userEntryView(data), null)
  const data = userEntryView({ kind: 'phone', report: 'reported', observed_at: '2026-01-01T00:00:00Z', factory: '<img src=x onerror=alert(1)>', uid: 'private-uid', token: 'private-token', nonce: 'private-nonce', grant: true })
  assert.ok(data)
  assert.match(userEntryLabel(data), /不是写入回执或接收就绪/)
  assert.equal(JSON.stringify(data).includes('private-'), false)
  assert.equal(JSON.stringify(data).includes('<img'), false)
})
