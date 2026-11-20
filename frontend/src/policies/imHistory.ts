// Original IM history is distinct from SMS, SDK readiness and read-status ACKs.
export interface ImHistoryRow { id: number; kind: 'p2p' | 'org'; index: string; text: string; attachments: number; receivedAt: string; infoType?: string; systemInfo?: boolean }
function index(value: unknown): value is string {
  if (typeof value !== 'string' || !/^(?:0|-[1-9][0-9]{0,18}|[1-9][0-9]{0,18})$/.test(value)) return false
  const number = BigInt(value)
  return number >= -9223372036854775808n && number <= 9223372036854775807n
}
export function imHistoryRows(value: unknown): ImHistoryRow[] {
  if (!Array.isArray(value)) return []
  const rows: ImHistoryRow[] = []; const seen = new Set<number>()
  for (const entry of value.slice(0, 128)) {
    if (typeof entry !== 'object' || entry === null || Array.isArray(entry)) continue
    const own = (name: string): unknown => Object.prototype.hasOwnProperty.call(entry, name) ? (entry as Record<string, unknown>)[name] : undefined
    const id = own('id'); const kind = own('box_kind'); const messageIndex = own('message_index'); const text = own('text'); const attachments = own('attachment_count'); const received = own('received_at')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || seen.has(id) || (kind !== 'p2p' && kind !== 'org') || !index(messageIndex)) continue
    if (typeof text !== 'string' || text.length > 65536 || text.includes('\0') || new TextEncoder().encode(text).length > 65536) continue
    if (typeof attachments !== 'number' || !Number.isInteger(attachments) || attachments < 0 || attachments > 128) continue
    const bodyKind = own('kind'); const infoType = own('info_type'); const systemInfo = own('system_info')
    const info = bodyKind === 'info'
    if (bodyKind !== undefined && !info || info && (typeof infoType !== 'string' || !/^[A-Za-z0-9_.-]{1,128}$/.test(infoType) || typeof systemInfo !== 'boolean') || !info && (infoType !== undefined || systemInfo !== undefined)) continue
    const receivedAt = typeof received === 'string' && received.length <= 64 && Number.isFinite(Date.parse(received)) ? new Date(received).toLocaleString() : '本地接收时间未知'
    seen.add(id); rows.push({ id, kind, index: messageIndex, text, attachments, receivedAt, ...(info ? { infoType: infoType as string, systemInfo: systemInfo as boolean } : {}) })
    if (rows.length === 16) break
  }
  return rows
}
export const IM_HISTORY_SCOPE = '原 IM 本地消费记录，不是运营商短信；持久保存不等于服务端已读、渠道送达、通话或媒体就绪。'
