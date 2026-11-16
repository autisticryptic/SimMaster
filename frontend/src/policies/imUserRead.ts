// User mark-read outcome is not received-position acknowledgment or media.
export type UserReadState = 'uncertain' | 'confirmed' | 'rejected' | 'not_sent'
export interface UserReadRow { id: number; messageId: number; index: string; state: UserReadState; updatedAt: string }
export function userReadRows(value: unknown): UserReadRow[] {
  if (!Array.isArray(value)) return []
  const rows: UserReadRow[] = []; const seen = new Set<number>()
  for (const entry of value.slice(0, 128)) {
    if (typeof entry !== 'object' || entry === null || Array.isArray(entry)) continue
    const own = (name: string): unknown => Object.prototype.hasOwnProperty.call(entry, name) ? (entry as Record<string, unknown>)[name] : undefined
    const id = own('id'); const messageId = own('message_id'); const index = own('target_index'); const state = own('state'); const time = own('updated_at')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || seen.has(id) || typeof messageId !== 'number' || !Number.isSafeInteger(messageId) || messageId <= 0) continue
    if (typeof index !== 'string' || !/^(?:0|[1-9][0-9]{0,18})$/.test(index) || BigInt(index) > 9223372036854775807n) continue
    if (state !== 'uncertain' && state !== 'confirmed' && state !== 'rejected' && state !== 'not_sent') continue
    const updatedAt = typeof time === 'string' && time.length <= 64 && !time.includes('\0') && Number.isFinite(Date.parse(time)) ? new Date(time).toLocaleString() : '本地观察时间未知'
    seen.add(id); rows.push({ id, messageId, index, state, updatedAt }); if (rows.length === 16) break
  }
  return rows
}
export function userReadLabel(state: UserReadState): string {
  switch (state) {
    case 'uncertain': return '用户已读写入未核实；仅查询，不重放'
    case 'confirmed': return '原用户已读操作已确认'
    case 'rejected': return '原服务端拒绝用户已读操作'
    case 'not_sent': return '本次用户已读操作未写出'
  }
}
export const USER_READ_SCOPE = '独立用户已读操作，不是接收位置回执、渠道送达或通话/媒体就绪；需当前原会话和明确用户确认。'
