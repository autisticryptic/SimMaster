import { useCallback, useEffect, useState } from 'react'
import {
  Alert, Box, Button, Card, CardContent, Chip, CircularProgress, Divider,
  FormControl, FormControlLabel, IconButton, InputLabel, List, ListItem,
  ListItemText, MenuItem, Select, Stack, Switch, TextField, Tooltip, Typography,
} from '@mui/material'
import { Add, ArrowDownward, ArrowUpward, Delete, Done, Refresh } from '@mui/icons-material'
import {
  api,
  type AccessPathKind,
  type CallHandlingAction,
  type CallScreeningDecision,
  type IncomingNumberRule,
  type VilteStatusResponse,
  type VolteVoiceStatusResponse,
  type VoiceInboxListResponse,
  type VoicePathPolicy,
  type VoiceServicesConfig,
  type WebCallCapabilitiesResponse,
} from '../../api/current'

const actionLabels: Record<CallHandlingAction, string> = {
  forward: '转发到内部话机',
  screen: '先进行语音筛选',
  voicemail: '进入语音信箱',
  reject: '拒绝/终止',
}
const pathLabels: Record<AccessPathKind, string> = { vowifi: 'VoWiFi', volte: 'VoLTE', cs: 'CS 基带' }
const categoryLabels: Record<string, string> = {
  whitelisted: '白名单', blacklisted: '黑名单', verification: '验证码',
  marketing: '疑似营销', ordinary: '普通来电', unknown: '未知',
}

function actionSelect(value: CallHandlingAction, onChange: (value: CallHandlingAction) => void, label: string) {
  return (
    <FormControl fullWidth size="small">
      <InputLabel>{label}</InputLabel>
      <Select label={label} value={value} onChange={(event) => onChange(event.target.value as CallHandlingAction)}>
        {Object.entries(actionLabels).map(([key, text]) => <MenuItem key={key} value={key}>{text}</MenuItem>)}
      </Select>
    </FormControl>
  )
}

