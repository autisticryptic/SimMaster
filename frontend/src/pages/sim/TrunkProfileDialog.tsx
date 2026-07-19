import { useEffect, useMemo, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormControl,
  FormControlLabel,
  InputLabel,
  MenuItem,
  Select,
  Stack,
  Switch,
  TextField,
  Typography,
} from '@mui/material'
import {
  api,
  type TrunkIncomingMode,
  type TrunkIpConnectMode,
  type TrunkProfileConfig,
  type TrunkProfileResponse,
  type TrunkRegistrationMode,
} from '../../api/current'
import { shortLineId } from '../../components/modemLineFormat'

interface TrunkProfileDialogProps {
  open: boolean
  line: TrunkProfileResponse | null
  onClose: () => void
  onSaved: (line: TrunkProfileResponse) => void
}

function cloneProfile(profile: TrunkProfileConfig): TrunkProfileConfig {
  return {
    ...profile,
    codec_allow: [...profile.codec_allow],
    secret: '',
  }
}

export default function TrunkProfileDialog({ open, line, onClose, onSaved }: TrunkProfileDialogProps) {
  const [draft, setDraft] = useState<TrunkProfileConfig | null>(null)
  const [codecText, setCodecText] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!line) return
    setDraft(cloneProfile(line.trunk))
    setCodecText(line.trunk.codec_allow.join(', '))
    setError(null)
  }, [line, open])

  const validationError = useMemo(() => {
    if (!draft) return '没有可编辑的线路'
    if (!draft.enabled) return null
    if (!draft.asterisk_host.trim()) return '请填写 Asterisk 地址'
    if (draft.asterisk_port < 1 || draft.asterisk_port > 65535) return '端口必须在 1–65535 之间'
    if (draft.local_port < 1 || draft.local_port > 65535) {
      return '启用 Trunk 时需要为每条线路配置唯一的本地 SIP 端口（1–65535）'
    }
    if (draft.registration_mode === 'outbound_register' && !draft.username.trim()) {
      return '主动注册模式需要填写用户名'
    }
    if (
      draft.registration_mode === 'outbound_register'
      && (draft.register_expiry_secs < 60 || draft.register_expiry_secs > 86400)
    ) {
      return '注册周期必须在 60–86400 秒之间'
    }
    return null
  }, [draft])

  const update = <K extends keyof TrunkProfileConfig>(key: K, value: TrunkProfileConfig[K]) => {
    setDraft((current) => current ? { ...current, [key]: value } : current)
  }

  const save = async () => {
    if (!line || !draft || validationError) return
    setSaving(true)
    setError(null)
    try {
      const profile: TrunkProfileConfig = {
        ...draft,
        codec_allow: codecText
          .split(',')
          .map((codec) => codec.trim().toLowerCase())
          .filter((codec, index, all) => codec && all.indexOf(codec) === index),
        match_host: draft.registration_mode === 'static_peer'
          ? draft.match_host?.trim() || draft.asterisk_host.trim()
          : null,
      }
      const response = await api.setTrunkLine(line.line_id, profile)
      if (response.data) onSaved(response.data)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  if (!draft || !line) return null

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="md">
      <DialogTitle>Asterisk Trunk · 线路 {shortLineId(line.line_id)}</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2.25}>
          <Alert severity="info">
            SIP UDP、REGISTER、双向 INVITE、DTMF 与 RTP 转发均已接线；SimAdmin 仅转发已协商的媒体，不执行转码。
          </Alert>

          <FormControlLabel
            control={<Switch checked={draft.enabled} onChange={(_, enabled) => update('enabled', enabled)} />}
            label="保存后启用此线路的 Trunk 意图"
          />

          <Box display="grid" gridTemplateColumns={{ xs: '1fr', sm: '1fr 1fr' }} gap={2}>
            <FormControl size="small" fullWidth>
              <InputLabel>连接模式</InputLabel>
              <Select
                value={draft.registration_mode}
                label="连接模式"
                onChange={(event) => update('registration_mode', event.target.value as TrunkRegistrationMode)}
              >
                <MenuItem value="outbound_register">主动 REGISTER（远程/NAT 推荐）</MenuItem>
                <MenuItem value="static_peer">静态 Peer（不注册、双向 INVITE）</MenuItem>
              </Select>
            </FormControl>
            <TextField
              size="small"
              type="number"
              label="注册时长（秒）"
              value={draft.register_expiry_secs}
              onChange={(event) => update('register_expiry_secs', Number(event.target.value))}
              disabled={draft.registration_mode !== 'outbound_register'}
              helperText="主动 REGISTER 有效期，允许 60–86400 秒"
            />
            <TextField
              size="small"
              label="Asterisk 地址"
              value={draft.asterisk_host}
              onChange={(event) => update('asterisk_host', event.target.value)}
              placeholder="pbx.example.com 或 10.0.0.10"
            />
            <TextField
              size="small"
              type="number"
              label="SIP 端口"
              value={draft.asterisk_port}
              onChange={(event) => update('asterisk_port', Number(event.target.value))}
            />
            <TextField
              size="small"
              type="number"
              label="本地 SIP 监听端口"
              value={draft.local_port}
              onChange={(event) => update('local_port', Number(event.target.value))}
              helperText="保持 Contact 稳定；多线路必须使用不同端口，例如 5062、5064"
            />
            <TextField
              size="small"
              label="线路用户名"
              value={draft.username}
              onChange={(event) => update('username', event.target.value)}
              helperText={draft.registration_mode === 'outbound_register' ? '用于 REGISTER 鉴权和线路身份' : '可作为静态 Peer 的线路标识'}
            />
            <TextField
              size="small"
              type="password"
              label="鉴权密码"
              value={draft.secret}
              onChange={(event) => update('secret', event.target.value)}
              placeholder={line.secret_set ? '已配置；留空保持原密码' : '尚未配置'}
              helperText={line.secret_set ? '服务器已保存密码，API 不会回传明文' : '密码只写入设备配置，响应始终脱敏'}
            />
            <TextField
              size="small"
              label="Asterisk Context"
              value={draft.context}
              onChange={(event) => update('context', event.target.value)}
              helperText="部署元数据；Context 实际由 Asterisk endpoint 决定"
            />
            <FormControl size="small" fullWidth>
              <InputLabel>呼入类型</InputLabel>
              <Select
                value={draft.incoming_mode}
                label="呼入类型"
                onChange={(event) => update('incoming_mode', event.target.value as TrunkIncomingMode)}
              >
                <MenuItem value="secondary_dial">二次拨号（Asterisk IVR）</MenuItem>
                <MenuItem value="bound_pending">绑定待接（分机接听后接通运营商）</MenuItem>
                <MenuItem value="bound_immediate">绑定立接（先接通运营商再呼叫分机）</MenuItem>
              </Select>
            </FormControl>
            <TextField
              size="small"
              label="呼入绑定"
              value={draft.incoming_binding}
              onChange={(event) => update('incoming_binding', event.target.value)}
              placeholder="6108"
              helperText={draft.incoming_mode === 'secondary_dial'
                ? '填写 Asterisk IVR 分机；提示音、收号和二次路由由 Asterisk 处理'
                : '运营商来电将呼叫此 Asterisk 分机'}
            />
            <TextField
              size="small"
              label="呼出绑定"
              value={draft.outgoing_binding}
              onChange={(event) => update('outgoing_binding', event.target.value)}
              placeholder="6108"
              helperText="填写后，仅允许 From 用户匹配的 Asterisk 分机通过此 SIM 呼出；留空不限制"
            />
            <Box>
              <FormControl size="small" fullWidth>
                <InputLabel>IP 接通方式</InputLabel>
                <Select
                  value={draft.ip_connect_mode}
                  label="IP 接通方式"
                  onChange={(event) => update('ip_connect_mode', event.target.value as TrunkIpConnectMode)}
                >
                  <MenuItem value="first_rtp">IP 接通</MenuItem>
                  <MenuItem value="gsm_answer">GSM 接通时，立即接通</MenuItem>
                </Select>
              </FormControl>
              <Typography variant="caption" color="text.secondary" display="block">
                IP 接通：首个运营商 RTP 后向 Asterisk 返回 200；GSM 接通：运营商应答后立即返回 200。
              </Typography>
            </Box>
            <TextField
              size="small"
              label="允许的编解码器"
              value={codecText}
              onChange={(event) => setCodecText(event.target.value)}
              placeholder="amr-wb, amr"
              helperText="使用英文逗号分隔；SimAdmin 只转发，不转码"
            />
            {draft.registration_mode === 'static_peer' && (
              <TextField
                size="small"
                label="允许的 Peer 地址"
                value={draft.match_host ?? ''}
                onChange={(event) => update('match_host', event.target.value)}
                placeholder="留空时使用 Asterisk 地址"
              />
            )}
          </Box>

          {validationError && <Typography variant="caption" color="error">{validationError}</Typography>}
          {error && <Alert severity="error">{error}</Alert>}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button variant="contained" onClick={() => void save()} disabled={saving || Boolean(validationError)}>
          {saving ? '保存中…' : '保存配置'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
