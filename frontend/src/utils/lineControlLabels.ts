import type { LineNetworkControlsResponse } from '../api/contracts'

export function airplaneControlLabel(
  network: Pick<LineNetworkControlsResponse, 'airplane_mode_requested' | 'airplane_mode_observed' | 'airplane_error' | 'radio_state'> | undefined,
  present: boolean,
  busy: boolean,
  loading: boolean,
): string {
  if (busy) return '切换中'
  if (!present) return '设备离线'
  if (loading) return '读取中'
  if (!network) return '状态未知'
  if (network.airplane_error) return '切换失败'
  if (network.radio_state === 'turning_on' || network.radio_state === 'turning_off') return '切换中'
  const observed = network.airplane_mode_observed
  if (observed !== true && observed !== false) return '状态未知'
  if (observed !== network.airplane_mode_requested) return '待生效'
  return observed ? '已开启' : '已关闭'
}
