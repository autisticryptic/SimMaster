import { parseChanges } from './relationWrite.ts'
import { groupPropertyList } from './groupManagement.ts'
import { relationHistoryRows, type RelationHistoryRow } from './relationHistory.ts'
export interface GroupCreateBody { expected_revision: number; nonce: string; name: string; group_type: number; expected_owner_uid: string; members: { uid: string; relation_type: number; first: string; second: string; properties: { key: number[]; value: number[] }[] }[]; properties: { key: string; value: string | number }[]; consent: true; write_consent: true; properties_read_consent: boolean }
export interface GroupCreateOutcome { write: RelationHistoryRow; read: { state: 'not_requested' | 'not_attempted' | 'unverified' | 'observed' | 'rejected'; accepted: boolean | null; properties: number | null } }
function text(s: string, max: number): boolean { const enc = new TextEncoder(); return !s.includes('\0') && new TextDecoder('utf-8', { fatal: true }).decode(enc.encode(s)) === s && enc.encode(s).length <= max }
export function groupCreateBody(revision: number, nonce: string, name: string, groupType: number, owner: string, membersJson: string, propertiesJson: string, consent: boolean, writeConsent: boolean, readConsent: boolean): GroupCreateBody | null {
  if (!Number.isSafeInteger(revision) || revision < 1 || !name || !text(name, 65536) || !owner || !text(owner, 512) || Array.from(owner, (ch) => ch.charCodeAt(0)).some((n) => n < 32 || n === 127) || !Number.isInteger(groupType) || groupType < 0 || groupType > 4 || /^[0-9a-f]{32}$/.exec(nonce)?.[0] !== nonce || !consent || !writeConsent) return null
  if (membersJson.length > 262144 || new TextEncoder().encode(membersJson).length > 262144) return null
  const members = parseChanges(membersJson, true)
  if (!members || members.length === 0 || members.filter((m) => m.label === owner).length !== 1) return null
  const uid = (s: string) => /^[0-9]+_[1-9][0-9]*$/.exec(s)?.[0] === s && text(s, 512)
  if (members.some((m) => m.label === owner && m.relation_type !== 257 || m.label !== owner && !uid(m.label) || !text(m.label, 65536) || !text(m.first, 65536) || !text(m.second, 65536) || m.properties.some((p) => p.key.includes(0) || p.value.includes(0)))) return null
  const properties = groupPropertyList(propertiesJson, 0, 126, 0)
  if (!properties || properties.some((p) => !text(p.key, 65536) || ['Name', 'Type', 'Group.Org.Name', 'Group.Org.Type'].includes(p.key))) return null
  const enc = new TextEncoder(); let wire = 128 + enc.encode(name).length
  for (const m of members) wire += 20 + enc.encode(m.label + m.first + m.second).length + m.properties.reduce((n, p) => n + 8 + p.key.length + p.value.length, 0)
  for (const p of properties) wire += 8 + enc.encode(p.key).length + (typeof p.value === 'number' ? 32 : enc.encode(p.value).length)
  const body: GroupCreateBody = { expected_revision: revision, nonce, name, group_type: groupType, expected_owner_uid: owner, members: members.map((m) => ({ uid: m.label, relation_type: m.relation_type, first: m.first, second: m.second, properties: m.properties })), properties, consent: true, write_consent: true, properties_read_consent: readConsent }
  return wire <= 65536 && enc.encode(JSON.stringify(body)).length <= 262144 ? body : null
}
export function captureGroupCreateBody(value: unknown): GroupCreateBody | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const v = value as Record<string, unknown>; const fields = ['expected_revision', 'nonce', 'name', 'group_type', 'expected_owner_uid', 'members', 'properties', 'consent', 'write_consent', 'properties_read_consent']
  if (Reflect.ownKeys(v).length !== fields.length || !fields.every((k) => Object.hasOwn(v, k)) || typeof v.expected_revision !== 'number' || typeof v.nonce !== 'string' || typeof v.name !== 'string' || typeof v.group_type !== 'number' || typeof v.expected_owner_uid !== 'string' || typeof v.properties_read_consent !== 'boolean' || !Array.isArray(v.members) || v.members.length > 128 || !Array.isArray(v.properties) || v.properties.length > 126) return null
  const members: unknown[] = []
  for (const raw of v.members) { if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null; const m = raw as Record<string, unknown>; const keys = ['uid', 'relation_type', 'first', 'second', 'properties']; if (Reflect.ownKeys(m).length !== keys.length || !keys.every((k) => Object.hasOwn(m, k))) return null; members.push({ label: m.uid, relation_type: m.relation_type, first: m.first, second: m.second, properties: m.properties }) }
  try { return groupCreateBody(v.expected_revision, v.nonce, v.name, v.group_type, v.expected_owner_uid, JSON.stringify(members), JSON.stringify(v.properties), v.consent === true, v.write_consent === true, v.properties_read_consent) } catch { return null }
}
export function groupCreateOutcome(value: unknown, operation: 'create' | 'create_owner_only' = 'create'): GroupCreateOutcome | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const v = value as Record<string, unknown>; const keys = Reflect.ownKeys(v)
  if (keys.length !== 2 || !keys.includes('write') || !keys.includes('properties_read')) return null
  const write = relationHistoryRows([v.write])?.[0]; if (!write || write.management?.operation !== operation) return null
  if (!v.properties_read || typeof v.properties_read !== 'object' || Array.isArray(v.properties_read)) return null
  const r = v.properties_read as Record<string, unknown>; const rk = Reflect.ownKeys(r); if (rk.length !== 3 || !['state', 'accepted', 'properties'].every((k) => rk.includes(k))) return null
  const state = r.state; if (state !== 'not_requested' && state !== 'not_attempted' && state !== 'unverified' && state !== 'observed' && state !== 'rejected') return null
  if (state === 'observed' || state === 'rejected') { if (write.state !== 'confirmed' || r.accepted !== (state === 'observed') || typeof r.properties !== 'number' || !Number.isSafeInteger(r.properties) || r.properties < 0 || r.properties > 128) return null; return { write, read: { state, accepted: state === 'observed', properties: r.properties } } }
  if (r.accepted !== null || r.properties !== null) return null
  return { write, read: { state, accepted: null, properties: null } }
}
export const GROUP_CREATE_SCOPE = '仅显式 owner/成员创建路径：owner UID 必须与当前原 SDK 返回值精确匹配。不自动补入 App 用户名，不解析电话/URI。创建 WRITE 与后续属性 READ 分开；READ 失败不撤销已确认创建，也不重放 Unknown-WRITE。'
