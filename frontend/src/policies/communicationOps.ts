import type { JegotripCommunicationAction, JegotripCommunicationOperation, JegotripCommunicationObservation } from '../api/current'

const actions: Record<JegotripCommunicationAction, string> = {
  query_profile: '查询通话配置', query_phone_service: '查询电话服务', query_display_status: '查询来显配置', query_forwarding_state: '查询呼转业务状态', set_phone_service: '设置电话服务原状态码',
  set_voip_profile: '设置远端呼转配置', close_forward: '关闭远端呼转', set_jego_m_status: '设置JegoM显示状态', logout: '退出App会话（不等于停止SDK）',
}
export function isCommunicationAction(action: unknown): action is JegotripCommunicationAction {
  return typeof action === 'string' && Object.prototype.hasOwnProperty.call(actions, action)
}
export function communicationActionLabel(action: unknown): string {
  return typeof action === 'string' && Object.prototype.hasOwnProperty.call(actions, action) ? actions[action as JegotripCommunicationAction] : '未知通信操作'
}
export function communicationWrites(action: unknown): boolean {
  return typeof action !== 'string' || !['query_profile', 'query_phone_service', 'query_display_status', 'query_forwarding_state'].includes(action)
}
export function validCommunicationOperation(operation: JegotripCommunicationOperation | undefined): boolean {
  if (!operation || !Object.prototype.hasOwnProperty.call(actions, operation.kind)) return false
  if (operation.kind === 'set_phone_service') return Number.isInteger(operation.status) && operation.status >= 0 && operation.status <= 999999
  if (operation.kind === 'set_voip_profile') return operation.cs_forward === '1' || operation.cs_forward === '2'
  if (operation.kind === 'set_jego_m_status') {
    const region = (v: string) => /^(?:\d{1,6}|[A-Z]{1,3})$/.test(v)
    return (operation.jegom_status === '1' || operation.jegom_status === '2') && region(operation.country_code) && (operation.region_code === '' || region(operation.region_code))
  }
  return true
}
export function communicationObservationLabel(value: unknown): string {
  const observation = typeof value === 'object' && value !== null ? value as Partial<JegotripCommunicationObservation> : {}
  const labels: Record<string, string> = { not_requested: '未请求', request_uncertain: '请求结果不确定，不自动重发', accepted_unconfirmed: '服务已接受，状态未确认', acknowledged: '原接口确认已返回，不推断托管开关', observed: '本次查询已返回', confirmed: '本次操作已确认', rejected: '服务拒绝本次请求' }
  const state = typeof observation.state === 'string' && Object.prototype.hasOwnProperty.call(labels, observation.state) ? labels[observation.state] : '状态未知'
  return `${communicationActionLabel(observation.action)}：${state}；不代表设备授信、SDK或收件成功`
}
export function communicationStatusLines(value: unknown): string[] {
  const observation = typeof value === 'object' && value !== null ? value as Partial<JegotripCommunicationObservation> : {}
  const status = observation.status
  if (!status || typeof status !== 'object') return []
  const lines: string[] = []
  for (const [name, number] of [['电话服务原状态', status.phone_server_status], ['cs_forward原状态', status.cs_forward], ['jegom_status原状态', status.jegom_status]] as const) {
    if (typeof number === 'number' && Number.isInteger(number) && number >= 0 && number <= 999999) lines.push(`${name}：${number}`)
  }
  if (typeof status.masked_jegom_phone === 'string' && /^\*\*\*[0-9+]{0,4}$/.test(status.masked_jegom_phone)) lines.push(`来显号码：${status.masked_jegom_phone}`)
  if (typeof status.region_code === 'string' && /^(?:\d{1,6}|[A-Z]{1,3})$/.test(status.region_code)) lines.push(`region_code：${status.region_code}`)
  if (status.profile && typeof status.profile === 'object') {
    for (const key of ['allow_mo_voice', 'allow_mt_voice', 'mo_voice_limit', 'mt_voice_limit'] as const) {
      const number = status.profile[key]
      if (typeof number === 'number' && Number.isInteger(number) && number >= -2147483648 && number <= 2147483647) lines.push(`${key}：${number}（原配置观察，不授权外呼或接听）`)
    }
    for (const [key, label] of [['forbidden_countries', '外呼限制国家'], ['answer_forbidden_countries', '接听限制国家']] as const) {
      const rows = status.profile[key]
      if (Array.isArray(rows) && rows.length <= 256) {
        const codes = rows.filter((row) => row && typeof row.country_code === 'string' && /^(?:\d{1,6}|[A-Z]{1,3})$/.test(row.country_code)).map((row) => row.country_code)
        lines.push(`${label}：${codes.join('、') || '未返回国家条目'}（不据此授予媒体权限）`)
      }
    }
  }
  return lines
}
export function newCommunicationOperation(kind: JegotripCommunicationAction): JegotripCommunicationOperation {
  if (kind === 'set_phone_service') return { kind, status: NaN }
  if (kind === 'set_voip_profile') return { kind, cs_forward: '' }
  if (kind === 'set_jego_m_status') return { kind, jegom_status: '', region_code: '', country_code: '' }
  return { kind }
}
