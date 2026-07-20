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
      const response = await api.setLineDataProxyConfig(lineId, {
        listen_ip: draft.listen_ip.trim(),
        listen_port: Number(draft.listen_port) || 0,
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
          <Alert severity="info">
            同一端口自动识别 HTTP 代理和 SOCKS5。监听地址变更会在数据连接已启用时立即重启代理监听。
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
