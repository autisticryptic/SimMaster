// Native received-position receipts are not user read/open or media grants.
export type ImReceiptState = 'uncertain' | 'confirmed' | 'rejected' | 'not_sent'
export interface ImReceiptRow { id: number; kind: 'p2p' | 'org'; index: string; state: ImReceiptState; updatedAt: string }
function index(value: unknown): value is string {
  if (typeof value !== 'string' || !/^(?:0|[1-9][0-9]{0,18})$/.test(value)) return false
  return BigInt(value) <= 9223372036854775807n
}
export function imReceiptRows(value: unknown): ImReceiptRow[] {
  if (!Array.isArray(value)) return []
  const rows: ImReceiptRow[] = []; const seen = new Set<number>()
  for (const entry of value.slice(0, 128)) {
    if (typeof entry !== 'object' || entry === null || Array.isArray(entry)) continue
    const own = (name: string): unknown => Object.prototype.hasOwnProperty.call(entry, name) ? (entry as Record<string, unknown>)[name] : undefined
    const id = own('id'); const kind = own('box_kind'); const target = own('target_index'); const state = own('state'); const updated = own('updated_at')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || seen.has(id) || (kind !== 'p2p' && kind !== 'org') || !index(target)) continue
    if (state !== 'uncertain' && state !== 'confirmed' && state !== 'rejected' && state !== 'not_sent') continue
    const updatedAt = typeof updated === 'string' && updated.length <= 64 && !updated.includes('\0') && Number.isFinite(Date.parse(updated)) ? new Date(updated).toLocaleString() : '本地观察时间未知'
    seen.add(id); rows.push({ id, kind, index: target, state, updatedAt })
    if (rows.length === 16) break
  }
  return rows
}
export function imReceiptLabel(state: ImReceiptState): string {
  switch (state) {
    case 'uncertain': return '接收位置写入尚未核实；不重放'
    case 'confirmed': return '原接收位置回执已确认'
    case 'rejected': return '原服务端拒绝接收位置回执'
    case 'not_sent': return '本次回执未写出'
  }
}
export const IM_RECEIPT_SCOPE = '原 IM 接收位置回执，不是用户已读、渠道送达或 SDK/媒体就绪；不确定写入不会按新 nonce 或重启重放。'
