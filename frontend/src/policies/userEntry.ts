export interface UserEntryView { kind: 'uid' | 'phone' | 'email' | 'username'; report: 'reported' | 'unreported' | 'rejected'; observedAt: string }
export function userEntryView(value: unknown): UserEntryView | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null
  const own = (key: string): unknown => Object.prototype.hasOwnProperty.call(value, key) ? (value as Record<string, unknown>)[key] : undefined
  const kind = own('kind'); const report = own('report'); const time = own('observed_at')
  if (kind !== 'uid' && kind !== 'phone' && kind !== 'email' && kind !== 'username' || report !== 'reported' && report !== 'unreported' && report !== 'rejected') return null
  if (typeof time !== 'string' || time.length > 64 || Array.from(time).some((c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) === 127) || !Number.isFinite(Date.parse(time))) return null
  return { kind, report, observedAt: new Date(time).toLocaleString() }
}
export function userEntryRecipient(value: string): boolean {
  if (!value || new TextEncoder().encode(value).length > 512 || Array.from(value).some((c) => c.charCodeAt(0) <= 32 || c.charCodeAt(0) === 127)) return false
  const uid = (v: string) => /^[0-9]+_[1-9][0-9]*$/.test(v)
  if (uid(value)) return true
  const shape = /^\[(uid|phone|email|username):(.+)\]$/.exec(value)
  if (!shape) return false
  const [, kind, rest] = shape; const at = rest.lastIndexOf('@')
  const recipient = at < 0 ? rest : rest.slice(0, at); const realm = at < 0 ? null : rest.slice(at + 1)
  if (realm !== null && (!realm.includes('.') || Array.from(realm).some((c) => '[]{}:/\\'.includes(c)))) return false
  if (kind === 'uid') return uid(recipient)
  if (realm === null) return false
  if (kind === 'phone') return recipient.length > 2 && /^\+?[0-9]+$/.test(recipient)
  if (kind === 'username') return /^[A-Za-z0-9+_][A-Za-z0-9+_.-]{0,63}$/.test(recipient)
  return recipient.length > 2 && recipient.split('@').length === 2 && recipient.slice(recipient.indexOf('@') + 1).includes('.')
}
export const USER_ENTRY_SCOPE = '仅向本次原账号 UserEntry 明确查询；不展示 UID，不标记已读、不清空未知写入、不发送消息，不重新登录或恢复授信。'
export function userEntryLabel(value: UserEntryView): string { return value.report === 'reported' ? '本次原服务端报告已解析；不是写入回执或接收就绪。' : value.report === 'unreported' ? '本次原服务端未报告收件人；不猜测 UID，不自动重查。' : '本次原查询被拒绝；不自动重试或重新登录。' }
