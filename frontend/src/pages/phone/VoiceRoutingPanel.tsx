import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  Alert, Box, Button, Card, CardContent, CircularProgress, FormControl,
  FormControlLabel, IconButton, InputLabel, List, ListItem, ListItemText,
  MenuItem, Select, Stack, Switch, TextField, Tooltip, Typography,
} from '@mui/material'
import { ArrowDownward, ArrowUpward } from '@mui/icons-material'
import {
  api,
  type AccessPathKind,
  type VilteStatusResponse,
  type VolteVoiceStatusResponse,
  type VoicePathPolicy,
  type WebCallCapabilitiesResponse,
} from '../../api/current'

const pathLabels: Record<AccessPathKind, string> = {
  vowifi: 'VoWiFi',
  volte: 'VoLTE',
  cs: 'CS 基带',
}

export default function VoiceRoutingPanel() {
  const [voicePath, setVoicePath] = useState<VoicePathPolicy | null>(null)
  const [webCall, setWebCall] = useState<WebCallCapabilitiesResponse | null>(null)
  const [vilte, setVilte] = useState<VilteStatusResponse | null>(null)
  const [volteVoice, setVolteVoice] = useState<VolteVoiceStatusResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [pathResponse, webResponse, vilteResponse, volteVoiceResponse] = await Promise.all([
        api.getVoicePathPolicy(),
        api.getWebCallCapabilities(),
        api.getVilteStatus(),
        api.getVolteVoiceStatus(),
      ])
      setVoicePath(pathResponse.data ?? null)
      setWebCall(webResponse.data ?? null)
      setVilte(vilteResponse.data ?? null)
      setVolteVoice(volteVoiceResponse.data ?? null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  const vilteValidationError = useMemo(() => {
    if (!vilte) return null
    if (vilte.config.codec.trim().toLowerCase() !== 'h264') return 'ViLTE 视频编码仅支持 H.264'
    if (vilte.config.video_payload_type < 96 || vilte.config.video_payload_type > 127) {
      return 'RTP Payload Type 必须使用 96 至 127 的动态范围'
    }
    if (vilte.config.feature_enabled && !volteVoice?.voice_enabled) {
      return '启用 ViLTE 前必须先启用 VoLTE 语音网关能力'
    }
    return null
  }, [vilte, volteVoice])

  const movePath = (index: number, delta: -1 | 1) => {
    if (!voicePath) return
    const target = index + delta
    if (target < 0 || target >= voicePath.priority.length) return
    const priority = [...voicePath.priority]
    ;[priority[index], priority[target]] = [priority[target], priority[index]]
    setVoicePath({ ...voicePath, priority })
  }

  const savePath = async () => {
    if (!voicePath) return
    setSaving(true)
    setError(null)
    try {
      const response = await api.setVoicePathPolicy(voicePath)
      if (response.data) setVoicePath(response.data)
      setSuccess('语音路径策略已保存')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  const saveVilte = async () => {
    if (!vilte || vilteValidationError) return
    setSaving(true)
    setError(null)
    try {
      const response = await api.setVilteConfig(vilte.config)
      if (response.data) setVilte(response.data)
      setSuccess('ViLTE 配置已保存')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  const toggleVolteVoice = async (enabled: boolean) => {
    setSaving(true)
    setError(null)
    try {
      const response = await api.setVolteVoice(enabled)
      if (response.data) setVolteVoice(response.data)
      if (!enabled) {
        const refreshed = await api.getVilteStatus()
        if (refreshed.data) setVilte(refreshed.data)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  if (loading || !voicePath) return <Box display="flex" justifyContent="center" py={6}><CircularProgress /></Box>

  return (
    <Stack spacing={2}>
      {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
      {success && <Alert severity="success" onClose={() => setSuccess(null)}>{success}</Alert>}
      <Alert severity="info">
        SimAdmin 只负责 IMS/CS 语音路径、媒体协商和 Asterisk Trunk 对接。来电广告、推销、验证码及其他分类由 Asterisk 拨号计划处理。
      </Alert>

      <Card><CardContent>
        <Typography variant="h6" gutterBottom>语音线路优先级</Typography>
        <Typography variant="body2" color="text.secondary" mb={1}>此策略独立于短信路径；启用的线路按顺序作为呼叫候选。</Typography>
        <List disablePadding>
          {voicePath.priority.map((layer, index) => (
            <ListItem key={layer.kind} divider secondaryAction={(
              <Box>
                <Tooltip title="上移"><span><IconButton disabled={index === 0} onClick={() => movePath(index, -1)}><ArrowUpward /></IconButton></span></Tooltip>
                <Tooltip title="下移"><span><IconButton disabled={index === voicePath.priority.length - 1} onClick={() => movePath(index, 1)}><ArrowDownward /></IconButton></span></Tooltip>
                <Switch checked={layer.enabled} onChange={(_, enabled) => setVoicePath({ ...voicePath, priority: voicePath.priority.map((item, i) => i === index ? { ...item, enabled } : item) })} />
              </Box>
            )}>
              <ListItemText primary={`${index + 1}. ${pathLabels[layer.kind]}`} secondary="网关模式；实际接通由对应 IMS/CS 运行时和 Trunk 驱动" />
            </ListItem>
          ))}
        </List>
        <Button variant="contained" onClick={() => void savePath()} disabled={saving} sx={{ mt: 2 }}>{saving ? '保存中…' : '保存语音路径'}</Button>
      </CardContent></Card>

      <Card><CardContent>
        <Typography variant="h6">ViLTE 视频能力</Typography>
        <Alert severity="warning" sx={{ my: 2 }}>这里只配置 H.264 中继参数，不会启动摄像头或发起视频呼叫。通话内视频切换需媒体入口接线后才可使用。</Alert>
        {vilte && <Stack spacing={2}>
          <FormControlLabel
            control={<Switch checked={volteVoice?.voice_enabled ?? false} disabled={!volteVoice?.feature_enabled || saving} onChange={(_, enabled) => void toggleVolteVoice(enabled)} />}
            label={volteVoice?.feature_enabled ? 'VoLTE 语音网关能力' : '请先在 SIM 卡线路中启用 VoLTE'}
          />
          <FormControlLabel
            control={<Switch checked={vilte.config.feature_enabled} disabled={!volteVoice?.voice_enabled || saving} onChange={(_, feature_enabled) => setVilte({ ...vilte, config: { ...vilte.config, feature_enabled } })} />}
            label="启用 ViLTE 能力（要求 VoLTE 语音已启用）"
          />
          <Box display="grid" gridTemplateColumns={{ xs: '1fr', md: '1fr 1fr' }} gap={2}>
            <FormControl fullWidth>
              <InputLabel>视频编码</InputLabel>
              <Select label="视频编码" value="h264" onChange={(event) => setVilte({ ...vilte, config: { ...vilte.config, codec: event.target.value } })}>
                <MenuItem value="h264">H.264</MenuItem>
              </Select>
            </FormControl>
            <TextField type="number" label="RTP Payload Type" value={vilte.config.video_payload_type} slotProps={{ htmlInput: { min: 96, max: 127 } }} helperText="动态 Payload Type 范围：96-127" onChange={(e) => setVilte({ ...vilte, config: { ...vilte.config, video_payload_type: Number(e.target.value) } })} />
          </Box>
          <TextField label="H.264 fmtp" value={vilte.config.h264_fmtp} onChange={(e) => setVilte({ ...vilte, config: { ...vilte.config, h264_fmtp: e.target.value } })} />
          {vilteValidationError && <Alert severity="error">{vilteValidationError}</Alert>}
          <Button variant="outlined" onClick={() => void saveVilte()} disabled={saving || Boolean(vilteValidationError)}>保存 ViLTE 配置</Button>
        </Stack>}
      </CardContent></Card>

      <Card><CardContent>
        <Typography variant="h6">网页直接接听</Typography>
        <Alert severity={webCall?.available ? 'success' : 'info'} sx={{ mt: 2 }}>{webCall?.note ?? '正在读取能力…'}</Alert>
        <Typography variant="body2" color="text.secondary" mt={1}>控制接口：{webCall?.control_plane_ready ? '已准备' : '未准备'}；浏览器媒体：{webCall?.ingress.browser_webrtc_ready ? '已接线' : '等待 WebRTC 网关'}。</Typography>
      </CardContent></Card>
    </Stack>
  )
}
