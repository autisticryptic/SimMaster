import type { UserReadRow } from './imUserRead'
// UI observations only: never native/write permission or a replay instruction.
export interface UserReadSnapshot { line: string; generation: number; context: string; rows: UserReadRow[] }
export interface UserReadUnknown { line: string; id: number; index: string; after: number }
export function userReadBaselineReady(snapshot: UserReadSnapshot | null, line: string, generation: number): snapshot is UserReadSnapshot {
  return snapshot?.line === line && snapshot.generation === generation
}
export function userReadKnownUnknown(snapshot: UserReadSnapshot | null, line: string, generation: number, messageId: number, index: string): boolean {
  return userReadBaselineReady(snapshot, line, generation) && snapshot.rows.some((row) => row.messageId === messageId && row.index === index && row.state === 'uncertain')
}
export function userReadUnknownResolved(unknown: UserReadUnknown, snapshot: UserReadSnapshot, line: string, generation: number): boolean {
  if (unknown.line !== line || !userReadBaselineReady(snapshot, line, generation) || userReadKnownUnknown(snapshot, line, generation, unknown.id, unknown.index)) return false
  return snapshot.rows.some((row) => row.id > unknown.after && row.messageId === unknown.id && row.index === unknown.index && row.state !== 'uncertain')
}
