import { useEffect, useMemo, useState } from 'react'
import {
  Alert, Box, Button, CircularProgress, Dialog, DialogActions, DialogContent, DialogTitle,
  FormControl, FormControlLabel, IconButton, InputLabel, MenuItem, Select, Stack, Switch,
  TextField, Tooltip, Typography,
} from '@mui/material'
import { ArrowDownward, ArrowUpward } from '@mui/icons-material'
import {
  api,
  type CarrierProfileSummary,
  type LineVowifiConfig,
  type VolteProfileCandidate,
  type VolteProfileSource,
  type VowifiLineConfigResponse,
  type VowifiProxyMode,
  type SimImsOverride,
} from '../../api/current'
import { shortLineId } from '../../components/modemLineFormat'
import { loadCarrierProfileSummaries } from './carrierProfileSummaryCache'

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

const sourceLabels: Record<VolteProfileSource, string> = {
  database: '用户数据库',
  carrier_catalog: '下载的只读数据库',
  derived: '自动派生配置',
}

function cloneAttempts(attempts: VolteProfileCandidate[]) {
  return attempts.map((attempt) => ({ ...attempt, profile_id: attempt.profile_id || null }))
}

function profilesForSource(profiles: CarrierProfileSummary[], source: VolteProfileSource) {
  const origin = source === 'database' ? 'database' : source === 'carrier_catalog' ? 'carrier_catalog' : null
  return origin ? profiles.filter((profile) => profile.origin === origin && profile.vowifi_ready) : []
}


