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
    if (!draft.asterisk_host.trim()) return '请填写 Asterisk 地址'
    if (draft.asterisk_port < 1 || draft.asterisk_port > 65535) return '端口必须在 1–65535 之间'
    if (draft.registration_mode === 'outbound_register' && !draft.username.trim()) {
      return '主动注册模式需要填写用户名'
    }
    if (draft.register_expiry_secs < 60 || draft.register_expiry_secs > 86400) {
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
            当前阶段只保存配置。真正的 SIP REGISTER、INVITE 和 RTP 桥接将在 D4–D6 接线后启动。
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
            <TextField
              size="small"
              label="入呼 Extension / 线路路由键"
              value={draft.extension}
              onChange={(event) => update('extension', event.target.value)}
              placeholder="4101"
            />
            <TextField
              size="small"
              label="允许的编解码器"
              value={codecText}
              onChange={(event) => setCodecText(event.target.value)}
              placeholder="amr-wb, amr"
              helperText="使用英文逗号分隔；SimAdmin 只转发，不转码"
            />
            <TextField
              size="small"
              type="number"
              label="REGISTER 周期（秒）"
              value={draft.register_expiry_secs}
              onChange={(event) => update('register_expiry_secs', Number(event.target.value))}
              disabled={draft.registration_mode !== 'outbound_register'}
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
