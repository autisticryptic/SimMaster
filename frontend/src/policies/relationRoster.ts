// Historical snapshots are DATA, not current auth, WRITE outcomes or IO grants.
export interface RelationRosterRow { id: number; targetKind: 'buddy' | 'group'; baseline: string | null; observed: string | null; target: string | null; historicalComplete: boolean; gap: boolean; fullRequired: boolean; members: number }
export interface RosterReadBody { expected_revision: number; operation: 'relations'; group_id: string | null; since: string; consent: boolean }
export interface RosterReadOutcome { outcome: 'rejected' | 'superseded' | 'stale' | 'baseline_required' | 'incremental' | 'snapshot'; completeRoster: boolean; originalTime: string | null; members: number }
function signed64(v: unknown): v is string { return typeof v === 'string' && /^(?:0|-?[1-9][0-9]{0,18})$/.test(v) && BigInt(v) >= -9223372036854775808n && BigInt(v) <= 9223372036854775807n }
function count(v: unknown): v is number { return typeof v === 'number' && Number.isSafeInteger(v) && v >= 0 && v <= 1024 }
function exact(v: unknown, fields: string[]): v is Record<string, unknown> { return !!v && typeof v === 'object' && !Array.isArray(v) && Reflect.ownKeys(v).every((key) => typeof key === 'string' && fields.includes(key)) && fields.every((key) => Object.prototype.hasOwnProperty.call(v, key)) }
export function relationRosterRows(value: unknown): RelationRosterRow[] | null {
  if (!Array.isArray(value) || value.length > 128) return null
  const rows: RelationRosterRow[] = []; const ids = new Set<number>()
  for (const raw of value) {
    if (!exact(raw, ['id', 'target_kind', 'baseline_time', 'observed_time', 'target_time', 'historical_complete', 'gap', 'full_required', 'members', 'updated_at'])) return null
    const id = raw.id; const kind = raw.target_kind; const base = raw.baseline_time; const observed = raw.observed_time; const target = raw.target_time; const complete = raw.historical_complete; const gap = raw.gap; const full = raw.full_required; const members = raw.members; const at = raw.updated_at
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || ids.has(id) || kind !== 'buddy' && kind !== 'group' || !count(members)) return null
    if (base !== null && !signed64(base) || observed !== null && !signed64(observed) || target !== null && !signed64(target) || typeof complete !== 'boolean' || typeof gap !== 'boolean' || typeof full !== 'boolean') return null
    if (complete && (gap || full || base === null || observed === null || target === null || target !== observed) || full && !gap || observed !== null && target !== null && BigInt(target) < BigInt(observed)) return null
    if (typeof at !== 'string' || at.length > 64 || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.[0-9]{1,9})?(?:Z|[+-]\d{2}:\d{2})$/.test(at)) return null
    ids.add(id); rows.push({ id, targetKind: kind, baseline: base, observed, target, historicalComplete: complete, gap, fullRequired: full, members })
  }
  return rows
}
export function rosterReadBody(revision: number, group: string, since: string, consent: boolean): RosterReadBody | null {
  if (!consent || !Number.isSafeInteger(revision) || revision <= 0 || group.length > 508 || group !== '' && !/^[1-9][0-9]*[-+][1-9][0-9]*$/.test(group) || !signed64(since) || BigInt(since) < -1n) return null
  return { expected_revision: revision, operation: 'relations', group_id: group === '' ? null : group, since, consent: true }
}
export function rosterReadOutcome(value: unknown): RosterReadOutcome | null {
  if (!exact(value, ['outcome', 'complete_roster', 'original_time', 'members'])) return null
  const outcome = value.outcome; const complete = value.complete_roster; const time = value.original_time; const members = value.members
  if (outcome !== 'rejected' && outcome !== 'superseded' && outcome !== 'stale' && outcome !== 'baseline_required' && outcome !== 'incremental' && outcome !== 'snapshot' || typeof complete !== 'boolean' || time !== null && !signed64(time) || !count(members)) return null
  if (complete !== (outcome === 'incremental' || outcome === 'snapshot') || complete && time === null || !complete && members !== 0) return null
  return { outcome, completeRoster: complete, originalTime: time, members }
}
export const ROSTER_SCOPE = '名册是原 getRelations2 的本地历史快照，不是当前 SDK 就绪、关系 WRITE 权限或送达证明。IsPartialUpdate=false 才是完整 RelationList；增量必须匹配原 since/基线。事件竞态会拒绝旧读取结果，不自动重试。'
export const ROSTER_UNKNOWN = '此处只允许明确目录 READ；不生成 nonce、不发送关系修改、不确认或清除 Unknown-WRITE。发现 gap 或完整快照必需时由用户另行明确读取，不自动查询。'
