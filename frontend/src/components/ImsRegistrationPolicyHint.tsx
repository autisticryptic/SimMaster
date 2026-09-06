import { Tooltip, Typography } from '@mui/material'
import type { ImsSubsystemState } from '@/api/contracts'

/** A configured concurrent preference is not proof of two valid bindings. */
export default function ImsRegistrationPolicyHint({ ims }: { ims: ImsSubsystemState | null }) {
  if (!ims?.registration_policy) return null
  const policy = ims.registration_policy
  const bothEnabled = ims.three_gpp.configured && ims.non_three_gpp.configured
  if (!bothEnabled && !policy.switch_deferred_for_call) return null

  const selected = policy.applied.cellular_registers ? 'VoLTE' : policy.applied.wlan_registers ? 'VoWiFi' : null
  const label = policy.switch_deferred_for_call
    ? '有通话，等待切换'
    : policy.effective === 'single_registration'
      ? `单注册 · ${selected}`
      : policy.effective === 'concurrent' ? '已协商并行注册' : '等待接入协调'
  const capability = policy.concurrent_support === 'client_incomplete'
    ? '当前客户端尚未完整实现 SIP outbound 多流维护，不能仅凭不同 reg-id 安全并行注册。这不代表运营商一定不支持。'
    : policy.concurrent_support === 'not_negotiated'
      ? '尚未通过成功 REGISTER 的 Require: outbound 确认多流注册支持。'
      : '客户端与网络已确认多流注册支持。'
  const detail = `${capability} 两个启用开关保持不变；未选中的接入为待机，故障时需要建立新注册，并非无缝切换。${policy.switch_deferred_for_call ? '等待拨号、振铃或现有通话结束后再切换接入。' : ''} 原因：${policy.desired.code}`
  return (
    <Tooltip title={detail}>
      <Typography variant="caption" color="text.secondary" display="block" tabIndex={0}>
        {label}
      </Typography>
    </Tooltip>
  )
}
