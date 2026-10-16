import { useEffect, useRef, useState } from 'react'
import { Alert, Box, Button, Typography } from '@mui/material'
import Grid from '@mui/material/Grid'
import { api, type CellularImsLineControlResponse, type NativeImsObservationStatus } from '../../api/current'
import { nativeImsErrorMessage, nativeImsObservationRows, parseNativeImsObservation } from './imsDiagnosticsFormat'

export function NativeImsObservationDetails({ line }: { line: CellularImsLineControlResponse }) {
  const modem = line.modem
  if (!modem.modem_path?.startsWith('native:')) return null
  // Remount on any visible binding/admission change, not on routine inventory refresh.
  // The key is internal only: never display SIM identities or modem identifiers here.
  const context = JSON.stringify([modem.line_id, modem.modem_id, modem.modem_path, modem.primary_port,
    modem.qmi_device, modem.uim_slot, modem.sim_path, modem.sim_iccid, modem.present,
    line.read_only, line.profile.enabled, line.profile.airplane_mode_enabled,
    line.profile.cellular_ims_connection_enabled, line.profile.vowifi.enabled])
  return <Observation key={context} line={line} />
}

function Observation({ line }: { line: CellularImsLineControlResponse }) {
  const [status, setStatus] = useState<NativeImsObservationStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [pending, setPending] = useState(false)
  const sequence = useRef(0)
  const inFlight = useRef(false)
  useEffect(() => () => { sequence.current += 1 }, [])

  const blocked = line.read_only ? '线路为只读展示，禁止采样。'
    : !line.modem.present ? '设备未连接，禁止采样。'
      : !line.profile.enabled || line.profile.airplane_mode_enabled ? '线路已禁用或启用飞行模式，禁止采样。'
        : line.profile.cellular_ims_connection_enabled || line.profile.vowifi.enabled
        ? '主机蜂窝 IMS 或 VoWiFi 连接意图已启用，禁止采样。'
        : null

  async function readObservation(probe: boolean) {
    if (inFlight.current || (probe && blocked)) return
    inFlight.current = true
    const token = ++sequence.current
    setPending(true)
    setError(null)
    try {
      const response = await (probe ? api.probeNativeImsObservation(line.modem.line_id)
        : api.getNativeImsObservationStatus(line.modem.line_id))
      if (sequence.current !== token) return
      const parsed = parseNativeImsObservation(response.data, line.modem.line_id)
      if (!parsed) {
        setStatus(null)
        setError(nativeImsErrorMessage('native_ims_malformed_response'))
      } else {
        setStatus(parsed)
        setError(nativeImsErrorMessage(parsed.last_error))
      }
    } catch (cause) {
      if (sequence.current !== token) return
      setStatus(null)
      setError(nativeImsErrorMessage(cause instanceof Error ? cause.message : 'unknown'))
    } finally {
      if (sequence.current === token) {
        inFlight.current = false
        setPending(false)
      }
    }
  }

  return (
    <Box component="details" data-testid="native-ims-observation" sx={{ border: 1, borderColor: 'divider', borderRadius: 1, p: 1.5 }}>
      <Box component="summary" sx={{ cursor: 'pointer', fontWeight: 600 }}>基带原生 IMS 观测（历史样本，非主机就绪）</Box>
      <Typography variant="caption" color="text.secondary" component="p" sx={{ my: 1 }}>
        仅点击“只读采样一次”才查询设备；不自动探测或轮询。查询按顺序执行，并非原子快照。
        不修改 NV、URC 或连接意图。以下均为最近样本，不代表当前主机 IMS 已注册、业务路由或资费状态。
        0 表示未知或未报告，不能证明未注册或服务不可用；UT 是补充业务，不是视频。
        报告标志仅为返回值，不证明持久 NV 读取成功。服务器仍会核验固件资格与实时 owner，可能拒绝采样。
      </Typography>
      <Box display="flex" gap={1} flexWrap="wrap" sx={{ mb: 1.5 }}>
        <Button size="small" variant="outlined" disabled={pending || Boolean(blocked)} onClick={() => void readObservation(true)}>只读采样一次</Button>
        <Button size="small" disabled={pending} onClick={() => void readObservation(false)}>读取缓存（不访问设备）</Button>
      </Box>
      {blocked && <Typography variant="caption" color="text.secondary" component="p">{blocked}</Typography>}
      {pending && <Typography variant="body2" role="status">正在读取观测结果…</Typography>}
      {error && <Alert severity="warning" sx={{ mb: 1 }}>{error}</Alert>}
      {!status && !pending && <Typography variant="body2" color="text.secondary">尚未读取本线路的观测记录。</Typography>}
      {status && <Grid container spacing={1.5}>
        {nativeImsObservationRows(status).map(({ label, value }) => (
          <Grid key={label} size={{ xs: 12, sm: 6 }}>
            <Typography variant="caption" color="text.secondary">{label}</Typography>
            <Typography variant="body2">{value}</Typography>
          </Grid>
        ))}
      </Grid>}
    </Box>
  )
}
