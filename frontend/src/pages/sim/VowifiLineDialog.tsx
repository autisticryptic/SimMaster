import { useEffect, useMemo, useState } from 'react'
import {
  Alert, Button, Dialog, DialogActions, DialogContent, DialogTitle,
  FormControl, FormControlLabel, InputLabel, MenuItem, Select, Stack, Switch, TextField,
} from '@mui/material'
import {
  api,
  type LineVowifiConfig,
  type VowifiLineConfigResponse,
  type VowifiProxyMode,
  type SimImsOverride,
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
  const [override, setOverride] = useState<SimImsOverride | null>(null)
  const [overrideLoading, setOverrideLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (line) setDraft({ ...line.config })
    setOverride(null)
    setError(null)
    if (!line || !open) return
    let active = true
    setOverrideLoading(true)
    void api.getImsOverride(line.line_id)
      .then((response) => {
        if (active && response.data) setOverride(response.data.override_)
      })
      .catch((err) => {
        if (active) setError(err instanceof Error ? err.message : String(err))
      })
      .finally(() => {
        if (active) setOverrideLoading(false)
      })
    return () => { active = false }
  }, [line, open])

  const validationError = useMemo(() => {
    if (!draft) return null
    if (draft.proxy_mode !== 'direct' && !draft.proxy_endpoint.trim()) return '所选代理模式需要填写代理端点'
    const customImsi = override?.ims_vowifi.custom_imsi?.trim() ?? ''
    if (override?.ims_vowifi.spoof_imsi && !customImsi) return '启用伪装 IMSI 后必须填写 IMSI'
    if (customImsi && !/^\d{5,16}$/.test(customImsi)) return 'IMSI 必须是 5-16 位数字'
    return null
  }, [draft, override])

  const update = <K extends keyof LineVowifiConfig>(key: K, value: LineVowifiConfig[K]) => {
    setDraft((current) => current ? { ...current, [key]: value } : current)
  }

  const save = async () => {
    if (!line || !draft || !override || validationError) return
    setSaving(true)
    setError(null)
    try {
      await api.setImsOverride(line.line_id, {
        ...override,
        ims_vowifi: {
          ...override.ims_vowifi,
          custom_imsi: override.ims_vowifi.spoof_imsi
            ? override.ims_vowifi.custom_imsi?.trim() || null
            : null,
        },
      })
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

  const patchVowifiOverride = (next: Partial<SimImsOverride['ims_vowifi']>) => {
    setOverride((current) => current ? {
      ...current,
      ims_vowifi: { ...current.ims_vowifi, ...next },
    } : current)
  }

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="sm">
      <DialogTitle>WiFi Calling 配置 · {shortLineId(line.line_id)}</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <Alert severity="info">
            这里仅配置<strong>这条线路</strong>的运行方式。运营商 profile、DNS、ePDG 和 IMS 覆写跟随 SIM 卡保存，
            换卡或移动 SIM 时不会与物理线路配置混用。
          </Alert>
          <Stack spacing={1}>
            <FormControlLabel
              control={
                <Switch
                  checked={override?.ims_vowifi.spoof_imsi ?? false}
                  disabled={overrideLoading || !override}
                  onChange={(_, spoof_imsi) => patchVowifiOverride({
                    spoof_imsi,
                    custom_imsi: spoof_imsi ? override?.ims_vowifi.custom_imsi ?? null : null,
                  })}
                />
              }
              label="伪装 IMSI"
            />
            <TextField
              label="伪装 IMSI"
              value={override?.ims_vowifi.custom_imsi ?? ''}
              disabled={overrideLoading || !override?.ims_vowifi.spoof_imsi}
              placeholder="460001234567890"
              helperText="用于 VoWiFi 的运营商匹配、IKE NAI 与 IMS 注册身份；SIM AKA 仍由当前卡片完成，重连后生效"
              inputProps={{ inputMode: 'numeric', maxLength: 16 }}
              onChange={(event) => patchVowifiOverride({ custom_imsi: event.target.value })}
            />
          </Stack>
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
        <Button variant="contained" onClick={() => void save()} disabled={saving || overrideLoading || !override || Boolean(validationError)}>
          {saving ? '保存中...' : '保存配置'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