export default function VowifiLineDialog({ open, line, onClose, onSaved }: Props) {
  const [draft, setDraft] = useState<LineVowifiConfig | null>(null)
  const [saving, setSaving] = useState(false)
  const [override, setOverride] = useState<SimImsOverride | null>(null)
  const [profiles, setProfiles] = useState<CarrierProfileSummary[]>([])
  const [overrideLoading, setOverrideLoading] = useState(false)
  const [profilesLoading, setProfilesLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (line) setDraft({ ...line.config })
    setOverride(null)
    setProfiles([])
    setError(null)
    if (!line || !open) return
    let active = true
    setOverrideLoading(true)
    setProfilesLoading(true)
    void Promise.all([api.getImsOverride(line.line_id), loadCarrierProfileSummaries()])
      .then(([response, loadedProfiles]) => {
        if (!active) return
        if (response.data) setOverride(response.data.override_)
        setProfiles(loadedProfiles)
      })
      .catch((err) => active && setError(err instanceof Error ? err.message : String(err)))
      .finally(() => {
        if (!active) return
        setOverrideLoading(false)
        setProfilesLoading(false)
      })
    return () => { active = false }
  }, [line, open])

  const validationError = useMemo(() => {
    if (!draft) return null
    if (draft.proxy_mode !== 'direct' && !draft.proxy_endpoint.trim()) return '所选代理模式需要填写代理端点'
    const customImsi = override?.ims_vowifi.custom_imsi?.trim() ?? ''
    if (override?.ims_vowifi.spoof_imsi && !customImsi) return '启用伪装 IMSI 后必须填写 IMSI'
    if (customImsi && !/^\d{5,16}$/.test(customImsi)) return 'IMSI 必须是 5-16 位数字'
    if (draft.profile_selection.attempts.length !== 3) return '必须保留恰好三个 Profile 尝试槽位'
    if (draft.profile_selection.attempts.some((attempt) => attempt.source === 'derived' && attempt.profile_id)) {
      return '自动派生配置不能指定 Profile ID'
    }
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
          dns: null,
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

  const patchAttempt = (index: number, patch: Partial<VolteProfileCandidate>) => {
    update('profile_selection', {
      attempts: draft.profile_selection.attempts.map((attempt, offset) => offset === index
        ? { ...attempt, ...patch }
        : attempt),
    })
  }

  const moveAttempt = (index: number, delta: -1 | 1) => {
    const target = index + delta
    if (target < 0 || target >= draft.profile_selection.attempts.length) return
    const attempts = cloneAttempts(draft.profile_selection.attempts)
    ;[attempts[index], attempts[target]] = [attempts[target], attempts[index]]
    update('profile_selection', { attempts })
  }

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="md">
      <DialogTitle>WiFi Calling 配置 · {shortLineId(line.line_id)}</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <Alert severity="info">
            这里配置<strong>这条线路</strong>的代理出口和 Profile 重试顺序。ePDG、DNS 与 IMS 参数只从运营商
            Profile 读取；详细内容请在“运营商 IMS Profile”中维护。
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

          <Box>
            <Stack direction="row" alignItems="center" justifyContent="space-between" mb={1}>
              <Box>
                <Typography variant="subtitle2" fontWeight={700}>Profile 读取与重试顺序</Typography>
                <Typography variant="caption" color="text.secondary">
                  每个槽位尝试一次；失败后按顺序切换，三个槽位耗尽后停止自动重连。
                </Typography>
              </Box>
              {profilesLoading && <CircularProgress size={18} />}
            </Stack>
            <Stack spacing={1}>
              {draft.profile_selection.attempts.map((attempt, index) => {
                const options = profilesForSource(profiles, attempt.source)
                const explicitProfile = attempt.profile_id
                  ? options.find((profile) => profile.profile_id === attempt.profile_id)
                  : null
                return (
                  <Box key={`${index}-${attempt.source}`} sx={{ p: 1.25, border: 1, borderColor: 'divider', borderRadius: 1 }}>
                    <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems={{ sm: 'center' }}>
                      <Stack direction="row" alignItems="center" minWidth={112}>
                        <Typography variant="body2" fontWeight={700}>第 {index + 1} 次</Typography>
                        <Tooltip title="上移">
                          <span>
                            <IconButton size="small" disabled={index === 0} onClick={() => moveAttempt(index, -1)}>
                              <ArrowUpward fontSize="small" />
                            </IconButton>
                          </span>
                        </Tooltip>
                        <Tooltip title="下移">
                          <span>
                            <IconButton size="small" disabled={index === draft.profile_selection.attempts.length - 1} onClick={() => moveAttempt(index, 1)}>
                              <ArrowDownward fontSize="small" />
                            </IconButton>
                          </span>
                        </Tooltip>
                      </Stack>
                      <FormControl size="small" sx={{ minWidth: 180 }}>
                        <InputLabel>Profile 来源</InputLabel>
                        <Select
                          value={attempt.source}
                          label="Profile 来源"
                          onChange={(event) => patchAttempt(index, {
                            source: event.target.value as VolteProfileSource,
                            profile_id: null,
                          })}
                        >
                          {(Object.keys(sourceLabels) as VolteProfileSource[]).map((source) => (
                            <MenuItem key={source} value={source}>{sourceLabels[source]}</MenuItem>
                          ))}
                        </Select>
                      </FormControl>
                      <FormControl size="small" fullWidth disabled={attempt.source === 'derived' || profilesLoading}>
                        <InputLabel>自动匹配 / 指定 Profile</InputLabel>
                        <Select
                          value={attempt.profile_id ?? ''}
                          label="自动匹配 / 指定 Profile"
                          onChange={(event) => patchAttempt(index, { profile_id: event.target.value || null })}
                        >
                          <MenuItem value="">按 IMSI / Home PLMN 自动匹配</MenuItem>
                          {attempt.profile_id && !explicitProfile && (
                            <MenuItem value={attempt.profile_id} disabled>{attempt.profile_id} · 已不存在或不支持 VoWiFi</MenuItem>
                          )}
                          {options.map((profile) => (
                            <MenuItem key={`${profile.origin}:${profile.profile_id}`} value={profile.profile_id}>
                              {profile.brand || profile.operator_legal_name || profile.profile_id} · PLMN {profile.plmn}
                            </MenuItem>
                          ))}
                        </Select>
                      </FormControl>
                    </Stack>
                    {attempt.source === 'derived' && (
                      <Typography variant="caption" color="text.secondary">根据当前 SIM 的 Home PLMN 生成标准 ePDG/IMS 配置。</Typography>
                    )}
                  </Box>
                )
              })}
            </Stack>
          </Box>

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
        <Button variant="contained" onClick={() => void save()} disabled={saving || overrideLoading || profilesLoading || !override || Boolean(validationError)}>
          {saving ? '保存中...' : '保存配置'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
