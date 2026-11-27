import { captureGroupCreateBody, groupCreateOutcome, type GroupCreateBody, type GroupCreateOutcome } from './groupCreate.ts'
import { captureOwnerOnlyBody, type OwnerOnlyBody } from './groupOwnerOnly.ts'
import { relationNonceTags, relationSnapshot, relationSnapshotClear, relationMarkerResolved, type RelationLocalMarker } from './relationWrite.ts'
import { loadRelationMarker, storeRelationMarker, clearMatchedRelationMarker, type RelationMarkerStore } from './relationWriteStorage.ts'
import type { RelationAttemptResult } from './relationWriteFlow.ts'
interface Ports<T> { storage: RelationMarkerStore; current(): boolean; read(): Promise<{ intents: unknown; writes: unknown }>; post(body: T): Promise<unknown> }
export type CreatePorts = Ports<GroupCreateBody>
export type OwnerOnlyPorts = Ports<OwnerOnlyBody>
export type CreateAttemptResult = RelationAttemptResult & { create?: GroupCreateOutcome }
// Shared original lock/key and durable server journal across all relation
// operations. A property observation never proves a create WRITE terminal.
export function submitGroupCreateOnce(ports: CreatePorts, line: string, input: unknown): Promise<CreateAttemptResult> {
  return submitCaptured(ports, line, captureGroupCreateBody(input), 'create')
}
export function submitOwnerOnlyOnce(ports: OwnerOnlyPorts, line: string, input: unknown): Promise<CreateAttemptResult> {
  return submitCaptured(ports, line, captureOwnerOnlyBody(input), 'create_owner_only')
}
async function submitCaptured<T extends OwnerOnlyBody | GroupCreateBody>(ports: Ports<T>, line: string, body: T | null, operation: 'create' | 'create_owner_only'): Promise<CreateAttemptResult> {
  if (!body) return { status: 'blocked' }
  if (!ports.current()) return { status: 'stale' }
  if (loadRelationMarker(ports.storage, line) !== null) return { status: 'blocked' }
  const raw = await ports.read(); const before = relationSnapshot(raw.intents, raw.writes)
  if (!ports.current()) return { status: 'stale' }
  if (!before || !relationSnapshotClear(before) || before.intents.length >= 128 || before.writes.length >= 128) return { status: 'blocked' }
  const tags = await relationNonceTags(body.nonce)
  if (!ports.current()) return { status: 'stale' }
  if (loadRelationMarker(ports.storage, line) !== null || before.intents.some((r) => r.nonceTag === tags.intentTag) || before.writes.some((r) => r.nonceTag === tags.writeTag)) return { status: 'blocked' }
  const marker: RelationLocalMarker = { version: 1, ...tags, revision: body.expected_revision, afterIntent: Math.max(0, ...before.intents.map((r) => r.id)), afterWrite: Math.max(0, ...before.writes.map((r) => r.id)), target: 'group', additions: 0, updates: 0, removals: 0, management: { operation, properties: body.properties.length + 2 } }
  storeRelationMarker(ports.storage, line, marker)
  let create: GroupCreateOutcome | null = null
  try { create = groupCreateOutcome(await ports.post(body), operation) } catch { /* Ambiguity survives, no automatic replay. */ }
  if (!ports.current()) return { status: 'stale' }
  try {
    const raw = await ports.read(); const after = relationSnapshot(raw.intents, raw.writes)
    if (!ports.current()) return { status: 'stale' }
    if (after && relationMarkerResolved(marker, after)) {
      // The marker is retired by *history*. Only display the POST's separate
      // READ if its write projection is that very row, not a stale response.
      const write = after.writes.find((r) => r.nonceTag === marker.writeTag)
      const intent = after.intents.find((r) => r.nonceTag === marker.intentTag)
      const matched = create && write && intent?.state === 'bound' && intent.writeId === write.id && create.write.id === write.id && create.write.nonceTag === write.nonceTag && create.write.state === write.state && create.write.baseTime === write.baseTime && create.write.updateTime === write.updateTime && create.write.management?.properties === marker.management?.properties && (body.properties_read_consent ? create.read.state !== 'not_requested' : create.read.state === 'not_requested')
      clearMatchedRelationMarker(ports.storage, line, marker)
      return { status: 'observed_terminal', snapshot: after, ...(matched && create ? { create } : {}) }
    }
    return { status: 'unresolved', ...(after ? { snapshot: after } : {}) }
  } catch { return { status: 'unresolved' } }
}
