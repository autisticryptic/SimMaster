import { useEffect, useMemo, useState } from 'react'
import {
  Alert, Button, Dialog, DialogActions, DialogContent, DialogTitle,
  FormControl, InputLabel, MenuItem, Select, Stack, TextField,
} from '@mui/material'
import {
  api,
  type LineVowifiConfig,
  type VowifiLineConfigResponse,
  type VowifiProxyMode,
} from '../../api/current'
import { shortLineId } from '../../components/modemLineFormat'

interface Props {
  open: boolean
  line: VowifiLineConfigResponse | null
  onClose: () => void
  onSaved: (line: VowifiLineConfigResponse) => void
}

const proxyHints: Record<VowifiProxyMode, string> = {
  direct: '不使用代理，IKEv2 直接连接 ePDG',
  socks5_udp_associate: '支持 UDP ASSOCIATE 的 SOCKS5，例：socks5://user:pass@127.0.0.1:1080（mihomo / sing-box / Xray 均可）',
  udp_relay: '暂未实现。要自建转发请在远端跑标准 SOCKS5（sing-box / mihomo / gost），再用上面的 SOCKS5 模式',
}

export default function VowifiLineDialog({ open, line, onClose, onSaved }: Props) {
  const [draft, setDraft] = useState<LineVowifiConfig | null>(null)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (line) setDraft({ ...line.config })
    setError(null)
  }, [line, open])

  const validationError = useMemo(() => {
    if (!draft) return null
    if (draft.proxy_mode !== 'direct' && !draft.proxy_endpoint.trim()) return '所选代理模式需要填写代理端点'
    return null
  }, [draft])

  const update = <K extends keyof LineVowifiConfig>(key: K, value: LineVowifiConfig[K]) => {
    setDraft((current) => current ? { ...current, [key]: value } : current)
  }

  const save = async () => {
    if (!line || !draft || validationError) return
    setSaving(true)
    setError(null)
    try {
      const response = await api.setVowifiLineConfig(line.line_id, draft)
      if (response.data) onSaved(response.data)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  if (!line || !draft) return null

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="sm">
      <DialogTitle>WiFi Calling 配置 · {shortLineId(line.line_id)}</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <Alert severity="info">
            这里仅配置<strong>这条线路</strong>的运行方式。运营商 profile、DNS、ePDG 和 IMS 覆写跟随 SIM 卡保存，
            换卡或移动 SIM 时不会与物理线路配置混用。
          </Alert>
          <FormControl fullWidth>
            <InputLabel>代理模式</InputLabel>
            <Select
              label="代理模式"
              value={draft.proxy_mode}
              onChange={(event) => update('proxy_mode', event.target.value as VowifiProxyMode)}
            >
              <MenuItem value="direct">直连</MenuItem>
              <MenuItem value="socks5_udp_associate">SOCKS5 UDP Associate</MenuItem>
              <MenuItem value="udp_relay" disabled>UDP Relay（未实现，建议自建 SOCKS5 代替）</MenuItem>
            </Select>
          </FormControl>
          <TextField
            label="代理端点"
            value={draft.proxy_endpoint}
            disabled={draft.proxy_mode === 'direct'}
            placeholder={proxyHints[draft.proxy_mode]}
            helperText={proxyHints[draft.proxy_mode]}
            onChange={(event) => update('proxy_endpoint', event.target.value)}
          />
          <Alert severity="info">
            每条线路各自持有独立的 VoWiFi 运行时、TUN 网卡与代理出口，多张不同国家的 SIM 可以同时注册，互不影响。
            普通 HTTP CONNECT 无法转发 IKEv2 的 UDP 500/4500，所以只提供直连与 SOCKS5 两种模式。
          </Alert>
          {validationError && <Alert severity="error">{validationError}</Alert>}
          {error && <Alert severity="error">{error}</Alert>}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button variant="contained" onClick={() => void save()} disabled={saving || Boolean(validationError)}>
          {saving ? '保存中...' : '保存配置'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
