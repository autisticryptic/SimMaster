import test from 'node:test'
import assert from 'node:assert/strict'
import { sdkInviteLabel, sdkInviteRows } from '../src/policies/sdkInviteNotifications.ts'
void test('original SDK Invite consumption label is not channel delivery answer media or trust readiness', () => {
  assert.match(sdkInviteLabel('consumed'), /已消费并持久保存/)
  assert.match(sdkInviteLabel('consumed'), /不代表渠道送达、来电已接听或媒体就绪/)
  assert.match(sdkInviteLabel(undefined), /不推断设备授信或接收成功/)
  for (const value of ['private-token', 'registered', 'hosted', 'constructor', true, {}, null]) {
    assert.doesNotMatch(sdkInviteLabel(value), /private-token|registered|hosted|constructor/)
  }
})
void test('Invite metadata ignores raw peer SDP native ID unsafe history numbers and inherited state', () => {
  const inherited = Object.assign(Object.create({ sdk_invite_notification: 'consumed' }), { history_id: 7 }) as unknown
  const rows = sdkInviteRows([{ history_id: 1, sdk_invite_notification: 'consumed', peer: 'private-peer', sdp: 'private-sdp', notification_id: 'private-native-id' },
    { history_id: 1, sdk_invite_notification: 'private-token' }, { history_id: '2', sdk_invite_notification: 'consumed' },
    { history_id: Number.MAX_SAFE_INTEGER + 1, sdk_invite_notification: 'consumed' }, { history_id: 0 }, inherited,
    Object.create({ history_id: 9, sdk_invite_notification: 'consumed' }) as unknown])
  assert.equal(rows.length, 2); assert.equal(rows[0]?.historyId, 1); assert.equal(rows[1]?.historyId, 7)
  assert.match(rows[1]?.label ?? '', /未观察到/)
  assert.doesNotMatch(JSON.stringify(rows), /private-peer|private-sdp|private-native-id|private-token/)
})
void test('Invite read-only projection keeps finite input output and unknown state labels', () => {
  for (const value of [null, undefined, 'consumed', {}, 1]) assert.deepEqual(sdkInviteRows(value), [])
  const rows = sdkInviteRows(Array.from({ length: 1000 }, (_, id) => ({ history_id: id + 1, sdk_invite_notification: id === 0 ? 'private-token' : undefined })))
  assert.equal(rows.length, 16); assert.match(rows[0]?.label ?? '', /状态未知/); assert.doesNotMatch(rows[0]?.label ?? '', /private-token/)
})
