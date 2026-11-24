// Explicit relation WRITE inputs and conservative outcome correlation only.
// No cached directory/UID/SDK flag/history row can create a native permit.
import { directoryGroupId } from './nativeDirectory.ts'
import { relationHistoryRows, type RelationHistoryRow } from './relationHistory.ts'
export interface RelationDraft { target: 'buddy' | 'group'; groupId: string; additions: string; updates: string; removals: string }
export interface RelationChangeBody { label: string; relation_type: number; first: string; second: string; properties: { key: number[]; value: number[] }[] }
export interface RelationWriteBody { expected_revision: number; nonce: string; group_id: string | null; additions: RelationChangeBody[]; updates: RelationChangeBody[]; removals: { label: string; flag: boolean }[]; consent: true; write_consent: true }
export interface RelationIntentRow { id: number; nonceTag: string; state: 'preparing' | 'bound' | 'not_sent'; writeId: number | null; targetKind: 'buddy' | 'group'; additions: number; updates: number; removals: number }
export interface RelationSnapshot { intents: RelationIntentRow[]; writes: RelationHistoryRow[] }
export interface RelationLocalMarker { version: 1; intentTag: string; writeTag: string; revision: number; afterIntent: number; afterWrite: number; target: 'buddy' | 'group'; additions: number; updates: number; removals: number }
const own = (v: object, k: string): unknown => Object.prototype.hasOwnProperty.call(v, k) ? (v as Record<string, unknown>)[k] : undefined
function object(v: unknown): v is Record<string, unknown> { return v !== null && typeof v === 'object' && !Array.isArray(v) }
function keys(v: Record<string, unknown>, allowed: string[], required: string[]): boolean { return Object.keys(v).every((k) => allowed.includes(k)) && required.every((k) => Object.prototype.hasOwnProperty.call(v, k)) && allowed.every((k) => !(k in v) || Object.prototype.hasOwnProperty.call(v, k)) }
function safeId(v: unknown): v is number { return typeof v === 'number' && Number.isSafeInteger(v) && v > 0 }
function counter(v: unknown): v is number { return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 && v <= 128 }
function tag(v: unknown): v is string { return typeof v === 'string' && /^[0-9a-f]{64}$/.test(v) }
function utf8(v: unknown): v is string { return typeof v === 'string' && new TextDecoder('utf-8', { fatal: true }).decode(new TextEncoder().encode(v)) === v && new TextEncoder().encode(v).length <= 65536 }
function bytes(v: unknown): v is number[] { return Array.isArray(v) && v.length <= 65536 && v.every((n: unknown) => typeof n === 'number' && Number.isInteger(n) && n >= 0 && n <= 255) }
const nativeTypes = [11, 12, 13, 14, 15, 16, 18, 257, 258, 259, 4097]
function parseChanges(text: string): RelationChangeBody[] | null {
  let raw: unknown; try { raw = JSON.parse(text) } catch { return null }
  if (!Array.isArray(raw) || raw.length > 128) return null
  const result: RelationChangeBody[] = []; const seen = new Set<string>()
  for (const entry of raw) {
    if (!object(entry) || !keys(entry, ['label', 'relation_type', 'first', 'second', 'properties'], ['label', 'relation_type'])) return null
    const label = own(entry, 'label'); const type = own(entry, 'relation_type'); const first = own(entry, 'first') === undefined ? '' : own(entry, 'first'); const second = own(entry, 'second') === undefined ? '' : own(entry, 'second'); const props = own(entry, 'properties') === undefined ? [] : own(entry, 'properties')
    if (!utf8(label) || label.length === 0 || seen.has(label) || typeof type !== 'number' || !nativeTypes.includes(type) || !utf8(first) || !utf8(second) || !Array.isArray(props) || props.length > 128) return null
    const properties: { key: number[]; value: number[] }[] = []; const propertyKeys = new Set<string>()
    for (const property of props) {
      if (!object(property) || !keys(property, ['key', 'value'], ['key', 'value'])) return null
      const key = own(property, 'key'); const value = own(property, 'value'); if (!bytes(key) || !bytes(value)) return null
      const fingerprint = key.join(','); if (propertyKeys.has(fingerprint)) return null
      propertyKeys.add(fingerprint); properties.push({ key: [...key], value: [...value] })
    }
    seen.add(label); result.push({ label, relation_type: type, first, second, properties })
  }
  return result
}
function parseRemovals(text: string): RelationWriteBody['removals'] | null {
  let raw: unknown; try { raw = JSON.parse(text) } catch { return null }
  if (!Array.isArray(raw) || raw.length > 128) return null
  const result: RelationWriteBody['removals'] = []; const seen = new Set<string>()
  for (const entry of raw) {
    if (!object(entry) || !keys(entry, ['label', 'flag'], ['label'])) return null
    const label = own(entry, 'label'); const flag = own(entry, 'flag') === undefined ? false : own(entry, 'flag')
    if (!utf8(label) || label.length === 0 || seen.has(label) || typeof flag !== 'boolean') return null
    seen.add(label); result.push({ label, flag })
  }
  return result
}
export function relationWriteBody(revision: number, nonce: string, draft: unknown, consent: boolean, writeConsent: boolean): RelationWriteBody | null {
  if (!safeId(revision) || !/^[0-9a-f]{32}$/.test(nonce) || !consent || !writeConsent || !object(draft) || !keys(draft, ['target', 'groupId', 'additions', 'updates', 'removals'], ['target', 'groupId', 'additions', 'updates', 'removals'])) return null
  const target = own(draft, 'target'); const groupId = own(draft, 'groupId'); const additions = own(draft, 'additions'); const updates = own(draft, 'updates'); const removals = own(draft, 'removals')
  if (target !== 'buddy' && target !== 'group' || typeof groupId !== 'string' || target === 'buddy' && groupId !== '' || target === 'group' && !directoryGroupId(groupId) || typeof additions !== 'string' || typeof updates !== 'string' || typeof removals !== 'string' || new TextEncoder().encode(additions + updates + removals).length > 262144) return null
  const add = parseChanges(additions); const update = parseChanges(updates); const remove = parseRemovals(removals)
  if (!add || !update || !remove || add.length + update.length + remove.length === 0) return null
  const labels = [...add, ...update, ...remove].map((entry) => entry.label); if (new Set(labels).size !== labels.length) return null
  // Same original payload limits; current UID length is never guessed here.
  // The original runtime performs exact final measurement with its actual UID.
  const encoder = new TextEncoder(); let wire = 21 + encoder.encode(target === 'group' ? groupId : 'shape-only').length
  for (const change of [...add, ...update]) wire += 20 + encoder.encode(change.label + change.first + change.second).length + change.properties.reduce((n, p) => n + 8 + p.key.length + p.value.length, 0)
  for (const removal of remove) wire += 5 + encoder.encode(removal.label).length
  const body: RelationWriteBody = { expected_revision: revision, nonce, group_id: target === 'group' ? groupId : null, additions: add, updates: update, removals: remove, consent: true, write_consent: true }
  if (wire > 131072 || encoder.encode(JSON.stringify(body)).length > 262144) return null
  return body
}
export function captureRelationWriteBody(value: unknown): RelationWriteBody | null {
  const fields = ['expected_revision', 'nonce', 'group_id', 'additions', 'updates', 'removals', 'consent', 'write_consent']
  if (!object(value) || !keys(value, fields, fields)) return null
  const revision = own(value, 'expected_revision'); const nonce = own(value, 'nonce'); const group = own(value, 'group_id'); const consent = own(value, 'consent'); const writeConsent = own(value, 'write_consent')
  if (typeof revision !== 'number' || typeof nonce !== 'string' || group !== null && typeof group !== 'string' || consent !== true || writeConsent !== true) return null
  try { return relationWriteBody(revision, nonce, { target: group === null ? 'buddy' : 'group', groupId: group ?? '', additions: JSON.stringify(own(value, 'additions')), updates: JSON.stringify(own(value, 'updates')), removals: JSON.stringify(own(value, 'removals')) }, true, true) } catch { return null }
}
export function relationIntentRows(value: unknown): RelationIntentRow[] | null {
  if (!Array.isArray(value) || value.length > 128) return null
  const result: RelationIntentRow[] = []; const ids = new Set<number>(); const tags = new Set<string>(); const writes = new Set<number>()
  for (const raw of value) {
    const fields = ['id', 'nonce_tag', 'state', 'write_id', 'target_kind', 'additions', 'updates', 'removals', 'updated_at']
    if (!object(raw) || !keys(raw, fields, fields)) return null
    const id = own(raw, 'id'); const nonceTag = own(raw, 'nonce_tag'); const state = own(raw, 'state'); const writeId = own(raw, 'write_id'); const targetKind = own(raw, 'target_kind'); const additions = own(raw, 'additions'); const updates = own(raw, 'updates'); const removals = own(raw, 'removals'); const at = own(raw, 'updated_at')
    if (!safeId(id) || ids.has(id) || !tag(nonceTag) || tags.has(nonceTag) || state !== 'preparing' && state !== 'bound' && state !== 'not_sent' || targetKind !== 'buddy' && targetKind !== 'group' || !counter(additions) || !counter(updates) || !counter(removals) || additions + updates + removals === 0 || typeof at !== 'string' || at.length > 64 || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.[0-9]{1,9})?(?:Z|[+-]\d{2}:\d{2})$/.test(at)) return null
    let capturedWrite: number | null = null
    if (state === 'bound') { if (!safeId(writeId) || writes.has(writeId)) return null; writes.add(writeId); capturedWrite = writeId } else if (writeId !== null) return null
    ids.add(id); tags.add(nonceTag); result.push({ id, nonceTag, state, writeId: capturedWrite, targetKind, additions, updates, removals })
  }
  return result
}
function sameShape(a: { targetKind: string; additions: number; updates: number; removals: number }, b: { targetKind: string; additions: number; updates: number; removals: number }): boolean { return a.targetKind === b.targetKind && a.additions === b.additions && a.updates === b.updates && a.removals === b.removals }
export function relationSnapshot(intents: unknown, writes: unknown): RelationSnapshot | null {
  const prepared = relationIntentRows(intents); const results = relationHistoryRows(writes); if (!prepared || !results) return null
  for (const row of prepared) { if (row.state !== 'bound') continue; const result = results.find((item) => item.id === row.writeId); if (!result || !sameShape(row, result)) return null }
  return { intents: prepared, writes: results }
}
export function relationSnapshotClear(value: RelationSnapshot): boolean { return !value.intents.some((row) => row.state === 'preparing') && !value.writes.some((row) => row.state === 'uncertain') }
export function relationMarker(value: unknown): RelationLocalMarker | null {
  const fields = ['version', 'intentTag', 'writeTag', 'revision', 'afterIntent', 'afterWrite', 'target', 'additions', 'updates', 'removals']
  if (!object(value) || !keys(value, fields, fields)) return null
  const version = own(value, 'version'); const intentTag = own(value, 'intentTag'); const writeTag = own(value, 'writeTag'); const revision = own(value, 'revision'); const afterIntent = own(value, 'afterIntent'); const afterWrite = own(value, 'afterWrite'); const target = own(value, 'target'); const additions = own(value, 'additions'); const updates = own(value, 'updates'); const removals = own(value, 'removals')
  if (version !== 1 || !tag(intentTag) || !tag(writeTag) || !safeId(revision) || typeof afterIntent !== 'number' || !Number.isSafeInteger(afterIntent) || afterIntent < 0 || typeof afterWrite !== 'number' || !Number.isSafeInteger(afterWrite) || afterWrite < 0 || target !== 'buddy' && target !== 'group' || !counter(additions) || !counter(updates) || !counter(removals) || additions + updates + removals === 0) return null
  return { version, intentTag, writeTag, revision, afterIntent, afterWrite, target, additions, updates, removals }
}
export function relationMarkerResolved(marker: RelationLocalMarker, snapshot: RelationSnapshot): boolean {
  if (!relationSnapshotClear(snapshot)) return false
  const intent = snapshot.intents.find((row) => row.nonceTag === marker.intentTag && row.id > marker.afterIntent)
  const expected = { targetKind: marker.target, additions: marker.additions, updates: marker.updates, removals: marker.removals }
  if (!intent || !sameShape(intent, expected)) return false
  if (intent.state === 'not_sent') return intent.writeId === null
  if (intent.state !== 'bound') return false
  const write = snapshot.writes.find((row) => row.id === intent.writeId && row.id > marker.afterWrite && row.nonceTag === marker.writeTag)
  return !!write && write.state !== 'uncertain' && sameShape(write, expected)
}
export async function relationNonceTags(nonce: string): Promise<{ intentTag: string; writeTag: string }> {
  if (!/^[0-9a-f]{32}$/.test(nonce)) throw new Error('Invalid original relation nonce')
  const hash = async (label: string) => {
    const encoder = new TextEncoder(); const parts = [encoder.encode(label), encoder.encode(nonce)]; const buffer = new Uint8Array(parts.reduce((n, p) => n + 8 + p.length, 0)); let offset = 0
    for (const part of parts) { new DataView(buffer.buffer).setBigUint64(offset, BigInt(part.length), false); offset += 8; buffer.set(part, offset); offset += part.length }
    const digest = await crypto.subtle.digest('SHA-256', buffer); return Array.from(new Uint8Array(digest), (b) => b.toString(16).padStart(2, '0')).join('')
  }
  return { intentTag: await hash('original-relation-intent-ui-nonce-v1'), writeTag: await hash('original-relation-ui-nonce-v1') }
}
export const RELATION_WRITE_SCOPE = '独立 Group/Buddy 关系修改：不是群组创建、目录 READ、短信、IM 外发或 SDK 授信；未知结果仅查询，不换 nonce 重放。'
