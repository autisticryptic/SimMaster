import { groupPropertyList } from './groupManagement.ts'
export interface OwnerOnlyBody {
  expected_revision: number; nonce: string; name: string; group_type: number
  properties: { key: string; value: string | number }[]
  consent: true; write_consent: true; session_owner_consent: true; properties_read_consent: boolean
}
export function ownerOnlyBody(revision: number, nonce: string, name: string, groupType: number, json: string, consent: boolean, write: boolean, sessionOwner: boolean, read: boolean): OwnerOnlyBody | null {
  const enc = new TextEncoder(); const bytes = enc.encode(name)
  if (!Number.isSafeInteger(revision) || revision < 1 || /^[0-9a-f]{32}$/.exec(nonce)?.[0] !== nonce || !name || name.includes('\0') || bytes.length > 65536 || new TextDecoder('utf-8', { fatal: true }).decode(bytes) !== name || !Number.isInteger(groupType) || groupType < 0 || groupType > 4 || !consent || !write || !sessionOwner) return null
  const properties = groupPropertyList(json, 0, 126, 64 + bytes.length)
  if (!properties || properties.some((p) => new TextDecoder('utf-8', { fatal: true }).decode(enc.encode(p.key)) !== p.key || ['Name','Type','Group.Org.Name','Group.Org.Type'].includes(p.key))) return null
  const body: OwnerOnlyBody = { expected_revision: revision, nonce, name, group_type: groupType, properties, consent: true, write_consent: true, session_owner_consent: true, properties_read_consent: read }
  return enc.encode(JSON.stringify(body)).length <= 262144 ? body : null
}
export function captureOwnerOnlyBody(value: unknown): OwnerOnlyBody | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const v = value as Record<string, unknown>; const fields = ['expected_revision','nonce','name','group_type','properties','consent','write_consent','session_owner_consent','properties_read_consent']
  if (Reflect.ownKeys(v).length !== fields.length || !fields.every((k) => Object.hasOwn(v,k)) || typeof v.expected_revision !== 'number' || typeof v.name !== 'string' || typeof v.nonce !== 'string' || typeof v.group_type !== 'number' || typeof v.properties_read_consent !== 'boolean' || !Array.isArray(v.properties) || v.properties.length > 126) return null
  try { return ownerOnlyBody(v.expected_revision, v.nonce, v.name, v.group_type, JSON.stringify(v.properties), v.consent === true, v.write_consent === true, v.session_owner_consent === true, v.properties_read_consent) } catch { return null }
}
