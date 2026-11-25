// Original inbound Group.Update observations are partial DATA, never WRITE outcomes.
export type RelationContinuity = 'unseeded' | 'partial_chain' | 'gap' | 'stale'
export interface RelationEventRow {
  id: number; targetKind: 'buddy' | 'group'; updates: number; removals: number
  baseTime: string; updateTime: string; continuity: RelationContinuity
  fullRosterVerified: false; receivedAt: string
}
const fields = ['id', 'target_kind', 'updates', 'removals', 'base_time', 'update_time', 'continuity', 'full_roster_verified', 'received_at']
function signed64(value: unknown): value is string {
  return typeof value === 'string' && /^(?:0|-?[1-9][0-9]{0,18})$/.test(value) && BigInt(value) >= -9223372036854775808n && BigInt(value) <= 9223372036854775807n
}
function count(value: unknown): value is number { return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && value <= 128 }
function receivedAt(value: unknown): value is string {
  if (typeof value !== 'string' || value.length > 64) return false
  const match = /^(\d{4})-(\d{2})-(\d{2})[Tt](\d{2}):(\d{2}):(\d{2})(?:\.[0-9]+)?(?:[Zz]|([+-])(\d{2}):(\d{2}))$/.exec(value)
  if (!match) return false
  const year = Number(match[1]); const month = Number(match[2]); const day = Number(match[3])
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0)
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
  return month >= 1 && month <= 12 && day >= 1 && day <= days[month - 1]
    && Number(match[4]) <= 23 && Number(match[5]) <= 59 && Number(match[6]) <= 60
    && (match[7] === undefined || Number(match[8]) <= 23 && Number(match[9]) <= 59)
}
export function relationEventRows(value: unknown): RelationEventRow[] | null {
  if (!Array.isArray(value) || value.length > 128) return null
  const rows: RelationEventRow[] = []; const ids = new Set<number>()
  for (const raw of value) {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw) || Reflect.ownKeys(raw).some((key) => typeof key !== 'string' || !fields.includes(key)) || fields.some((key) => !Object.prototype.hasOwnProperty.call(raw, key))) return null
    const own = (key: string): unknown => (raw as Record<string, unknown>)[key]
    const id = own('id'); const targetKind = own('target_kind'); const updates = own('updates'); const removals = own('removals')
    const baseTime = own('base_time'); const updateTime = own('update_time'); const continuity = own('continuity'); const at = own('received_at')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || ids.has(id) || targetKind !== 'buddy' && targetKind !== 'group' || !count(updates) || !count(removals) || updates + removals > 128) return null
    if (!signed64(baseTime) || !signed64(updateTime) || BigInt(updateTime) <= BigInt(baseTime)) return null
    if (continuity !== 'unseeded' && continuity !== 'partial_chain' && continuity !== 'gap' && continuity !== 'stale' || own('full_roster_verified') !== false || !receivedAt(at)) return null
    ids.add(id); rows.push({ id, targetKind, updates, removals, baseTime, updateTime, continuity, fullRosterVerified: false, receivedAt: at })
  }
  return rows
}
export function relationEventLabel(continuity: RelationContinuity): string {
  switch (continuity) {
    case 'unseeded': return '未建立基线（unseeded）：只有局部入站观测'
    case 'partial_chain': return '局部连续（partial_chain）：仅已观测片段衔接，不是完整关系名册'
    case 'gap': return '存在缺口（gap）：不自动推进基线或清除缺口'
    case 'stale': return '旧事件（stale）：不作为新的当前状态'
  }
}
export const RELATION_EVENTS_SCOPE = '仅明确点击读取原入站 Group.Update 关系事件；这是已观测的局部 DATA，不是完整关系名册、当前关系状态、SDK/接收就绪或出站 WRITE 结果。full_roster_verified 固定为 false；不展示 UID、正文、token、nonce 或配置元数据。'
export const RELATION_EVENTS_UNKNOWN = '读取成功、读取失败或空列表均不解决 Unknown-WRITE（未知 WRITE），不能确认、否认或重放先前写入；不会自动查询、重试、重新登录、更新关系或生成 nonce。'
