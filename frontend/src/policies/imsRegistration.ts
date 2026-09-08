import type { ImsAccessPreference, ImsRegistrationPolicyStatus } from '../api/contracts'

export type SelectableRegistrationPreference = 'concurrent' | 'wlan_preferred'

export const registrationModeOptions = [
  { value: 'concurrent', label: '自动双注册（默认）' },
  { value: 'wlan_preferred', label: '仅单注册（VoWiFi 优先）' },
] as const

export function isSelectableRegistrationPreference(value: unknown): value is SelectableRegistrationPreference {
  return value === 'concurrent' || value === 'wlan_preferred'
}

export function registrationPreferenceLabel(preference: ImsAccessPreference) {
  return registrationModeOptions.find((item) => item.value === preference)?.label
    ?? '旧设置：单注册、蜂窝优先'
}

export function registrationSupportText(
  policy: Pick<ImsRegistrationPolicyStatus, 'requested' | 'concurrent_support' | 'multiple_registration_blocked'>,
) {
  if (policy.requested === 'wlan_preferred') {
    return '已选择仅单注册：优先 VoWiFi，不可用时再考虑已启用的 4G/5G IMS，不追加第二路注册。'
  }
  if (policy.requested === 'cellular_preferred') {
    return '保留了旧版的蜂窝优先单注册设置；不会在打开页面时自动改写。'
  }
  if (policy.multiple_registration_blocked) {
    return '额外注册流被拒绝或协商校验未通过，暂按单注册优先 VoWiFi 回退；已有有效通道仍可续期，不永久拉黑运营商。'
  }
  switch (policy.concurrent_support) {
    case 'client_incomplete':
      return '当前客户端尚未具备完整的多流维护能力，暂按单注册运行。'
    case 'not_negotiated':
      return '尚未取得有效的网络协商结果，先保留单路注册；不能把等待或超时等同于运营商明确拒绝。'
    case 'not_supported':
      return '已请求多流能力，但当前网络的成功响应未接受，已按优先级回退单注册；这不是永久判定运营商不支持。'
    case 'negotiated':
      return '网络已接受多流机制；新建第二路仍需验证通道，两路有效绑定分别续期。'
  }
}

export function humanizeCostPolicyError(message: string) {
  if (message.includes('sms_vowifi_only_required')) {
    return '仅通过 VoWiFi 发送短信的限制已生效。本次 VoWiFi 发送未成功，未回退到 4G/5G IMS 或 CS。'
  }
  if (message.includes('voice_vowifi_only_required')) {
    return '仅通过 VoWiFi 通话的限制已生效。当前不能通过 VoWiFi 呼出，已阻止蜂窝或 CS 回退。'
  }
  return message
}
