import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Alert, Box, Button, Card, CardContent, CircularProgress, FormControl,
  IconButton, InputLabel, List, ListItem, ListItemText,
  MenuItem, Select, Stack, Switch, TextField, Tooltip, Typography,
} from '@mui/material'
import { ArrowDownward, ArrowUpward } from '@mui/icons-material'
import {
  api,
  type VilteStatusResponse,
  type VolteVoiceStatusResponse,
  type VoiceAccessPathKind,
  type VoicePathPolicy,
  type WebCallCapabilitiesResponse,
} from '../../api/current'

const pathLabels: Record<VoiceAccessPathKind, string> = {
  vowifi: 'VoWiFi',
  volte: 'VoLTE',
}

type Props = { lineId: string }

export default function VoiceRoutingPanel({ lineId }: Props) {
  const activeLineId = useRef(lineId)
  const loadGeneration = useRef(0)
  activeLineId.current = lineId
  const [voicePath, setVoicePath] = useState<VoicePathPolicy | null>(null)
  const [webCall, setWebCall] = useState<WebCallCapabilitiesResponse | null>(null)
  const [vilte, setVilte] = useState<VilteStatusResponse | null>(null)
  const [volteVoice, setVolteVoice] = useState<VolteVoiceStatusResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const load = useCallback(async () => {
    const generation = ++loadGeneration.current
    if (!lineId) {
      setVoicePath(null)
      setWebCall(null)
      setVilte(null)
      setVolteVoice(null)
      setLoading(false)
      return
    }
    setLoading(true)
    setError(null)
    setSuccess(null)
    setVoicePath(null)
    setWebCall(null)
    setVilte(null)
    setVolteVoice(null)
    try {
      const [pathResponse, webResponse, vilteResponse, volteVoiceResponse] = await Promise.all([
        api.getVoicePathPolicy(lineId),
        api.getWebCallCapabilities(),
        api.getVilteStatus(lineId),
        api.getVolteVoiceStatus(lineId),
      ])
      if (generation !== loadGeneration.current || activeLineId.current !== lineId) return
      if (vilteResponse.data?.line_id !== lineId || volteVoiceResponse.data?.line_id !== lineId) {
        throw new Error('线路媒体状态响应与当前线路不匹配')
      }
      setVoicePath(pathResponse.data ?? null)
      setWebCall(webResponse.data ?? null)
      setVilte(vilteResponse.data ?? null)
      setVolteVoice(volteVoiceResponse.data ?? null)
    } catch (err) {
      if (generation === loadGeneration.current && activeLineId.current === lineId) {
        setError(err instanceof Error ? err.message : String(err))
      }
    } finally {
      if (generation === loadGeneration.current && activeLineId.current === lineId) {
        setLoading(false)
      }
    }
  }, [lineId])

  useEffect(() => { void load() }, [load])

  const vilteValidationError = useMemo(() => {
    if (!vilte) return null
    if (vilte.config.codec.trim().toLowerCase() !== 'h264') return 'ViLTE 视频编码仅支持 H.264'
    if (vilte.config.video_payload_type < 96 || vilte.config.video_payload_type > 127) {
      return 'RTP Payload Type 必须使用 96 至 127 的动态范围'
    }
    return null
  }, [vilte])

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
      const response = await api.setVoicePathPolicy(lineId, voicePath)
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
      const response = await api.setVilteConfig(lineId, vilte.config)
      if (activeLineId.current !== lineId) return
      if (response.data?.line_id !== lineId) throw new Error('ViLTE 配置响应线路不匹配')
      setVilte(response.data)
      setSuccess('当前线路 ViLTE 配置已保存')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  if (!lineId) return <Alert severity="info">请选择电话线路后查看语音路由。</Alert>
  if (loading || !voicePath) return <Box display="flex" justifyContent="center" py={6}><CircularProgress /></Box>

  return (
    <Stack spacing={2}>
      {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
      {success && <Alert severity="success" onClose={() => setSuccess(null)}>{success}</Alert>}
      {volteVoice && !volteVoice.registered && (
        <Alert severity="warning">当前线路 IMS 尚未注册，VoLTE/ViLTE 媒体中继不可用。</Alert>
      )}
      <Alert severity="info">
        此处只配置当前线路的 IMS 语音路径和 Asterisk Trunk。CS 基带通话由电话页的线路通话控制直接处理，不参与 Trunk 路由。
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
              <ListItemText primary={`${index + 1}. ${pathLabels[layer.kind]}`} secondary="网关模式；实际接通由对应 IMS 运行时和 Trunk 驱动" />
            </ListItem>
          ))}
        </List>
        <Button variant="contained" onClick={() => void savePath()} disabled={saving} sx={{ mt: 2 }}>{saving ? '保存中…' : '保存语音路径'}</Button>
      </CardContent></Card>

      <Card><CardContent>
        <Typography variant="h6">IMS 视频能力</Typography>
        <Alert severity="info" sx={{ my: 2 }}>视频中继自动跟随当前线路的 VoLTE 语音和 VoWiFi 连接，不需要单独开关。这里仅配置 H.264 中继参数，不会启动摄像头或主动发起视频呼叫。</Alert>
        {vilte && <Stack spacing={2}>
          <Typography variant="body2" color="text.secondary">
            VoLTE 语音：{volteVoice?.ims_connection_enabled ? '随 IMS 连接自动可用' : '请先启用当前线路的 VoLTE IMS 连接'}
          </Typography>
          <Typography variant="body2" color="text.secondary">
            VoLTE 视频：{vilte.config.volte_enabled ? '已随连接启用' : '等待 VoLTE 连接启用'}；VoWiFi 视频：{vilte.config.vowifi_enabled ? '已随连接启用' : '等待 VoWiFi 连接启用'}
          </Typography>
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
          <Button variant="outlined" onClick={() => void saveVilte()} disabled={saving || Boolean(vilteValidationError)}>保存 IMS 视频配置</Button>
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
