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
  type ExternalVowifiProfile,
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
  const [externalProfiles, setExternalProfiles] = useState<ExternalVowifiProfile[]>([])
  const [savingProfile, setSavingProfile] = useState(false)
  const [profileSaved, setProfileSaved] = useState(false)

  useEffect(() => {
    if (line) setDraft({ ...line.config })
    if (open) void api.getExternalVowifiProfiles().then((response) => setExternalProfiles(response.data ?? [])).catch(() => setExternalProfiles([]))
    setError(null)
    setProfileSaved(false)
  }, [line, open])

  const validationError = useMemo(() => {
    if (!draft) return null
    if (draft.dns_server && !/^[0-9a-f:.]+$/i.test(draft.dns_server.trim())) {
      return 'DNS 解析器必须填写 IPv4 或 IPv6 地址'
    }
    if (draft.epdg_host && /[\s/]/.test(draft.epdg_host)) return '自定义 ePDG 主机格式不正确'
    if (draft.epdg_port < 1 || draft.epdg_port > 65535) return 'ePDG 端口必须在 1-65535 之间'
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

  const saveAsExternalProfile = async () => {
    if (!line || !draft || validationError) return
    setSavingProfile(true)
    setError(null)
    try {
      const matched = await api.getVowifiProfile()
      const profile = matched.data?.profile
      const epdg = matched.data?.epdg
      if (!profile || !(draft.epdg_host.trim() || epdg?.host)) throw new Error('尚未匹配运营商 profile，或缺少 ePDG 主机')
      const response = await api.setExternalVowifiProfile({
        profile_id: line.matched_profile_id || profile.profile_id,
        mcc: profile.mcc,
        mnc: profile.mnc,
        epdg_host: draft.epdg_host.trim() || epdg?.host || '',
        epdg_port: draft.epdg_port,
        ip_stack: epdg?.ip_stack || 'ipv6',
        apn: epdg?.apn || 'ims',
        dns_server: draft.dns_server.trim() || epdg?.dns_server || null,
      })
      setExternalProfiles(response.data ?? [])
      setProfileSaved(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSavingProfile(false)
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
            现在存在数据库里，可在「SIM 卡管理 → 运营商 Profile」页完整编辑。
          </Alert>
          <FormControl fullWidth>
            <InputLabel>自定义 ePDG profile</InputLabel>
            <Select label="自定义 ePDG profile" value="" onChange={(event) => {
              const profile = externalProfiles.find((item) => item.profile_id === event.target.value)
              if (profile) setDraft((current) => current ? { ...current, epdg_host: profile.epdg_host, epdg_port: profile.epdg_port, dns_server: profile.dns_server || '' } : current)
            }}>
              <MenuItem value=""><em>不套用</em></MenuItem>
              {externalProfiles.map((profile) => <MenuItem key={profile.profile_id} value={profile.profile_id}>{profile.profile_id} · {profile.epdg_host}</MenuItem>)}
            </Select>
          </FormControl>
          <TextField
            label="自定义 ePDG 主机"
            value={draft.epdg_host}
            placeholder="epdg.epc.mnc001.mcc460.pub.3gppnetwork.org"
            helperText="留空时使用运营商 profile 中的 ePDG"
            onChange={(event) => update('epdg_host', event.target.value)}
          />
          <TextField
            type="number"
            label="ePDG IKE 端口"
            value={draft.epdg_port}
            slotProps={{ htmlInput: { min: 1, max: 65535 } }}
            onChange={(event) => update('epdg_port', Number(event.target.value))}
          />
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
          {profileSaved && <Alert severity="success">已写入设备的 vowifi-profiles.conf</Alert>}
          {error && <Alert severity="error">{error}</Alert>}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={() => void saveAsExternalProfile()} disabled={saving || savingProfile || Boolean(validationError)}>{savingProfile ? '写入中...' : '保存为自定义 profile'}</Button>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button variant="contained" onClick={() => void save()} disabled={saving || Boolean(validationError)}>
          {saving ? '保存中...' : '保存配置'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
