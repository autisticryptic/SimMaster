import type { EsimCommandResponse, EsimDownloadRequest, EsimEuiccInfo, EsimProfile } from '../api/contracts'
import { esimProfileActive } from './esimQuickSwitch.ts'

export const CONFIRM_DELETE_PROFILE = '确认删除'

export function commandSucceeded(response?: EsimCommandResponse) {
  return Boolean(response && response.code === 0 && ['', 'success', 'ok'].includes(response.status.toLowerCase()))
}

// Only the eUICC endpoint supplies manufacturer identity; never substitute the
// modem model/line id or print an unknown EID prefix as a manufacturer.
export function euiccManufacturer(euicc: EsimEuiccInfo | null) {
  const manufacturer = euicc?.manufacturer?.trim()
  if (!manufacturer || /^(unknown|n\/a|invalid|未知)/i.test(manufacturer) || /\d{8}/.test(manufacturer)) return '未知 eUICC 厂商'
  return manufacturer
}

export function remainingStorage(euicc: EsimEuiccInfo | null) {
  const available = euicc?.memory_available_kb
  return typeof available === 'number' && Number.isFinite(available) && available >= 0
    ? `${available.toLocaleString()} KB` : '未读取'
}

export function profileDeleteBlockReason(profile: EsimProfile) {
  if (esimProfileActive(profile)) return '已启用的配置不能删除，请先切换到其他配置'
  if (profile.delete_allowed === false) return '策略不允许删除'
  if (!profile.iccid || !['disabled', 'inactive', '0'].includes(profile.state.trim().toLowerCase())) return '配置状态未知，请刷新后重试'
  return null
}

export function profileStateLabel(profile: EsimProfile) {
  if (esimProfileActive(profile)) return '已启用'
  return ['disabled', 'inactive', '0'].includes(profile.state.trim().toLowerCase()) ? '已禁用' : '状态未知'
}

export function parseLpaCode(code: string): EsimDownloadRequest | null {
  const match = code.trim().match(/^LPA:1\$([^$\s]+)\$([^$\s]+)(?:\$([^$\s]*))?$/i)
  return match ? { smdp: match[1], matching_id: match[2], confirmation_code: match[3] || undefined } : null
}

export function validateDownload(form: EsimDownloadRequest): string | null {
  if (!form.smdp.trim() || !form.matching_id.trim()) return '请填写 SM-DP+ 服务器地址和 Matching ID'
  if (/\s|\$/.test(form.smdp.trim()) || /\s|\$/.test(form.matching_id.trim())) return '服务器地址和 Matching ID 不能包含空白或 $'
  if (form.imei?.trim() && !/^\d{15}$/.test(form.imei.trim())) return 'IMEI 必须为 15 位数字'
  return null
}

export function translateEsimError(rawError: string): string {
  const err = rawError.toLowerCase()
  if (err.includes('matchingid is refused') || err.includes('matching id was refused')) return '激活码已被使用或失效 (Matching ID was refused by SM-DP+ server)'
  if (err.includes('es9p_initiate_authentication')) return '无法启动身份认证，请检查设备联网情况或激活码是否有效'
  if (err.includes('es10b_load_bound_profile_package')) return '安全域装载失败，该配置可能已存在于当前芯片中，无法重复写入'
  if (err.includes('connect') || err.includes('timeout') || err.includes('resolve')) return '网络连接超时或无法解析服务器地址，请确保设备已正常联网后再试'
  if (err.includes('es10b_')) return `芯片交互阶段发生错误 (${rawError})，请确认卡片接触良好或芯片空间是否充足`
  if (err.includes('es9p_')) return `服务器通信阶段发生错误 (${rawError})，请检查网络状态与激活码是否正确`
  return rawError
}

export function extractDefaultSmdp(raw: unknown) {
  const paths = [
    ['EuiccConfiguredAddresses', 'defaultDpAddress'],
    ['euiccConfiguredAddresses', 'defaultDpAddress'],
    ['euicc_configured_addresses', 'default_dp_address'],
    ['defaultDpAddress'], ['defaultSmdpAddress'],
  ]
  for (const path of paths) {
    let value = raw
    for (const key of path) {
      value = value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>)[key] : null
    }
    if (typeof value === 'string' && value.trim()) return value.trim()
  }
  return ''
}

export function maskedEid(eid?: string | null) {
  const value = eid?.trim()
  if (!value) return '未读取'
  // Only expose endpoints for a complete EID; short/malformed identifiers must
  // never fall through to a slice that could reveal the whole identity.
  return /^\d{32}$/.test(value) ? `${value.slice(0, 6)}••••${value.slice(-4)}` : '••••'
}

export function redactEid(text: string, eid?: string) {
  const redacted = eid ? text.split(eid).join('EID 已隐藏') : text
  return redacted.replace(/\b\d{32}\b/g, 'EID 已隐藏')
}
