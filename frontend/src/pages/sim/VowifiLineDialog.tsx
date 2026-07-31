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
  type StoredCarrierProfile,
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
  const [profiles, setProfiles] = useState<StoredCarrierProfile[]>([])

  useEffect(() => {
    if (line) setDraft({ ...line.config })
    if (open) void api.listVowifiCarrierProfiles().then((response) => setProfiles(response.data ?? [])).catch(() => setProfiles([]))
    setError(null)
  }, [line, open])

  const validationError = useMemo(() => {
    if (!draft) return null
    if (draft.dns_server && !/^[0-9a-f:.]+$/i.test(draft.dns_server.trim())) {
      return 'DNS 解析器必须填写 IPv4 或 IPv6 地址'
    }
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
            这里配置的是<strong>这条线路</strong>的覆盖值。运营商本身的 profile（ePDG、IMS、REGISTER 细节）
            存在数据库里，可在「SIM 卡管理 → 运营商 Profile」页完整编辑。
          </Alert>
          <FormControl fullWidth>
            <InputLabel>指定运营商 profile</InputLabel>
            <Select
              label="指定运营商 profile"
              value={draft.profile_id ?? ''}
              onChange={(event) => update('profile_id', event.target.value ? event.target.value : null)}
            >
              <MenuItem value=""><em>自动（按 SIM 卡 IMSI 匹配）</em></MenuItem>
              {profiles.map((profile) => (
                <MenuItem key={profile.profile_id} value={profile.profile_id}>
                  {profile.record.meta.brand || profile.profile_id} · {profile.plmn} · {profile.record.epdg.host}
                </MenuItem>
              ))}
            </Select>
          </FormControl>
          <Alert severity="info">
            留空时按 SIM 卡 IMSI 自动匹配运营商（先查数据库，未命中则按 3GPP 标准推算连接域名）。
            指定后强制使用所选数据库 profile 的全部连接参数。
          </Alert>
          <TextField
            label="专用 DNS 解析器"
            value={draft.dns_server}
            placeholder="例如 8.8.8.8 或 2001:4860:4860::8888"
            helperText="留空时依次使用系统 DNS、resolv.conf 和内置公共 DNS"
            onChange={(event) => update('dns_server', event.target.value)}
          />
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
