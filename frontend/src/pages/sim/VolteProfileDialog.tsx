import { useEffect, useMemo, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Chip,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormControl,
  IconButton,
  InputLabel,
  MenuItem,
  Select,
  Stack,
  Typography,
} from '@mui/material'
import { ArrowDownward, ArrowUpward } from '@mui/icons-material'
import {
  api,
  type StoredCarrierProfile,
  type VolteProfileCandidate,
  type VolteProfileSelectionResponse,
  type VolteProfileSource,
} from '../../api/current'
import { shortLineId } from '../../components/modemLineFormat'

interface Props {
  open: boolean
  lineId: string | null
  onClose: () => void
  onSaved: (response: VolteProfileSelectionResponse) => void
}

const sourceLabels: Record<VolteProfileSource, string> = {
  database: '用户数据库',
  carrier_catalog: '下载的只读数据库',
  derived: '自动派生配置',
}

function cloneAttempts(attempts: VolteProfileCandidate[]) {
  return attempts.map((attempt) => ({ ...attempt, profile_id: attempt.profile_id || null }))
}

function profilesForSource(profiles: StoredCarrierProfile[], source: VolteProfileSource) {
  const origin = source === 'database' ? 'database' : source === 'carrier_catalog' ? 'carrier_catalog' : null
  return origin ? profiles.filter((profile) => profile.origin === origin) : []
}

