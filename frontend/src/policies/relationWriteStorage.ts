// Browser-only conservative exclusion; storage/locks are not server permission.
import { relationMarker, type RelationLocalMarker } from './relationWrite.ts'
export interface RelationMarkerStore { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem(key: string): void }
export const relationStorageKey = (line: string) => `simadmin:relation-pending:v1:${line}`
export function loadRelationMarker(storage: RelationMarkerStore, line: string): RelationLocalMarker | null {
  const value = storage.getItem(relationStorageKey(line)); if (value === null) return null
  if (value.length > 1024) throw new Error('Unverified relation marker')
  const parsed: unknown = JSON.parse(value); const marker = relationMarker(parsed); if (!marker) throw new Error('Unverified relation marker')
  return marker
}
// Must execute inside the same origin's exclusive Web Lock. Even an apparently
// stale/different-revision marker cannot be overwritten by a new intent.
export function storeRelationMarker(storage: RelationMarkerStore, line: string, marker: RelationLocalMarker): void {
  if (!relationMarker(marker) || loadRelationMarker(storage, line) !== null) throw new Error('Original relation intent unresolved')
  const value = JSON.stringify(marker); storage.setItem(relationStorageKey(line), value)
  if (storage.getItem(relationStorageKey(line)) !== value) throw new Error('Original relation intent persistence failed')
}
export function clearMatchedRelationMarker(storage: RelationMarkerStore, line: string, marker: RelationLocalMarker): void {
  const current = loadRelationMarker(storage, line)
  if (!current || JSON.stringify(current) !== JSON.stringify(marker)) throw new Error('Original relation marker changed')
  storage.removeItem(relationStorageKey(line))
  if (storage.getItem(relationStorageKey(line)) !== null) throw new Error('Original relation marker retirement failed')
}
