// Persisted relation outcomes are DATA, never current SDK/WRITE permission.
export type RelationState = 'uncertain' | 'confirmed' | 'rejected' | 'not_sent'
export interface RelationHistoryRow { id: number; nonceTag: string; targetKind: 'buddy' | 'group'; state: RelationState; additions: number; updates: number; removals: number; baseTime: string | null; updateTime: string | null }
const fields = ['id', 'nonce_tag', 'target_kind', 'state', 'additions', 'updates', 'removals', 'base_time', 'update_time', 'updated_at']
function signed64(value: unknown): value is string { return typeof value === 'string' && /^(?:0|-?[1-9][0-9]{0,18})$/.test(value) && BigInt(value) >= -9223372036854775808n && BigInt(value) <= 9223372036854775807n }
function count(value: unknown): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value <= 128 }
export function relationHistoryRows(value: unknown): RelationHistoryRow[] | null {
  if (!Array.isArray(value) || value.length > 128) return null
  const rows: RelationHistoryRow[] = []; const ids = new Set<number>(); const tags = new Set<string>()
  for (const raw of value) {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw) || Object.keys(raw).some((key) => !fields.includes(key)) || fields.some((key) => !Object.prototype.hasOwnProperty.call(raw, key))) return null
    const own = (key: string): unknown => (raw as Record<string, unknown>)[key]
    const id = own('id'); const nonceTag = own('nonce_tag'); const targetKind = own('target_kind'); const state = own('state'); const additions = own('additions'); const updates = own('updates'); const removals = own('removals'); const baseTime = own('base_time'); const updateTime = own('update_time'); const at = own('updated_at')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || ids.has(id) || typeof nonceTag !== 'string' || !/^[0-9a-f]{64}$/.test(nonceTag) || tags.has(nonceTag) || targetKind !== 'buddy' && targetKind !== 'group' || state !== 'uncertain' && state !== 'confirmed' && state !== 'rejected' && state !== 'not_sent' || !count(additions) || !count(updates) || !count(removals) || additions + updates + removals === 0) return null
    if (typeof at !== 'string' || at.length > 64 || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.[0-9]{1,9})?(?:Z|[+-]\d{2}:\d{2})$/.test(at)) return null
    if (state === 'confirmed' || state === 'rejected') { if (!signed64(baseTime) || !signed64(updateTime)) return null } else if (baseTime !== null || updateTime !== null) return null
    ids.add(id); tags.add(nonceTag); rows.push({ id, nonceTag, targetKind, state, additions, updates, removals, baseTime: baseTime as string | null, updateTime: updateTime as string | null })
  }
  return rows
}
export function relationHistoryLabel(state: RelationState): string { switch (state) { case 'uncertain': return '关系修改结果未知；仅查询，不重放'; case 'confirmed': return '原关系修改操作已确认（不是在线或送达证明）'; case 'rejected': return '原服务端拒绝关系修改'; case 'not_sent': return '本次关系修改未写出' } }
export const RELATION_HISTORY_SCOPE = '仅查询既有 Group/Buddy 修改记录；不授予 SDK、目录 READ 或关系 WRITE，不生成新 nonce，不解决或重放未知 WRITE。'