export default function VolteProfileDialog({ open, lineId, onClose, onSaved }: Props) {
  const [data, setData] = useState<VolteProfileSelectionResponse | null>(null)
  const [attempts, setAttempts] = useState<VolteProfileCandidate[]>([])
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open || !lineId) return
    let active = true
    setLoading(true)
    setError(null)
    setData(null)
    setAttempts([])
    void api.getVolteProfileSelection(lineId)
      .then((response) => {
        if (!active) return
        if (!response.data) {
          setError('后端未返回 VoLTE Profile 配置')
          return
        }
        setData(response.data)
        setAttempts(cloneAttempts(response.data.selection.attempts))
      })
      .catch((err) => active && setError(err instanceof Error ? err.message : String(err)))
      .finally(() => active && setLoading(false))
    return () => { active = false }
  }, [lineId, open])

  const validationError = useMemo(() => {
    if (attempts.length !== 3) return '必须保留恰好三个尝试槽位'
    if (attempts.some((attempt) => attempt.source === 'derived' && attempt.profile_id)) {
      return '自动派生配置不能指定 Profile ID'
    }
    return null
  }, [attempts])

  const patchAttempt = (index: number, patch: Partial<VolteProfileCandidate>) => {
    setAttempts((current) => current.map((attempt, offset) => offset === index
      ? { ...attempt, ...patch }
      : attempt))
  }

  const moveAttempt = (index: number, delta: -1 | 1) => {
    setAttempts((current) => {
      const target = index + delta
      if (target < 0 || target >= current.length) return current
      const next = [...current]
      ;[next[index], next[target]] = [next[target], next[index]]
      return next
    })
  }

  const save = async () => {
    if (!lineId || validationError) return
    setSaving(true)
    setError(null)
    try {
      const response = await api.setVolteProfileSelection(lineId, { attempts })
      if (!response.data) throw new Error('后端未返回保存后的 VoLTE Profile 配置')
      setData(response.data)
      setAttempts(cloneAttempts(response.data.selection.attempts))
      onSaved(response.data)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="md">
      <DialogTitle>VoLTE Profile 编排 · {lineId ? shortLineId(lineId) : ''}</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <Alert severity="info">
            这是每条物理基带/读卡器独立设置，不是全局设置。每条线路独立保存三个尝试槽位。IMS 恢复会严格按从上到下的顺序执行；某个数据库来源不可用时，该槽位会改用派生配置，但不会被去重。
          </Alert>
          {error && <Alert severity="error">{error}</Alert>}
          {validationError && <Alert severity="warning">{validationError}</Alert>}
          {loading && <Typography color="text.secondary">正在加载 Profile 配置…</Typography>}

          {!loading && data && attempts.map((attempt, index) => {
            const options = profilesForSource(data.profiles, attempt.source)
            const readyOptions = options.filter((profile) => profile.volte_ready)
            const explicitProfile = attempt.profile_id
              ? options.find((profile) => profile.profile_id === attempt.profile_id)
              : null
            const fallbackReason = attempt.source === 'derived'
              ? null
              : attempt.profile_id && !explicitProfile
                ? `指定的 Profile ${attempt.profile_id} 已不存在，本槽位将使用派生配置兜底。`
                : explicitProfile && !explicitProfile.volte_ready
                  ? `指定的 Profile ${attempt.profile_id} 没有 LTE/EPC 投影，本槽位将使用派生配置兜底。`
                  : readyOptions.length === 0
                    ? '该来源当前没有 LTE-ready Profile，本槽位将使用派生配置兜底。'
                    : null
            return (
              <Box key={`${index}-${attempt.source}`} sx={{ p: 1.5, border: 1, borderColor: 'divider', borderRadius: 1.5 }}>
                <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1.25} alignItems={{ sm: 'center' }}>
                  <Stack direction="row" alignItems="center" minWidth={110}>
                    <Typography fontWeight={700}>第 {index + 1} 次</Typography>
                    <IconButton size="small" disabled={index === 0} onClick={() => moveAttempt(index, -1)} aria-label="上移">
                      <ArrowUpward fontSize="small" />
                    </IconButton>
                    <IconButton size="small" disabled={index === attempts.length - 1} onClick={() => moveAttempt(index, 1)} aria-label="下移">
                      <ArrowDownward fontSize="small" />
                    </IconButton>
                  </Stack>
                  <FormControl size="small" sx={{ minWidth: 190 }}>
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
                  <FormControl size="small" fullWidth disabled={attempt.source === 'derived'}>
                    <InputLabel>自动匹配 / 指定 Profile</InputLabel>
                    <Select
                      value={attempt.profile_id ?? ''}
                      label="自动匹配 / 指定 Profile"
                      onChange={(event) => patchAttempt(index, { profile_id: event.target.value || null })}
                    >
                      <MenuItem value="">按 IMSI / Home PLMN 自动匹配</MenuItem>
                      {attempt.profile_id && !explicitProfile && (
                        <MenuItem value={attempt.profile_id} disabled>
                          {attempt.profile_id} · 已不存在
                        </MenuItem>
                      )}
                      {options.map((profile) => {
                        const name = profile.record.meta.brand || profile.record.meta.operator_legal_name
                        return (
                          <MenuItem
                            key={`${profile.origin}:${profile.profile_id}`}
                            value={profile.profile_id}
                            disabled={!profile.volte_ready}
                          >
                            {profile.profile_id}{name ? ` · ${name}` : ''} · PLMN {profile.plmn} · {profile.volte_ready ? 'LTE 可用' : 'LTE 不可用'} · {profile.source}
                          </MenuItem>
                        )
                      })}
                    </Select>
                  </FormControl>
                </Stack>
                {attempt.source === 'derived' && (
                  <Typography variant="caption" color="text.secondary">始终根据当前 SIM/Home PLMN 派生 LTE/EPC IMS 配置。</Typography>
                )}
                {fallbackReason && (
                  <Alert severity="warning" sx={{ mt: 1 }}>{fallbackReason}</Alert>
                )}
              </Box>
            )
          })}

          {data?.legacy_pinned_profile_id && (
            <Alert severity="warning">
              检测到旧 SIM 覆盖中的 Profile：<strong>{data.legacy_pinned_profile_id}</strong>。线路槽位已显式指定 ID 时优先使用线路配置；否则仅在来源一致时使用此兼容 pin。
            </Alert>
          )}

          {data && (
            <Box>
              <Typography variant="subtitle2" mb={1}>当前解析与最近尝试</Typography>
              <Stack direction="row" spacing={1} flexWrap="wrap" useFlexGap mb={1}>
                <Chip size="small" label={`实际来源：${data.runtime.profile_source ? sourceLabels[data.runtime.profile_source] : '未解析'}`} />
                <Chip size="small" label={`实际 Profile：${data.runtime.profile_id || '—'}`} />
                {data.runtime.profile_candidate_index && <Chip size="small" label={`当前第 ${data.runtime.profile_candidate_index} 次`} />}
                {data.runtime.profile_candidate_source && (
                  <Chip size="small" label={`请求来源：${sourceLabels[data.runtime.profile_candidate_source]}`} />
                )}
                {data.runtime.profile_candidate_profile_id && (
                  <Chip size="small" label={`请求 Profile：${data.runtime.profile_candidate_profile_id}`} />
                )}
              </Stack>
              {data.runtime.profile_fallback_reason && (
                <Alert severity="warning" sx={{ mb: 1 }}>
                  当前槽位使用了派生兜底：{data.runtime.profile_fallback_reason}
                </Alert>
              )}
              <Stack spacing={0.75}>
                {(data.runtime.profile_attempt_results ?? []).slice(-3).map((result) => (
                  <Box key={`${result.index}-${result.at}`} display="flex" gap={1} alignItems="center" flexWrap="wrap">
                    <Chip size="small" color={result.outcome === 'succeeded' ? 'success' : 'error'} label={`第 ${result.index} 次 · ${result.outcome === 'succeeded' ? '成功' : '失败'}`} />
                    <Typography variant="caption" color="text.secondary">
                      {sourceLabels[result.requested_source]}{result.requested_profile_id ? ` / ${result.requested_profile_id}` : ' / 自动'}
                      {' → '}
                      {result.effective_source ? sourceLabels[result.effective_source] : '未解析'}{result.effective_profile_id ? ` / ${result.effective_profile_id}` : ''}
                      {result.fallback_reason ? ` · 兜底：${result.fallback_reason}` : ''}
                      {result.error_code ? ` · 错误：${result.error_code}` : ''}
                    </Typography>
                  </Box>
                ))}
                {!data.runtime.profile_attempt_results?.length && <Typography variant="caption" color="text.secondary">尚无候选尝试记录。</Typography>}
              </Stack>
            </Box>
          )}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button variant="contained" onClick={() => void save()} disabled={loading || saving || !data || Boolean(validationError)}>保存并从第 1 个槽位重连</Button>
      </DialogActions>
    </Dialog>
  )
}
