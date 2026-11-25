export const BUDDY_CHECK_SCOPE = '仅检查明确原 UID 的 Buddy 关系类型；不解析电话或 URI，不修改关系、名册或消息位置，不证明在线、SDK 就绪或 WRITE 成功。'
export const BUDDY_CHECK_UNKNOWN = 'Unknown-WRITE 保持；检查结果不能确认、清除或重放任何关系修改。'
export interface BuddyCheckBody { expected_revision: number; target_uid: string; consent: true }
export interface BuddyCheckResult { accepted: boolean; relationType: number }
export function buddyCheckBody(revision: number, uid: string, consent: boolean): BuddyCheckBody | null {
  if (!Number.isSafeInteger(revision) || revision < 1 || !consent || uid.length < 3 || uid.length > 512 || /^[0-9]+_[1-9][0-9]*$/.exec(uid)?.[0] !== uid) return null
  return { expected_revision: revision, target_uid: uid, consent: true }
}
export function buddyCheckResult(value: unknown): BuddyCheckResult | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const v = value as Record<string, unknown>; const keys = Reflect.ownKeys(v)
  if (keys.length !== 2 || !keys.includes('accepted') || !keys.includes('relation_type') || typeof v.accepted !== 'boolean' || typeof v.relation_type !== 'number' || !Number.isSafeInteger(v.relation_type) || v.relation_type < 0 || v.relation_type > 0xffffffff) return null
  return { accepted: v.accepted, relationType: v.relation_type }
}
