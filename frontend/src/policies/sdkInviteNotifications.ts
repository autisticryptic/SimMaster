// Local original-Invite consumption metadata only. Never a channel/answer/
// media/hosting/device-trust grant and never a raw native ID/token/peer display.
const consumed = '原 SDK Invite 已消费并持久保存；不代表渠道送达、来电已接听或媒体就绪'
const absent = '未观察到原 SDK Invite 的已消费记录；不推断设备授信或接收成功'
const unknown = '邀请消费状态未知；不反射服务原值，也不自动重发或重新登录'
export function sdkInviteLabel(value: unknown): string {
  if (value === 'consumed') return consumed
  if (value === undefined || value === null) return absent
  return unknown
}
export interface SdkInviteRow { historyId: number; label: string }
export function sdkInviteRows(value: unknown): SdkInviteRow[] {
  if (!Array.isArray(value)) return []
  const rows: SdkInviteRow[] = []; const seen = new Set<number>()
  for (const entry of value.slice(0, 128)) {
    if (typeof entry !== 'object' || entry === null || Array.isArray(entry) || !Object.prototype.hasOwnProperty.call(entry, 'history_id')) continue
    const id: unknown = (entry as Record<string, unknown>).history_id
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || seen.has(id)) continue
    const state = Object.prototype.hasOwnProperty.call(entry, 'sdk_invite_notification') ? (entry as Record<string, unknown>).sdk_invite_notification : undefined
    seen.add(id); rows.push({ historyId: id, label: sdkInviteLabel(state) })
    if (rows.length === 16) break
  }
  return rows
}