export default function VoiceServicesPanel() {
  const [config, setConfig] = useState<VoiceServicesConfig | null>(null)
  const [voicePath, setVoicePath] = useState<VoicePathPolicy | null>(null)
  const [inbox, setInbox] = useState<VoiceInboxListResponse | null>(null)
  const [webCall, setWebCall] = useState<WebCallCapabilitiesResponse | null>(null)
  const [vilte, setVilte] = useState<VilteStatusResponse | null>(null)
  const [volteVoice, setVolteVoice] = useState<VolteVoiceStatusResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [testNumber, setTestNumber] = useState('')
  const [testTranscript, setTestTranscript] = useState('')
  const [testDecision, setTestDecision] = useState<CallScreeningDecision | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [status, inboxResponse, webResponse, vilteResponse, volteVoiceResponse] = await Promise.all([
        api.getVoiceServicesStatus(), api.getVoiceInbox(), api.getWebCallCapabilities(), api.getVilteStatus(), api.getVolteVoiceStatus(),
      ])
      setConfig(status.data?.config ?? null)
      setVoicePath(status.data?.voice_path ?? null)
      setInbox(inboxResponse.data ?? null)
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

  const saveConfig = async () => {
    if (!config || !voicePath) return
    setSaving(true); setError(null); setSuccess(null)
    try {
      const [configResponse, pathResponse] = await Promise.all([
        api.setVoiceServicesConfig(config), api.setVoicePathPolicy(voicePath),
      ])
      if (configResponse.data) setConfig(configResponse.data)
      if (pathResponse.data) setVoicePath(pathResponse.data)
      setSuccess('语音服务策略已保存')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally { setSaving(false) }
  }

  const updateRule = (index: number, patch: Partial<IncomingNumberRule>) => {
    if (!config) return
    setConfig({ ...config, number_rules: config.number_rules.map((rule, i) => i === index ? { ...rule, ...patch } : rule) })
  }

  const addRule = () => {
    if (!config) return
    setConfig({ ...config, number_rules: [...config.number_rules, {
      id: `rule-${Date.now()}`, name: '', enabled: true, list: 'whitelist', matcher: 'exact', pattern: '', action: 'forward',
    }] })
  }

  const movePath = (index: number, delta: -1 | 1) => {
    if (!voicePath) return
    const target = index + delta
    if (target < 0 || target >= voicePath.priority.length) return
    const priority = [...voicePath.priority]
    ;[priority[index], priority[target]] = [priority[target], priority[index]]
    setVoicePath({ ...voicePath, priority })
  }

  const runDryScreen = async () => {
    setError(null); setTestDecision(null)
    try {
      const response = await api.screenVoiceCall(testNumber, testTranscript.trim() || undefined)
      setTestDecision(response.data ?? null)
    } catch (err) { setError(err instanceof Error ? err.message : String(err)) }
  }

  const updateInbox = async (id: number, action: 'read' | 'delete') => {
    try {
      if (action === 'read') await api.setVoiceInboxRead(id, true)
      else await api.deleteVoiceInbox(id)
      const response = await api.getVoiceInbox()
      setInbox(response.data ?? null)
    } catch (err) { setError(err instanceof Error ? err.message : String(err)) }
  }

  const saveVilte = async () => {
    if (!vilte) return
    setSaving(true); setError(null)
    try {
      const response = await api.setVilteConfig(vilte.config)
      if (response.data) setVilte(response.data)
      setSuccess('ViLTE 配置已保存')
    } catch (err) { setError(err instanceof Error ? err.message : String(err)) }
    finally { setSaving(false) }
  }

  const toggleVolteVoice = async (enabled: boolean) => {
    setSaving(true); setError(null)
    try {
      const response = await api.setVolteVoice(enabled)
      if (response.data) setVolteVoice(response.data)
      if (!enabled) {
        const refreshed = await api.getVilteStatus()
        if (refreshed.data) setVilte(refreshed.data)
      }
    } catch (err) { setError(err instanceof Error ? err.message : String(err)) }
    finally { setSaving(false) }
  }

  if (loading || !config || !voicePath) return <Box display="flex" justifyContent="center" py={6}><CircularProgress /></Box>

  return (
    <Stack spacing={2}>
      {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
      {success && <Alert severity="success" onClose={() => setSuccess(null)}>{success}</Alert>}
      <Alert severity="info">
        当前完成的是规则、收件箱、转写结果处理和通知。实际取音频、转发到 Linphone 或网页接听仍等待媒体接入方式确定。
      </Alert>

      <Card><CardContent>
        <Box display="flex" justifyContent="space-between" alignItems="center" gap={2}>
          <Box><Typography variant="h6">来电筛选与语音信箱</Typography><Typography variant="body2" color="text.secondary">功能默认关闭；号码规则从上到下匹配</Typography></Box>
          <Switch checked={config.feature_enabled} onChange={(_, feature_enabled) => setConfig({ ...config, feature_enabled })} />
        </Box>
        <Divider sx={{ my: 2 }} />
        <Stack spacing={2}>
          {actionSelect(config.unknown_number_action, (unknown_number_action) => setConfig({ ...config, unknown_number_action }), '未知号码接通前')}
          <Box display="grid" gridTemplateColumns={{ xs: '1fr', md: '1fr 1fr' }} gap={2}>
            {actionSelect(config.verification_action, (verification_action) => setConfig({ ...config, verification_action }), '验证码语音')}
            {actionSelect(config.marketing_action, (marketing_action) => setConfig({ ...config, marketing_action }), '疑似营销语音')}
            {actionSelect(config.ordinary_action, (ordinary_action) => setConfig({ ...config, ordinary_action }), '普通语音')}
            {actionSelect(config.uncertain_action, (uncertain_action) => setConfig({ ...config, uncertain_action }), '无法确定')}
          </Box>
          <Box display="grid" gridTemplateColumns={{ xs: '1fr', md: 'repeat(3, 1fr)' }} gap={2}>
            <TextField type="number" label="筛选最长秒数" value={config.screening_max_seconds} onChange={(e) => setConfig({ ...config, screening_max_seconds: Number(e.target.value) })} />
            <TextField type="number" label="收件箱保留天数" value={config.inbox_retention_days} onChange={(e) => setConfig({ ...config, inbox_retention_days: Number(e.target.value) })} />
            <TextField type="number" label="收件箱最多条目" value={config.inbox_max_entries} onChange={(e) => setConfig({ ...config, inbox_max_entries: Number(e.target.value) })} />
          </Box>
          <TextField label="验证码关键词（逗号分隔）" value={config.verification_keywords.join(', ')} onChange={(e) => setConfig({ ...config, verification_keywords: e.target.value.split(/[,，]/).map((item) => item.trim()).filter(Boolean) })} />
          <TextField label="营销关键词（逗号分隔）" value={config.marketing_keywords.join(', ')} onChange={(e) => setConfig({ ...config, marketing_keywords: e.target.value.split(/[,，]/).map((item) => item.trim()).filter(Boolean) })} />
        </Stack>

        <Box display="flex" justifyContent="space-between" alignItems="center" mt={3} mb={1}>
          <Typography variant="subtitle1" fontWeight={600}>号码黑白名单</Typography>
          <Button startIcon={<Add />} onClick={addRule}>添加规则</Button>
        </Box>
        {config.number_rules.length === 0 && <Alert severity="info">暂无号码规则。未知号码将按上方默认动作处理。</Alert>}
        <Stack spacing={1}>
          {config.number_rules.map((rule, index) => (
            <Box key={rule.id} display="grid" gridTemplateColumns={{ xs: '1fr', md: 'auto 1fr 130px 130px 1fr 180px auto' }} gap={1} alignItems="center">
              <Switch checked={rule.enabled} onChange={(_, enabled) => updateRule(index, { enabled })} />
              <TextField size="small" label="名称" value={rule.name} onChange={(e) => updateRule(index, { name: e.target.value })} />
              <Select size="small" value={rule.list} onChange={(e) => updateRule(index, { list: e.target.value as IncomingNumberRule['list'] })}><MenuItem value="whitelist">白名单</MenuItem><MenuItem value="blacklist">黑名单</MenuItem></Select>
              <Select size="small" value={rule.matcher} onChange={(e) => updateRule(index, { matcher: e.target.value as IncomingNumberRule['matcher'] })}><MenuItem value="exact">完全匹配</MenuItem><MenuItem value="prefix">号码开头</MenuItem><MenuItem value="suffix">号码结尾</MenuItem><MenuItem value="contains">包含</MenuItem></Select>
              <TextField size="small" label="号码模式" value={rule.pattern} onChange={(e) => updateRule(index, { pattern: e.target.value })} />
              <Select size="small" value={rule.action} onChange={(e) => updateRule(index, { action: e.target.value as CallHandlingAction })}>{Object.entries(actionLabels).map(([key, text]) => <MenuItem key={key} value={key}>{text}</MenuItem>)}</Select>
              <IconButton color="error" onClick={() => setConfig({ ...config, number_rules: config.number_rules.filter((_, i) => i !== index) })}><Delete /></IconButton>
            </Box>
          ))}
        </Stack>

        <Divider sx={{ my: 3 }} />
        <Typography variant="subtitle1" fontWeight={600}>语音线路优先级（独立于短信）</Typography>
        <List disablePadding>
          {voicePath.priority.map((layer, index) => <ListItem key={layer.kind} divider secondaryAction={<Box><Tooltip title="上移"><span><IconButton disabled={index === 0} onClick={() => movePath(index, -1)}><ArrowUpward /></IconButton></span></Tooltip><Tooltip title="下移"><span><IconButton disabled={index === voicePath.priority.length - 1} onClick={() => movePath(index, 1)}><ArrowDownward /></IconButton></span></Tooltip><Switch checked={layer.enabled} onChange={(_, enabled) => setVoicePath({ ...voicePath, priority: voicePath.priority.map((item, i) => i === index ? { ...item, enabled } : item) })} /></Box>}><ListItemText primary={`${index + 1}. ${pathLabels[layer.kind]}`} secondary="网关模式；当前没有媒体适配器时不会实际选中线路" /></ListItem>)}
        </List>
        <Button variant="contained" onClick={() => void saveConfig()} disabled={saving} sx={{ mt: 2 }}>{saving ? '保存中…' : '保存语音策略'}</Button>
      </CardContent></Card>

      <Card><CardContent>
        <Typography variant="h6" gutterBottom>规则模拟（不拨号）</Typography>
        <Box display="grid" gridTemplateColumns={{ xs: '1fr', md: '1fr 2fr auto' }} gap={1}>
          <TextField label="来电号码" value={testNumber} onChange={(e) => setTestNumber(e.target.value)} />
          <TextField label="可选的语音转写" value={testTranscript} onChange={(e) => setTestTranscript(e.target.value)} />
          <Button variant="outlined" onClick={() => void runDryScreen()} disabled={!testNumber.trim()}>模拟判断</Button>
        </Box>
        {testDecision && <Alert severity="success" sx={{ mt: 2 }}>分类：{categoryLabels[testDecision.category] ?? testDecision.category}；动作：{actionLabels[testDecision.action]}{testDecision.verification_code ? `；验证码：${testDecision.verification_code}` : ''}</Alert>}
      </CardContent></Card>

      <Card><CardContent>
        <Box display="flex" justifyContent="space-between" alignItems="center"><Typography variant="h6">语音收件箱</Typography><IconButton onClick={() => void load()}><Refresh /></IconButton></Box>
        <Typography variant="body2" color="text.secondary" mb={1}>未读 {inbox?.stats.waiting ?? 0}，验证码 {inbox?.stats.verification ?? 0}，疑似营销 {inbox?.stats.marketing ?? 0}</Typography>
        {!inbox?.messages.length ? <Alert severity="info">暂无语音留言或筛选记录</Alert> : <List>{inbox.messages.map((message) => <ListItem key={message.id} divider secondaryAction={<Box>{message.status === 'new' && <Tooltip title="标记已读"><IconButton onClick={() => void updateInbox(message.id, 'read')}><Done /></IconButton></Tooltip>}<Tooltip title="删除"><IconButton color="error" onClick={() => void updateInbox(message.id, 'delete')}><Delete /></IconButton></Tooltip></Box>}><ListItemText primary={<Box display="flex" gap={1} flexWrap="wrap"><Typography fontWeight={600}>{message.phone_number}</Typography><Chip size="small" label={categoryLabels[message.category] ?? message.category} color={message.category === 'verification' ? 'success' : message.category === 'marketing' ? 'warning' : 'default'} />{message.verification_code && <Chip size="small" label={`验证码 ${message.verification_code}`} color="success" />}</Box>} secondary={`${new Date(message.created_at).toLocaleString('zh-CN')} · ${message.transcript}`} /></ListItem>)}</List>}
      </CardContent></Card>

      <Card><CardContent>
        <Typography variant="h6">ViLTE 视频能力</Typography>
        <Alert severity="warning" sx={{ my: 2 }}>这里只配置 H.264 中继参数，不会启动摄像头或发起视频呼叫。通话内视频切换需媒体入口接线后才可使用。</Alert>
        {vilte && <Stack spacing={2}>
          <FormControlLabel
            control={<Switch checked={volteVoice?.voice_enabled ?? false} disabled={!volteVoice?.feature_enabled || saving} onChange={(_, enabled) => void toggleVolteVoice(enabled)} />}
            label={volteVoice?.feature_enabled ? 'VoLTE 语音网关能力' : '请先启用 VoLTE 总开关'}
          />
          <FormControlLabel control={<Switch checked={vilte.config.feature_enabled} onChange={(_, feature_enabled) => setVilte({ ...vilte, config: { ...vilte.config, feature_enabled } })} />} label="启用 ViLTE 能力（要求 VoLTE 语音已启用）" />
          <Box display="grid" gridTemplateColumns={{ xs: '1fr', md: '1fr 1fr' }} gap={2}><TextField label="视频编码" value={vilte.config.codec} onChange={(e) => setVilte({ ...vilte, config: { ...vilte.config, codec: e.target.value } })} /><TextField type="number" label="RTP Payload Type" value={vilte.config.video_payload_type} onChange={(e) => setVilte({ ...vilte, config: { ...vilte.config, video_payload_type: Number(e.target.value) } })} /></Box>
          <TextField label="H.264 fmtp" value={vilte.config.h264_fmtp} onChange={(e) => setVilte({ ...vilte, config: { ...vilte.config, h264_fmtp: e.target.value } })} />
          <Button variant="outlined" onClick={() => void saveVilte()} disabled={saving}>保存 ViLTE 配置</Button>
        </Stack>}
      </CardContent></Card>

      <Card><CardContent>
        <Typography variant="h6">网页直接接听</Typography>
        <Alert severity={webCall?.available ? 'success' : 'info'} sx={{ mt: 2 }}>{webCall?.note ?? '正在读取能力…'}</Alert>
        <Typography variant="body2" color="text.secondary" mt={1}>控制接口：{webCall?.control_plane_ready ? '已准备' : '未准备'}；浏览器媒体：{webCall?.ingress.browser_webrtc_ready ? '已接线' : '等待 WebRTC 网关'}。浏览器侧必须使用 WSS、DTLS-SRTP、ICE 和短期会话令牌。</Typography>
      </CardContent></Card>
    </Stack>
  )
}
