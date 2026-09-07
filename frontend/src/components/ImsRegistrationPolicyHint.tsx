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
  const bothValidated = bothRegistered
    && policy.cellular_flow?.transport_validated
    && policy.wlan_flow?.transport_validated
  const label = policy.switch_deferred_for_call
    ? '有通话，等待切换'
    : policy.effective === 'single_registration'
      ? `单注册 · ${selected}`
      : policy.effective === 'concurrent'
        ? bothValidated ? '双注册 · VoWiFi + 4G/5G'
          : bothRegistered ? '双注册绑定 · 等待流验证' : '双注册协商通过 · 等待另一接入上线'
        : '等待接入协调'
  const capability = policy.concurrent_support === 'client_incomplete'
    ? '当前客户端尚未完整实现 SIP outbound 多流维护，不能仅凭不同 reg-id 安全并行注册。这不代表运营商一定不支持。'
    : policy.concurrent_support === 'not_negotiated'
      ? '尚无有效的多流协商结果；不能把未完成协商或超时当成运营商明确拒绝。'
      : policy.concurrent_support === 'not_supported'
        ? '当前注册已提供 outbound，但成功响应未接受多流注册，按优先级回退。这只描述当前接入，不是永久判定运营商不支持。'
        : '网络已接受 SIP outbound；新建第二路仍需验证传输流，两路已有绑定分别续期，不因短暂保活等待拆线。'
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
