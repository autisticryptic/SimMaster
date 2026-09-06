import { Tooltip, Typography } from '@mui/material'
import type { ImsSubsystemState } from '@/api/contracts'

/** A configured concurrent preference is not proof of two valid bindings. */
export default function ImsRegistrationPolicyHint({ ims }: { ims: ImsSubsystemState | null }) {
  if (!ims?.registration_policy) return null
  const policy = ims.registration_policy
  const bothEnabled = ims.three_gpp.configured && ims.non_three_gpp.configured
  if (!bothEnabled && !policy.switch_deferred_for_call) return null

  const selected = policy.applied.wlan_registers ? 'VoWiFi' : policy.applied.cellular_registers ? '4G/5G' : null
  const bothRegistered = ims.three_gpp.registered && ims.non_three_gpp.registered
  const label = policy.switch_deferred_for_call
    ? '有通话，等待切换'
    : policy.effective === 'single_registration'
      ? `单注册 · ${selected}`
      : policy.effective === 'concurrent'
        ? bothRegistered ? '双注册 · VoWiFi + 4G/5G' : '双注册协商通过 · 等待另一接入上线'
        : '等待接入协调'
  const capability = policy.concurrent_support === 'client_incomplete'
    ? '当前客户端尚未完整实现 SIP outbound 多流维护，不能仅凭不同 reg-id 安全并行注册。这不代表运营商一定不支持。'
    : policy.concurrent_support === 'not_negotiated'
      ? '客户端已实现 SIP outbound，等待网络确认注册绑定和流保活；尚不能确认双注册在线。'
      : '已确认 SIP outbound 注册绑定和流保活；两路分别维护注册租期。'
  const modeDetail = policy.effective === 'concurrent'
    ? '双注册不改变业务选路优先级，也不会迁移已建立的通话。'
    : '两个启用开关保持不变；尚未并行注册的接入不算已在线。'
  const detail = `${capability} ${modeDetail}${policy.switch_deferred_for_call ? '等待拨号、振铃或现有通话结束后再切换接入。' : ''} 原因：${policy.desired.code}`
  return (
    <Tooltip title={detail}>
      <Typography variant="caption" color="text.secondary" display="block" tabIndex={0}>
        {label}
      </Typography>
    </Tooltip>
  )
}
