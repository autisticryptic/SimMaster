// One explicit browser intent, never a retry engine or native authority.
import { captureRelationWriteBody, relationNonceTags, relationSnapshot, relationSnapshotClear, relationMarkerResolved, type RelationLocalMarker, type RelationSnapshot, type RelationWriteBody } from './relationWrite.ts'
import { loadRelationMarker, storeRelationMarker, clearMatchedRelationMarker, type RelationMarkerStore } from './relationWriteStorage.ts'
export interface RelationWritePorts { storage: RelationMarkerStore; current(): boolean; read(): Promise<{ intents: unknown; writes: unknown }>; post(body: RelationWriteBody): Promise<unknown> }
export interface RelationAttemptResult { status: 'observed_terminal' | 'unresolved' | 'blocked' | 'stale'; snapshot?: RelationSnapshot }
function room(snapshot: RelationSnapshot): boolean { return snapshot.intents.length < 128 && snapshot.writes.length < 128 }
export async function submitRelationOnce(ports: RelationWritePorts, line: string, input: RelationWriteBody): Promise<RelationAttemptResult> {
  const body = captureRelationWriteBody(input); if (!body) return { status: 'blocked' }
  // Caller holds the origin Web Lock for this account through all awaits. The
  // synchronous storage guard predates every network/read or nonce hashing wait.
  if (!ports.current()) return { status: 'stale' }
  if (loadRelationMarker(ports.storage, line) !== null) return { status: 'blocked' }
  const baseline = await ports.read(); const snapshot = relationSnapshot(baseline.intents, baseline.writes)
  if (!ports.current()) return { status: 'stale' }
  if (!snapshot || !relationSnapshotClear(snapshot) || !room(snapshot)) return { status: 'blocked' }
  const tags = await relationNonceTags(body.nonce)
  if (!ports.current()) return { status: 'stale' }
  if (loadRelationMarker(ports.storage, line) !== null || snapshot.intents.some((row) => row.nonceTag === tags.intentTag) || snapshot.writes.some((row) => row.nonceTag === tags.writeTag)) return { status: 'blocked' }
  const marker: RelationLocalMarker = { version: 1, ...tags, revision: body.expected_revision, afterIntent: Math.max(0, ...snapshot.intents.map((row) => row.id)), afterWrite: Math.max(0, ...snapshot.writes.map((row) => row.id)), target: body.group_id === null ? 'buddy' : 'group', additions: body.additions.length, updates: body.updates.length, removals: body.removals.length }
  storeRelationMarker(ports.storage, line, marker)
  // No await or cancellation gap between durable local marker and POST start.
  // Every HTTP error/abort or apparently positive reply leaves the marker until
  // exact paired preparation/write history proves its own terminal outcome.
  try { await ports.post(body) } catch { /* Keep ambiguity, do not inspect error text. */ }
  if (!ports.current()) return { status: 'stale' }
  try {
    const raw = await ports.read(); const after = relationSnapshot(raw.intents, raw.writes)
    if (!ports.current()) return { status: 'stale' }
    if (after && relationMarkerResolved(marker, after)) { clearMatchedRelationMarker(ports.storage, line, marker); return { status: 'observed_terminal', snapshot: after } }
    return { status: 'unresolved', ...(after ? { snapshot: after } : {}) }
  } catch { return { status: 'unresolved' } }
}
export async function readRelationOutcome(ports: RelationWritePorts, line: string): Promise<RelationAttemptResult> {
  const marker = loadRelationMarker(ports.storage, line); const raw = await ports.read(); const snapshot = relationSnapshot(raw.intents, raw.writes)
  if (!ports.current()) return { status: 'stale' }
  if (!snapshot) return { status: 'blocked' }
  if (marker) {
    if (!relationMarkerResolved(marker, snapshot)) return { status: 'unresolved', snapshot }
    clearMatchedRelationMarker(ports.storage, line, marker)
  }
  return { status: relationSnapshotClear(snapshot) ? 'observed_terminal' : 'blocked', snapshot }
}
export const RELATION_WRITE_UNRESOLVED = '本次关系修改尚未核实；仅查询自己的准备/结果记录，不换 nonce 重放，也不从空历史推断未写出。'
