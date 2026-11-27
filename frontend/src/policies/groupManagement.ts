import { directoryGroupId } from './nativeDirectory.ts'
export interface GroupManagementBody { expected_revision: number; group_id: string; operation: 'set_properties' | 'disband'; nonce: string; properties: { key: string; value: string | number }[]; consent: true; write_consent: true; destructive_consent: boolean }
const aliases: Record<string, string> = { Name: 'Group.Org.Name', Type: 'Group.Org.Type', UpdateMethod: 'Group.Org.Notify', Permission: 'Group.Org.JoinAuth', PropertyControl: 'Group.Org.PropsControl' }
const limits: Record<string, number> = { Type: 4, UpdateMethod: 2, Permission: 3, PropertyControl: 2 }
// Shared property-list validation, not an operation, group identifier, or WRITE grant.
export function groupPropertyList(json: string, min: number, max: number, initialBytes: number): GroupManagementBody['properties'] | null {
  if (json.length > 262144) return null
  let raw: unknown; try { raw = JSON.parse(json) } catch { return null }
  if (!Array.isArray(raw) || raw.length < min || raw.length > max) return null
  const props: GroupManagementBody['properties'] = []; const keys = new Set<string>(); const enc = new TextEncoder(); let size = initialBytes
  for (const row of raw) {
    if (!row || typeof row !== 'object' || Array.isArray(row) || Reflect.ownKeys(row).length !== 2 || !Object.hasOwn(row, 'key') || !Object.hasOwn(row, 'value')) return null
    const r = row as Record<string, unknown>; if (typeof r.key !== 'string' || !r.key || r.key.includes('\0') || enc.encode(r.key).length > 65536) return null
    const numeric = Object.hasOwn(limits, r.key); const key = Object.hasOwn(aliases, r.key) ? aliases[r.key] : r.key
    if (keys.has(key) || (numeric ? typeof r.value !== 'number' || !Number.isInteger(r.value) || r.value < 0 || r.value > limits[r.key] : typeof r.value !== 'string' || r.value.includes('\0') || new TextDecoder('utf-8', { fatal: true }).decode(enc.encode(r.value)) !== r.value || enc.encode(r.value).length > 65536)) return null
    if (typeof r.value !== 'number' && typeof r.value !== 'string') return null
    keys.add(key); size += 8 + enc.encode(key).length + (numeric ? 15 : enc.encode(String(r.value)).length); props.push({ key: r.key, value: r.value })
  }
  return size > 65536 ? null : props
}
export function groupManagementBody(revision: number, group: string, operation: string, nonce: string, json: string, consent: boolean, writeConsent: boolean, destructive: boolean): GroupManagementBody | null {
  if (!Number.isSafeInteger(revision) || revision < 1 || !directoryGroupId(group) || (operation !== 'set_properties' && operation !== 'disband') || /^[0-9a-f]{32}$/.exec(nonce)?.[0] !== nonce || !consent || !writeConsent || operation === 'disband' && !destructive) return null
  const enc = new TextEncoder(); const props = groupPropertyList(json, operation === 'disband' ? 0 : 1, operation === 'disband' ? 0 : 128, 13 + enc.encode(group).length)
  if (!props) return null
  const body: GroupManagementBody = { expected_revision: revision, group_id: group, operation, nonce, properties: props, consent: true, write_consent: true, destructive_consent: destructive }
  return enc.encode(JSON.stringify(body)).length <= 262144 ? body : null
}
export function captureGroupManagementBody(value: unknown): GroupManagementBody | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const v = value as Record<string, unknown>; const fields = ['expected_revision', 'group_id', 'operation', 'nonce', 'properties', 'consent', 'write_consent', 'destructive_consent']
  if (Reflect.ownKeys(v).length !== fields.length || !fields.every((f) => Object.hasOwn(v, f)) || typeof v.expected_revision !== 'number' || typeof v.group_id !== 'string' || typeof v.operation !== 'string' || typeof v.nonce !== 'string' || typeof v.destructive_consent !== 'boolean') return null
  try { return groupManagementBody(v.expected_revision, v.group_id, v.operation, v.nonce, JSON.stringify(v.properties), v.consent === true, v.write_consent === true, v.destructive_consent) } catch { return null }
}
export const GROUP_MANAGEMENT_SCOPE = '原群组属性修改/解散是真实 WRITE。与成员修改共用未决排他，不从名册、事件或关系检查授予权限。属性修改没有原始时间字段；解散不等于删除单个成员。'
