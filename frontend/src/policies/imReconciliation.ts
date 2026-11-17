import type { UserReadState } from './imUserRead'
export interface ImReconciliationView {
  receiptId: number; messageId: number; targetIndex: string; localState: UserReadState
  report: 'reported' | 'unreported' | 'rejected'; lastRead: string | null; lastReceived: string | null; latest: string | null; serverTime: string | null; observedAt: string
}
function index(value: unknown, negative: boolean): value is string {
  if (typeof value !== 'string' || !/^(?:-1|0|[1-9][0-9]{0,18})$/.test(value) || !negative && value.startsWith('-')) return false
  return BigInt(value) <= 9223372036854775807n
}
export function imReconciliationView(value: unknown): ImReconciliationView | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null
  const own = (name: string): unknown => Object.prototype.hasOwnProperty.call(value, name) ? (value as Record<string, unknown>)[name] : undefined
  const receiptId = own('receipt_id'); const messageId = own('message_id'); const target = own('target_index'); const state = own('local_state'); const report = own('report')
  if (typeof receiptId !== 'number' || !Number.isSafeInteger(receiptId) || receiptId <= 0 || typeof messageId !== 'number' || !Number.isSafeInteger(messageId) || messageId <= 0 || !index(target, false)) return null
  if (state !== 'uncertain' && state !== 'confirmed' && state !== 'rejected' && state !== 'not_sent' || report !== 'reported' && report !== 'unreported' && report !== 'rejected') return null
  const read = own('reported_last_read'); const received = own('reported_last_received'); const latest = own('reported_latest'); const server = own('server_update_time'); const time = own('observed_at')
  for (const field of [read, received, latest, server]) if (field !== null && !index(field, true)) return null
  if (report !== 'reported' && [read, received, latest, server].some((field) => field !== null)) return null
  if (typeof time !== 'string' || time.length > 64 || time.includes('\0') || !Number.isFinite(Date.parse(time))) return null
  return { receiptId, messageId, targetIndex: target, localState: state, report, lastRead: read as string | null, lastReceived: received as string | null, latest: latest as string | null, serverTime: server as string | null, observedAt: new Date(time).toLocaleString() }
}
export function imReconciliationLabel(value: ImReconciliationView): string {
  if (value.report === 'rejected') return '本次原状态查询被拒绝；不能判断原写入是否执行。'
  if (value.report === 'unreported' || value.lastRead === null) return '原服务端未报告用户已读位置；不等于原写入未执行。'
  return BigInt(value.lastRead) >= BigInt(value.targetIndex) ? '当前报告已读位置达到目标；不是原写入匹配回执，未知记录保持。' : '当前报告已读位置低于目标；不等于原写入未执行，未知记录保持。'
}
export const IM_RECONCILIATION_SCOPE = '独立只读核对；服务端当前报告不替代原写入回执，不清空未知写入、不重放，不授予 SDK、短信、通话或媒体就绪。'
