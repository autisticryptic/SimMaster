import { useEffect, useState } from 'react'
import {
  Alert,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Stack,
  TextField,
} from '@mui/material'
import { api, type LineDataProxyConfig, type LineNetworkControlsResponse } from '../../api/current'

interface DataProxyDialogProps {
  open: boolean
  lineId: string | null
  controls: LineNetworkControlsResponse | null
  onClose: () => void
  onSaved: (updated: LineNetworkControlsResponse) => void
}

export default function DataProxyDialog({ open, lineId, controls, onClose, onSaved }: DataProxyDialogProps) {
  const [draft, setDraft] = useState<LineDataProxyConfig>({ listen_ip: '0.0.0.0', listen_port: 0 })
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open || !controls) return
    setDraft({ ...controls.data.config })
    setError(null)
  }, [open, controls])

  const save = async () => {
    if (!lineId) return
    setSaving(true)
    setError(null)
    try {
      const username = (draft.username ?? '').trim()
      const password = draft.password ?? ''
      const keepingSavedPassword = username.length > 0
        && username === (controls?.data.config.username ?? '')
        && controls?.data.password_set === true
        && password.length === 0
      if ((username.length === 0) !== (password.length === 0) && !keepingSavedPassword) {
        setError('启用代理认证时，用户名和密码必须同时填写')
        return
      }
      const response = await api.setLineDataProxyConfig(lineId, {
        listen_ip: draft.listen_ip.trim(),
        listen_port: Number(draft.listen_port) || 0,
        username,
        password,
      })
      if (response.data) {
        onSaved(response.data)
        onClose()
      } else {
        setError(response.message)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onClose={() => !saving && onClose()} fullWidth maxWidth="xs">
      <DialogTitle>数据代理出口配置</DialogTitle>
      <DialogContent>
        <Stack spacing={2} pt={1}>
          {error && <Alert severity="error">{error}</Alert>}
          <TextField
            label="监听 IP"
            value={draft.listen_ip}
            onChange={(event) => setDraft((current) => ({ ...current, listen_ip: event.target.value }))}
            helperText="127.0.0.1 仅本机可用；0.0.0.0 允许外部网络访问"
            disabled={saving}
            fullWidth
          />
          <TextField
            label="监听端口"
            type="number"
            value={draft.listen_port}
            onChange={(event) => setDraft((current) => ({ ...current, listen_port: Number(event.target.value) }))}
            helperText="填写 0 自动分配可用端口"
            inputProps={{ min: 0, max: 65535 }}
            disabled={saving}
            fullWidth
          />
          <TextField
            label="认证用户名"
            value={draft.username ?? ''}
            onChange={(event) => setDraft((current) => ({ ...current, username: event.target.value }))}
            helperText="用户名和密码都为空时不启用代理认证"
            disabled={saving}
            autoComplete="username"
            fullWidth
          />
          <TextField
            label="认证密码"
            type="password"
            value={draft.password ?? ''}
            onChange={(event) => setDraft((current) => ({ ...current, password: event.target.value }))}
            placeholder={controls?.data.password_set ? '留空保留已保存密码' : '填写后启用认证'}
            helperText={controls?.data.password_set ? '如需关闭认证，请清空用户名并保存' : '与用户名同时填写后启用认证'}
            disabled={saving}
            autoComplete="new-password"
            fullWidth
          />
          <Alert severity="info">
            同一端口自动识别 HTTP 代理和 SOCKS5。监听地址变更会在数据连接已启用时立即重启代理监听。{controls?.data.proxy.auth_required ? ' 当前已启用认证。' : ''}
          </Alert>
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button onClick={() => void save()} variant="contained" disabled={saving || !draft.listen_ip.trim()}>保存</Button>
      </DialogActions>
    </Dialog>
  )
}
