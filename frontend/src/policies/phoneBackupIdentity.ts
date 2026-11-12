import { validPhoneAppUuid } from './appLogin.ts'

export const PHONE_BACKUP_MAX_BYTES = 256 * 1024

// 固定错误码不含文件内容、路径、账号或 token；只解析本地原始材料。
export function phoneBackupError(error: unknown): string {
  const code = error instanceof Error ? error.message : ''
  const labels: Record<string, string> = {
    jegotrip_backup_protected_container: '这是 SBA1 / v1::: 备份封装，不是明文 App 配置。请用现有备份工具导出解密后的原始数据；不会猜密钥或申请新授信。',
    jegotrip_backup_archive_requires_local_reader: 'ZIP 请先用项目的离线备份读取脚本生成 UUID-only JSON，或解压普通 root ZIP 后选择 setting_power.xml / 可读的 device.dat。整个备份不会上传。',
    jegotrip_backup_identity_file_too_large: 'UUID 来源文件超过 256 KiB，未读取或上传。',
    jegotrip_backup_uuid_missing: '所选原始配置没有 phone_uuid。SDK UUID 和本地授信缓存不能代替它。',
    jegotrip_backup_uuid_conflict: '原始配置包含重复或冲突的 phone_uuid，未选择、覆盖或生成新 UUID。',
    jegotrip_backup_uuid_invalid: '来源不是原样的 36 位 App UUID；不会把 IMEI、MD5、token 或编码数据当作 UUID。',
    jegotrip_backup_xml_invalid: '请选择原始 setting_power.xml：须为普通 SharedPreferences map，不接受 DTD、实体声明或损坏 XML。',
    jegotrip_backup_export_invalid: 'JSON 只接受离线脚本的 version: 1、source: android_root_export、app_device_uuid 三个字段；不接受密码、token 或“已授信”字段。',
    jegotrip_backup_format_unsupported: '请选择原始 setting_power.xml、可读的 device.dat，或离线脚本生成的 UUID-only JSON。',
    jegotrip_backup_read_failed: '无法读取所选本地文件；没有上传备份或修改标识。',
  }
  return labels[code] ?? '本地备份格式未确认，未上传或修改任何标识。'
}

function originalUuid(value: unknown): string {
  if (typeof value !== 'string' || !validPhoneAppUuid(value)) throw new Error('jegotrip_backup_uuid_invalid')
  return value.trim() // 保留大小写和连字符，不生成或替换为 SDK 标识。
}

// 只返回 App UUID；历史授信状态、业务缓存及任何会话材料都不进入 API。
export function phoneUuidFromBackupText(text: string, filename: string): string {
  if (new TextEncoder().encode(text).length > PHONE_BACKUP_MAX_BYTES) throw new Error('jegotrip_backup_identity_file_too_large')
  const value = text.replace(/^\uFEFF/, '').trim()
  if (value.startsWith('SBA1') || value.startsWith('v1:::')) throw new Error('jegotrip_backup_protected_container')
  const name = filename.toLowerCase()
  if (name.endsWith('.zip') || value.startsWith('PK\u0003\u0004')) throw new Error('jegotrip_backup_archive_requires_local_reader')
  if (name === 'device.dat') return originalUuid(value)
  if (name.endsWith('.json')) {
    let parsed: unknown
    try { parsed = JSON.parse(value) } catch { throw new Error('jegotrip_backup_export_invalid') }
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error('jegotrip_backup_export_invalid')
    const record = parsed as Record<string, unknown>
    const keys = Object.keys(record).sort()
    // Export values are scalar constants/UUIDs. Count lexical object keys too:
    // JSON.parse alone would silently accept duplicate credential-like fields.
    const lexicalKeys = value.match(/"(?:\\.|[^"\\])*"\s*:/g) ?? []
    if (keys.join(',') !== 'app_device_uuid,source,version' || lexicalKeys.length !== 3
      || record.version !== 1 || record.source !== 'android_root_export') throw new Error('jegotrip_backup_export_invalid')
    return originalUuid(record.app_device_uuid)
  }
  if (name === 'setting_power.xml') {
    if (/<!\s*(?:DOCTYPE|ENTITY)/i.test(value) || typeof DOMParser === 'undefined') throw new Error('jegotrip_backup_xml_invalid')
    const document = new DOMParser().parseFromString(value, 'application/xml')
    if (document.querySelector('parsererror') || document.documentElement.tagName !== 'map'
      || document.documentElement.namespaceURI !== null) throw new Error('jegotrip_backup_xml_invalid')
    const matches = Array.from(document.documentElement.children).filter((element) => element.getAttribute('name') === 'phone_uuid')
    if (!matches.length) throw new Error('jegotrip_backup_uuid_missing')
    if (matches.length !== 1 || matches[0].tagName !== 'string' || matches[0].children.length) throw new Error('jegotrip_backup_uuid_conflict')
    return originalUuid(matches[0].textContent)
  }
  throw new Error('jegotrip_backup_format_unsupported')
}
