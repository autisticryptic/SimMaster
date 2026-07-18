import { useCallback, useEffect, useState } from 'react'
import {
  Alert,
  Box,
  Card,
  CardContent,
  CardHeader,
  Chip,
  CircularProgress,
  IconButton,
  Stack,
  Tooltip,
  Typography,
} from '@mui/material'
import Grid from '@mui/material/Grid'
import { Refresh, SettingsEthernet } from '@mui/icons-material'
import { api, type TrunkProfileResponse } from '../../api/current'
import { modemSlotLabel, modemSlotSourceLabel, shortLineId, stableModemSort } from '../../components/modemLineFormat'

function timestamp(value?: string) {
  return value ? new Date(value).toLocaleString() : '无'
}

function bytes(value: number) {
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`
  return `${(value / 1024 / 1024).toFixed(1)} MiB`
}

function phaseLabel(line: TrunkProfileResponse) {
  if (!line.trunk.enabled) return '未启用'
  if (line.runtime.registered) return '已注册'
  if (line.runtime.phase === 'ready') return '静态 Peer 已就绪'
  if (line.runtime.phase === 'degraded') return '连接异常'
  if (line.runtime.phase === 'starting') return '正在连接'
  return line.runtime.phase
}

function Metric({ label, value }: { label: string, value: string | number }) {
  return (
    <Box minWidth={0}>
      <Typography variant="caption" color="text.secondary">{label}</Typography>
      <Typography variant="body2" sx={{ wordBreak: 'break-word' }}>{value}</Typography>
    </Box>
  )
}

export default function TrunkDiagnosticsPanel() {
  const [lines, setLines] = useState<TrunkProfileResponse[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async (background = false) => {
    if (!background) setLoading(true)
    try {
      const response = await api.getTrunkLines()
      setLines(stableModemSort(response.data ?? []))
      setError(null)
    } catch (err) {
      if (!background) setError(err instanceof Error ? err.message : String(err))
    } finally {
      if (!background) setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
    const timer = window.setInterval(() => void load(true), 3_000)
    return () => window.clearInterval(timer)
  }, [load])

  if (loading) {
    return <Box display="flex" justifyContent="center" minHeight="35vh" alignItems="center"><CircularProgress /></Box>
  }

  return (
    <Stack spacing={2}>
      <Box display="flex" justifyContent="space-between" alignItems="center" gap={2}>
        <Box>
          <Typography variant="h6" fontWeight={650}>Asterisk Trunk runtime</Typography>
          <Typography variant="body2" color="text.secondary">运行数据每 3 秒刷新，计数从当前线路端点启动时累计。</Typography>
        </Box>
        <Tooltip title="立即刷新">
          <IconButton onClick={() => void load()}><Refresh /></IconButton>
        </Tooltip>
      </Box>

      {error && <Alert severity="error">{error}</Alert>}
      {lines.length === 0 && <Alert severity="warning">当前没有可诊断的基带线路。</Alert>}

      {lines.map((line, index) => {
        const runtime = line.runtime
        const healthy = runtime.registered || runtime.phase === 'ready'
        return (
          <Card key={line.line_id} variant="outlined">
            <CardHeader
              avatar={<SettingsEthernet color={line.trunk.enabled ? 'primary' : 'disabled'} />}
              title={`${modemSlotLabel(line.modem, index)} · 卡槽 ${line.modem.uim_slot} · 线路 ${shortLineId(line.line_id)}`}
              subheader={`${line.modem.manufacturer || '未知厂商'} ${line.modem.model || ''} · ${line.trunk.registration_mode === 'outbound_register' ? '主动注册' : '静态 Peer'} · ${line.modem.present ? '在线' : '离线保留'} · ${modemSlotSourceLabel(line.modem.slot_source, line.modem.slot_stable)}`}
              sx={{
                alignItems: 'flex-start',
                flexWrap: { xs: 'wrap', sm: 'nowrap' },
                '& .MuiCardHeader-content': { minWidth: 0, flexBasis: { xs: 'calc(100% - 52px)', sm: 'auto' } },
                '& .MuiCardHeader-action': { margin: 0, marginLeft: { xs: '52px', sm: 'auto' }, width: { xs: 'calc(100% - 52px)', sm: 'auto' } },
              }}
              titleTypographyProps={{ variant: 'subtitle1', fontWeight: 650 }}
              action={
                <Stack direction="row" spacing={0.75} flexWrap="wrap" justifyContent={{ xs: 'flex-start', sm: 'flex-end' }}>
                  {line.modem.slot_conflict && <Chip size="small" label="槽位冲突" color="error" />}
                  <Chip size="small" label={phaseLabel(line)} color={healthy ? 'success' : line.trunk.enabled ? 'warning' : 'default'} />
                </Stack>
              }
            />
            <CardContent sx={{ pt: 0 }}>
              <Grid container spacing={2}>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="阶段" value={`${runtime.phase} / ${runtime.stage}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="本地 SIP" value={runtime.local_endpoint ?? '未监听'} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="Asterisk Peer" value={runtime.peer ?? '未解析'} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="最近 SIP 状态" value={runtime.last_sip_status ?? '无'} /></Grid>

                <Grid size={{ xs: 6, md: 3 }}><Metric label="REGISTER 尝试" value={runtime.register_attempts} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="重连次数" value={runtime.reconnect_count} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="注册时间" value={timestamp(runtime.registered_at)} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="到期时间" value={timestamp(runtime.expires_at)} /></Grid>

                <Grid size={{ xs: 6, md: 3 }}><Metric label="活跃对话 / 通话" value={`${runtime.active_dialogs} / ${runtime.active_calls}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="INVITE / re-INVITE" value={`${runtime.invite_count} / ${runtime.reinvite_count}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="媒体 / 视频协商" value={`${runtime.media_negotiations} / ${runtime.video_negotiations}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="DTMF 事件" value={runtime.dtmf_events} /></Grid>

                <Grid size={{ xs: 6, md: 3 }}><Metric label="SIP 接收" value={`${runtime.sip_rx_frames} 帧 · ${bytes(runtime.sip_rx_bytes)}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="SIP 发送" value={`${runtime.sip_tx_frames} 帧 · ${bytes(runtime.sip_tx_bytes)}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="Operator 命令 / 事件" value={`${runtime.operator_commands} / ${runtime.operator_events}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="活跃 RTP Relay" value={runtime.active_media_relays} /></Grid>

                <Grid size={{ xs: 6, md: 3 }}><Metric label="来自 Asterisk 的 RTP" value={`${runtime.rtp_from_asterisk_packets} 包 · ${bytes(runtime.rtp_from_asterisk_bytes)}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="发往 Asterisk 的 RTP" value={`${runtime.rtp_to_asterisk_packets} 包 · ${bytes(runtime.rtp_to_asterisk_bytes)}`} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="最近活动" value={timestamp(runtime.last_activity_at)} /></Grid>
                <Grid size={{ xs: 6, md: 3 }}><Metric label="下次重试" value={timestamp(runtime.next_retry_at)} /></Grid>
              </Grid>

              {runtime.last_error && <Alert severity="error" sx={{ mt: 2 }}>{runtime.last_error}</Alert>}
            </CardContent>
          </Card>
        )
      })}
    </Stack>
  )
}
