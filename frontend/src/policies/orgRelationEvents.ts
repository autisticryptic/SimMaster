// Original system-control consumption, not chat text, complete roster or WRITE result.
export type OrgRelationContinuity = 'unseeded' | 'partial_chain' | 'gap' | 'stale' | 'unsequenced'
export interface OrgRelationRow { id: number; messageIndex: string; baseTime: string | null; updateTime: string | null; removal: boolean; orgDeleted: boolean; relationSupplied: boolean; continuity: OrgRelationContinuity; fullRosterVerified: false }
const fields = ['id', 'message_index', 'base_time', 'update_time', 'removal', 'org_deleted', 'relation_supplied', 'continuity', 'full_roster_verified', 'received_at']
function signed64(v: unknown): v is string { return typeof v === 'string' && /^(?:0|-?[1-9][0-9]{0,18})$/.test(v) && BigInt(v) >= -9223372036854775808n && BigInt(v) <= 9223372036854775807n }
export function orgRelationRows(value: unknown): OrgRelationRow[] | null {
  if (!Array.isArray(value) || value.length > 128) return null
  const rows: OrgRelationRow[] = []; const ids = new Set<number>()
  for (const raw of value) {
    if (!raw || typeof raw !== 'object' || Array.isArray(raw) || Reflect.ownKeys(raw).some((k) => typeof k !== 'string' || !fields.includes(k)) || fields.some((k) => !Object.prototype.hasOwnProperty.call(raw, k))) return null
    const own = (k: string): unknown => (raw as Record<string, unknown>)[k]
    const id = own('id'); const messageIndex = own('message_index'); const baseTime = own('base_time'); const updateTime = own('update_time'); const removal = own('removal'); const orgDeleted = own('org_deleted'); const relationSupplied = own('relation_supplied'); const continuity = own('continuity'); const at = own('received_at')
    if (typeof id !== 'number' || !Number.isSafeInteger(id) || id <= 0 || ids.has(id) || !signed64(messageIndex) || baseTime !== null && !signed64(baseTime) || updateTime !== null && !signed64(updateTime)) return null
    if (typeof removal !== 'boolean' || typeof orgDeleted !== 'boolean' || typeof relationSupplied !== 'boolean' || own('full_roster_verified') !== false || continuity !== 'unseeded' && continuity !== 'partial_chain' && continuity !== 'gap' && continuity !== 'stale' && continuity !== 'unsequenced') return null
    if (typeof at !== 'string' || at.length > 64 || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.[0-9]{1,9})?(?:Z|[+-]\d{2}:\d{2})$/.test(at)) return null
    if (!relationSupplied && !removal) return null
    ids.add(id); rows.push({ id, messageIndex, baseTime, updateTime, removal, orgDeleted, relationSupplied, continuity, fullRosterVerified: false })
  }
  return rows
}
export const ORG_RELATION_SCOPE = 'System.OrgRelSync 是系统 IM 参数中的独立控制记录，不是 Group.Update RelationMap 或聊天正文。IM 索引与关系时间互不替代；仅保存已观测的局部状态，不证明完整关系名册。'
export const ORG_RELATION_UNKNOWN = '读取成功、失败或空列表均不确认或清除 Unknown-WRITE；不重放写入、不生成 nonce、不自动查询或重新登录。OrgDel 是原服务端数据，不是本地删除账号的授权。'
