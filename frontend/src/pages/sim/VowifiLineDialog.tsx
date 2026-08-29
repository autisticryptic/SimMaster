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

function isValidIpv4(value: string): boolean {
  const octets = value.split('.')
  return octets.length === 4 && octets.every((octet) => {
    if (!/^(0|[1-9]\d{0,2})$/.test(octet)) return false
    const number = Number(octet)
    return number >= 0 && number <= 255
  })
}

function isValidIpv6(value: string): boolean {
  if (!value.includes(':') || !/^[0-9a-f:.]+$/i.test(value)) return false

  // Rust's IpAddr parser accepts IPv4-embedded IPv6 addresses. Replace the
  // dotted-quad tail with its two equivalent hextets before counting groups.
  let normalized = value
  if (value.includes('.')) {
    const lastColon = value.lastIndexOf(':')
    const embeddedIpv4 = value.slice(lastColon + 1)
    if (!isValidIpv4(embeddedIpv4)) return false
    normalized = `${value.slice(0, lastColon + 1)}0:0`
  }

  const compressionCount = (normalized.match(/::/g) ?? []).length
  if (compressionCount > 1) return false
  const [left = '', right = ''] = compressionCount === 1
    ? normalized.split('::')
    : [normalized, '']
  const validHextets = (part: string) =>
    part.length === 0 || part.split(':').every((hextet) => /^[0-9a-f]{1,4}$/i.test(hextet))
  if (!validHextets(left) || !validHextets(right)) return false

  const groupCount = (part: string) => (part.length === 0 ? 0 : part.split(':').length)
  const groups = groupCount(left) + groupCount(right)
  return compressionCount === 1 ? groups < 8 : groups === 8
}

function isValidDnsPort(value: string): boolean {
  if (!/^\d+$/.test(value)) return false
  const port = Number(value)
  return port >= 1 && port <= 65535
}

/** Accept bare IP addresses, IPv4:port, or bracketed IPv6:port. */
function isValidDnsServer(value: string): boolean {
  const server = value.trim()
  if (!server) return false

  if (server.startsWith('[')) {
    const match = /^\[([^\]]+)\]:(\d+)$/.exec(server)
    return Boolean(match && isValidIpv6(match[1]) && isValidDnsPort(match[2]))
  }
  if (isValidIpv4(server) || isValidIpv6(server)) return true

  const ipv4WithPort = /^([^:]+):(\d+)$/.exec(server)
  return Boolean(ipv4WithPort && isValidIpv4(ipv4WithPort[1]) && isValidDnsPort(ipv4WithPort[2]))
}

function parseDnsServers(value: string): string[] {
  return value
    .split(/\r?\n/)
    .map((server) => server.trim())
    .filter((server) => server.length > 0)
}


export default function VowifiLineDialog({ open, line, onClose, onSaved }: Props) {
  const [draft, setDraft] = useState<LineVowifiConfig | null>(null)
  const [saving, setSaving] = useState(false)
  const [override, setOverride] = useState<SimImsOverride | null>(null)
  const [dnsText, setDnsText] = useState('')
  const [overrideLoading, setOverrideLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (line) setDraft({ ...line.config })
    setOverride(null)
    setDnsText('')
    setError(null)
    if (!line || !open) return
    let active = true
    setOverrideLoading(true)
    void api.getImsOverride(line.line_id)
      .then((response) => {
        if (active && response.data) {
          const nextOverride = response.data.override_
          setOverride(nextOverride)
          setDnsText(nextOverride.ims_vowifi.dns?.join('\n') ?? '')
        }
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
    const dnsServers = parseDnsServers(dnsText)
    const invalidDnsIndex = dnsServers.findIndex((server) => !isValidDnsServer(server))
    if (invalidDnsIndex >= 0) return `自定义 ePDG DNS 格式不正确（第 ${invalidDnsIndex + 1} 行）`
    return null
  }, [draft, override, dnsText])

  const update = <K extends keyof LineVowifiConfig>(key: K, value: LineVowifiConfig[K]) => {
    setDraft((current) => current ? { ...current, [key]: value } : current)
  }

  const save = async () => {
    if (!line || !draft || !override || validationError) return
    setSaving(true)
    setError(null)
    try {
      const dnsServers = parseDnsServers(dnsText)
      await api.setImsOverride(line.line_id, {
        ...override,
        ims_vowifi: {
          ...override.ims_vowifi,
          dns: dnsServers.length > 0 ? dnsServers : null,
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
          <TextField
            fullWidth
            label="自定义 ePDG DNS（地址或地址:端口）"
            value={dnsText}
            disabled={overrideLoading || !override}
            multiline
            minRows={2}
            maxRows={6}
            placeholder={'1.1.1.1\n8.8.8.8:53\n[2001:4860:4860::8888]:5353'}
            helperText="每行一个 DNS 服务器：IPv4/IPv6 地址，或 IPv4:端口 / [IPv6]:端口；省略端口默认 53，按填写顺序依次尝试。留空则回退到运营商 profile DNS 或系统 DNS；修改后需重新连接 VoWiFi 生效"
            onChange={(event) => setDnsText(event.target.value)}
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
        <Button variant="contained" onClick={() => void save()} disabled={saving || overrideLoading || !override || Boolean(validationError)}>
          {saving ? '保存中...' : '保存配置'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
