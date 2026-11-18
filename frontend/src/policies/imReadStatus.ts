// Manual original read-position status is NOT user UDMR or message/SDK ready.
export interface ReadStatusTarget { id: number; kind: 'p2p' | 'org'; index: string }
export interface ReadStatusRow extends ReadStatusTarget { targetId: number; state: 'uncertain' | 'confirmed' | 'rejected' | 'not_sent' }
function position(v: unknown): v is string { return typeof v === 'string' && /^(?:0|[1-9][0-9]{0,18})$/.test(v) && BigInt(v) <= 9223372036854775807n }
function own(v: Record<string, unknown>, k: string): unknown { return Object.prototype.hasOwnProperty.call(v, k) ? v[k] : undefined }
export function readStatusTargets(value: unknown): ReadStatusTarget[] {
  if (!Array.isArray(value)) return []
  const rows: ReadStatusTarget[] = []; const seen = new Set<number>()
  for (const raw of value.slice(0, 128)) {
    if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) continue
    const v = raw as Record<string, unknown>; const id = own(v, 'id'); const kind = own(v, 'box_kind'); const index = own(v, 'target_index')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || seen.has(id) || kind !== 'p2p' && kind !== 'org' || !position(index)) continue
    rows.push({ id, kind, index }); seen.add(id); if (rows.length === 16) break
  }
  return rows
}
export interface ReadStatusSnapshot { context: string; generation: number; rows: ReadStatusRow[] }
export interface ReadStatusUnknown { context: string; targetId: number; index: string; after: number }
export function readStatusRows(value: unknown, limit = 16): ReadStatusRow[] {
  if (!Array.isArray(value) || !Number.isSafeInteger(limit) || limit < 1 || limit > 128) return []
  const rows: ReadStatusRow[] = []; const seen = new Set<number>()
  for (const raw of value.slice(0, 128)) {
    const target = readStatusTargets([raw])[0]
    if (!target || seen.has(target.id)) continue
    const v = raw as Record<string, unknown>; const id = own(v, 'target_id'); const state = own(v, 'state')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || state !== 'uncertain' && state !== 'confirmed' && state !== 'rejected' && state !== 'not_sent') continue
    rows.push({ ...target, targetId: id, state }); seen.add(target.id); if (rows.length === limit) break
  }
  return rows
}
// The action journal is bounded to 128/account. Read the WHOLE journal before
// admitting a UI intent, even though only 16 rows are displayed. Dropped or
// malformed metadata cannot serve as proof that no unknown WRITE exists.
export function readStatusHistory(value: unknown): ReadStatusRow[] | null {
  if (!Array.isArray(value) || value.length > 128) return null
  const rows = readStatusRows(value, 128)
  return rows.length === value.length ? rows : null
}
export function readStatusBaselineReady(snapshot: ReadStatusSnapshot | null, context: string, generation: number): snapshot is ReadStatusSnapshot {
  return snapshot?.context === context && snapshot.generation === generation
}
export function readStatusUnknownResolved(unknown: ReadStatusUnknown, snapshot: ReadStatusSnapshot, context: string, generation: number): boolean {
  if (unknown.context !== context || !readStatusBaselineReady(snapshot, context, generation) || snapshot.rows.some((row) => row.state === 'uncertain')) return false
  return snapshot.rows.some((row) => row.targetId === unknown.targetId && row.index === unknown.index && row.id > unknown.after && row.state !== 'uncertain')
}
export function readStatusRecipientNoWriteError(error: unknown): boolean {
  return error instanceof Error && ['app_account_im_read_status_recipient_unconfirmed', 'app_account_im_read_status_recipient_target_stale', 'app_account_im_read_status_resolution_inflight', 'app_account_im_read_status_resolution_finished', 'app_account_im_read_status_recipient_capacity', 'app_account_im_read_status_recipient_not_installed', 'app_account_im_read_status_recipient_unsupported', 'app_account_im_read_status_request_invalid'].includes(error.message)
}
export const READ_STATUS_SCOPE = '原连续消费位置的手动设置，含群组；不是用户已读 P:P_UDMR-N、消息通知回执、渠道送达或 SDK/媒体授信，不重放未知写入。'
export function readStatusLabel(s: ReadStatusRow['state']): string { switch (s) { case 'uncertain': return '原读位置写入未核实，不重放'; case 'confirmed': return '原手动读位置操作已确认（不是用户已读）'; case 'rejected': return '原手动读位置操作被拒绝'; case 'not_sent': return '原手动读位置操作未写出' } }
