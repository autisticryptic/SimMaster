// Native directory observations are READ DATA, not SDK, IM ACK or WRITE grants.
export type DirectoryOperation = 'relations' | 'properties'
export interface DirectoryInput { operation: DirectoryOperation; groupId: string; since: string }
export interface DirectorySnapshot { operation: DirectoryOperation; targetKind: 'current_user' | 'group'; accepted: boolean; reported?: boolean; time?: string; first: number; second: number; names: number; properties: number }
const own = (value: object, key: string): unknown => Object.prototype.hasOwnProperty.call(value, key) ? (value as Record<string, unknown>)[key] : undefined
function signed64(value: unknown): value is string {
  if (typeof value !== 'string' || !/^(?:0|-[1-9][0-9]{0,18}|[1-9][0-9]{0,18})$/.test(value)) return false
  const n = BigInt(value); return n >= -9223372036854775808n && n <= 9223372036854775807n
}
export function directoryGroupId(value: unknown): value is string { return typeof value === 'string' && value.length <= 508 && /^[1-9][0-9]*[-+][1-9][0-9]*$/.test(value) }
export function directoryBody(revision: number, value: unknown): { expected_revision: number; operation: DirectoryOperation; group_id?: string; since: string; consent: true } | null {
  if (!Number.isSafeInteger(revision) || revision <= 0 || typeof value !== 'object' || value === null || Array.isArray(value)) return null
  if (Object.keys(value).some((key) => !['operation', 'groupId', 'since'].includes(key))) return null
  const operation = own(value, 'operation'); const group = own(value, 'groupId'); const since = own(value, 'since')
  if ((operation !== 'relations' && operation !== 'properties') || typeof group !== 'string' || (group !== '' && !directoryGroupId(group)) || !signed64(since) || BigInt(since) < -1n || operation === 'properties' && (group === '' || since !== '-1')) return null
  return { expected_revision: revision, operation, ...(group === '' ? {} : { group_id: group }), since, consent: true }
}
export function directorySnapshot(value: unknown, requested: DirectoryInput): DirectorySnapshot | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null
  const operation = own(value, 'operation'); const target = own(value, 'target_kind'); const accepted = own(value, 'accepted'); const reported = own(value, 'reported'); const time = own(value, 'original_time')
  const first = own(value, 'first_relations'); const second = own(value, 'second_relations'); const names = own(value, 'names'); const properties = own(value, 'properties')
  const count = (value: unknown): value is number => typeof value === 'number' && Number.isInteger(value) && value >= 0 && value <= 128
  if (operation !== requested.operation || target !== (requested.groupId === '' ? 'current_user' : 'group') || typeof accepted !== 'boolean' || !count(first) || !count(second) || !count(names) || !count(properties)) return null
  if (operation === 'relations' ? typeof reported !== 'boolean' || !signed64(time) || properties !== 0 : reported !== undefined || time !== undefined || first !== 0 || second !== 0 || names !== 0) return null
  return { operation, targetKind: target as 'current_user' | 'group', accepted, ...(operation === 'relations' ? { reported: reported as boolean, time: time as string } : {}), first, second, names, properties }
}
export const DIRECTORY_READ_SCOPE = '原 Group/Buddy 目录只读观测：不创建关系、群组、短信或通知，不标记已读、不解决未知 WRITE；返回字段和数量不代表 SDK 上线、联系人在线或渠道送达。'
