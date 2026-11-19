// Original IM business send observations are not SMS/channel/user-read grants.
export interface ImSendRow { id: number; nonceTag: string; kind: 'text' | 'info'; state: 'uncertain' | 'confirmed' | 'rejected' | 'not_sent'; index: string | null; time: string | null }
function s64(value: unknown): value is string { return typeof value === 'string' && /^(?:0|-?[1-9][0-9]{0,18})$/.test(value) && BigInt(value) >= -9223372036854775808n && BigInt(value) <= 9223372036854775807n }
export function imSendRows(value: unknown): ImSendRow[] | null {
  if (!Array.isArray(value) || value.length > 128) return null
  const rows: ImSendRow[] = []; const seen = new Set<number>(); const tags = new Set<string>()
  for (const raw of value) {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
    const own = (key: string): unknown => Object.prototype.hasOwnProperty.call(raw, key) ? (raw as Record<string, unknown>)[key] : undefined
    const id = own('id'); const nonceTag = own('nonce_tag'); const kind = own('kind'); const state = own('state'); const index = own('message_index'); const time = own('original_time')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || seen.has(id) || typeof nonceTag !== 'string' || !/^[0-9a-f]{64}$/.test(nonceTag) || kind !== 'text' && kind !== 'info' || state !== 'uncertain' && state !== 'confirmed' && state !== 'rejected' && state !== 'not_sent' || index !== null && !s64(index) || time !== null && !s64(time)) return null
    if (tags.has(nonceTag) || state === 'confirmed' && (index === null || time === null) || (state === 'uncertain' || state === 'not_sent') && (index !== null || time !== null)) return null
    seen.add(id); tags.add(nonceTag); rows.push({ id, nonceTag, kind, state, index, time })
  }
  return rows
}
// This exact, authenticated POST failure observation authorizes NO native
// action. It only invalidates the baseline; full current history is required
// again before a user can choose another explicit intent.
export function imSendFailureEnvelope(value: unknown, nonceTag: string): boolean {
  if (!value || typeof value !== 'object' || Array.isArray(value) || !/^[0-9a-f]{64}$/.test(nonceTag)) return false
  const own = (key: string): unknown => Object.prototype.hasOwnProperty.call(value, key) ? (value as Record<string, unknown>)[key] : undefined
  const code = own('message')
  return own('status') === 'error' && own('im_send_admission') === 'not_admitted' && own('nonce_tag') === nonceTag && typeof code === 'string' && /^app_account_[a-z_]{1,96}$/.test(code)
}
export class ImSendNotAdmittedError extends Error {
  readonly nonceTag: string
  constructor(code: string, nonceTag: string) { super(code); this.name = 'ImSendNotAdmittedError'; this.nonceTag = nonceTag }
}
export function imSendNotAdmitted(error: unknown, nonceTag: string): boolean { return error instanceof ImSendNotAdmittedError && /^[0-9a-f]{64}$/.test(nonceTag) && error.nonceTag === nonceTag }
// Bounded optional original notification DATA; none of these is a permission,
// account/endpoint selector, server ACK or actual delivered/read status.
export interface ImSendMetadataInput { digest?: string; userData?: string; onlineMsg?: string; imdnId?: string }
export interface ImSendMetadataBody { notification_digest?: string; parameters?: { key: number[]; value: number[] }[] }
export function imSendMetadataBody(value: unknown): ImSendMetadataBody | null {
  if (value === undefined) return {}
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const keys = ['digest', 'userData', 'onlineMsg', 'imdnId']; if (Object.keys(value).some((key) => !keys.includes(key))) return null
  const own = (key: string): unknown => Object.prototype.hasOwnProperty.call(value, key) ? (value as Record<string, unknown>)[key] : undefined
  if (keys.some((key) => key in value && !Object.prototype.hasOwnProperty.call(value, key))) return null
  const encoder = new TextEncoder(); const body: ImSendMetadataBody = {}; const parameters: { key: number[]; value: number[] }[] = []
  for (const key of keys) { const text = own(key); if (text === undefined) continue; if (typeof text !== 'string' || text.includes('\0') || encoder.encode(text).length > 65536) return null; if (key === 'digest') body.notification_digest = text; else parameters.push({ key: Array.from(encoder.encode(key === 'onlineMsg' ? 'OnlineMsg' : key)), value: Array.from(encoder.encode(text)) }) }
  if (parameters.length > 0) body.parameters = parameters
  return body
}
// Nonce tag is UI correlation DATA, never a native request/ACK/WRITE permit.
export async function imSendNonceTag(nonce: string): Promise<string> { if (!/^[0-9a-f]{32}$/.test(nonce)) throw new Error('Original UI nonce invalid'); const bytes = new TextEncoder().encode(`original-im-send-ui-nonce-v1\0${nonce}`); const hash = await crypto.subtle.digest('SHA-256', bytes); return Array.from(new Uint8Array(hash), (value) => value.toString(16).padStart(2, '0')).join('') }
export function imSendLabel(state: ImSendRow['state']): string { switch (state) { case 'uncertain': return '原 IM 外发结果未知；仅查询，不重放'; case 'confirmed': return '原 IM 外发操作已确认（不是短信或渠道送达）'; case 'rejected': return '原服务端拒绝 IM 外发'; case 'not_sent': return '本次原 IM 外发未写出' } }
export const IM_SEND_SCOPE = '原 IM 业务外发，与短信、呼叫、接收位置、用户已读、通知渠道或 SDK/媒体授信独立；未知结果不自动重放。'
