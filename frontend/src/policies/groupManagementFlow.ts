import { captureGroupManagementBody, type GroupManagementBody } from './groupManagement.ts'
import { relationNonceTags, relationSnapshot, relationSnapshotClear, relationMarkerResolved, type RelationLocalMarker } from './relationWrite.ts'
import { loadRelationMarker, storeRelationMarker, clearMatchedRelationMarker, type RelationMarkerStore } from './relationWriteStorage.ts'
import type { RelationAttemptResult } from './relationWriteFlow.ts'
export interface ManagementPorts { storage: RelationMarkerStore; current(): boolean; read(): Promise<{ intents: unknown; writes: unknown }>; post(body: GroupManagementBody): Promise<unknown> }
// Same origin lock and storage key as membership WRITEs, not a second bypass.
export async function submitGroupManagementOnce(ports: ManagementPorts, line: string, input: unknown): Promise<RelationAttemptResult> {
  const body = captureGroupManagementBody(input); if (!body) return { status: 'blocked' }
  if (!ports.current()) return { status: 'stale' }
  if (loadRelationMarker(ports.storage, line) !== null) return { status: 'blocked' }
  const raw = await ports.read(); const baseline = relationSnapshot(raw.intents, raw.writes)
  if (!ports.current()) return { status: 'stale' }
  if (!baseline || !relationSnapshotClear(baseline) || baseline.intents.length >= 128 || baseline.writes.length >= 128) return { status: 'blocked' }
  const tags = await relationNonceTags(body.nonce)
  if (!ports.current()) return { status: 'stale' }
  if (loadRelationMarker(ports.storage, line) !== null || baseline.intents.some((v) => v.nonceTag === tags.intentTag) || baseline.writes.some((v) => v.nonceTag === tags.writeTag)) return { status: 'blocked' }
  const marker: RelationLocalMarker = { version: 1, ...tags, revision: body.expected_revision, afterIntent: Math.max(0, ...baseline.intents.map((v) => v.id)), afterWrite: Math.max(0, ...baseline.writes.map((v) => v.id)), target: 'group', additions: 0, updates: 0, removals: 0, management: { operation: body.operation, properties: body.properties.length } }
  storeRelationMarker(ports.storage, line, marker)
  try { await ports.post(body) } catch { /* Unknown remains; no retry or response parsing. */ }
  if (!ports.current()) return { status: 'stale' }
  try {
    const raw = await ports.read(); const after = relationSnapshot(raw.intents, raw.writes)
    if (!ports.current()) return { status: 'stale' }
    if (after && relationMarkerResolved(marker, after)) { clearMatchedRelationMarker(ports.storage, line, marker); return { status: 'observed_terminal', snapshot: after } }
    return { status: 'unresolved', ...(after ? { snapshot: after } : {}) }
  } catch { return { status: 'unresolved' } }
}
