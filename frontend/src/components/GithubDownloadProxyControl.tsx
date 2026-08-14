import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  CircularProgress,
  FormControl,
  InputLabel,
  MenuItem,
  Select,
  Stack,
  TextField,
  Typography,
} from '@mui/material'
import { Save } from '@mui/icons-material'
import { api } from '../api/current'
import type { GithubDownloadProxyConfig } from '../api/types'

const PROXY_UPDATED_EVENT = 'simadmin-github-download-proxy-updated'
const DIRECT_PRESET = 'direct'
const CUSTOM_PRESET = 'custom'
const PROXY_PRESETS = [
  { label: 'gh-proxy.com（默认）', value: 'https://gh-proxy.com/' },
  { label: 'ghproxy.net', value: 'https://ghproxy.net/' },
  { label: 'githubproxy.cc', value: 'https://githubproxy.cc/' },
] as const

const DEFAULT_CONFIG: GithubDownloadProxyConfig = {
  enabled: true,
  proxy_prefix: PROXY_PRESETS[0].value,
}

function normalizePrefix(value: string) {
  const trimmed = value.trim()
  if (!trimmed) return ''
  return trimmed.endsWith('/') ? trimmed : `${trimmed}/`
}

function presetFor(config: GithubDownloadProxyConfig) {
  if (!config.enabled) return DIRECT_PRESET
  return PROXY_PRESETS.some((preset) => preset.value === config.proxy_prefix)
    ? config.proxy_prefix
    : CUSTOM_PRESET
}

export default function GithubDownloadProxyControl({ compact = false }: { compact?: boolean }) {
  const [config, setConfig] = useState<GithubDownloadProxyConfig>(DEFAULT_CONFIG)
  const [customPrefix, setCustomPrefix] = useState('')
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const preset = useMemo(() => presetFor(config), [config])

  const applyLoadedConfig = useCallback((next: GithubDownloadProxyConfig) => {
    setConfig(next)
    if (presetFor(next) === CUSTOM_PRESET) setCustomPrefix(next.proxy_prefix)
  }, [])

  const load = useCallback(async () => {
    try {
      const response = await api.getGithubDownloadProxy()
      if (response.data) applyLoadedConfig(response.data)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [applyLoadedConfig])

  useEffect(() => {
    void load()
    const sync = (event: Event) => {
      const next = (event as CustomEvent<GithubDownloadProxyConfig>).detail
      if (next) applyLoadedConfig(next)
    }
    window.addEventListener(PROXY_UPDATED_EVENT, sync)
    return () => window.removeEventListener(PROXY_UPDATED_EVENT, sync)
  }, [applyLoadedConfig, load])

  const save = async (next: GithubDownloadProxyConfig) => {
    const normalized = { ...next, proxy_prefix: normalizePrefix(next.proxy_prefix) }
    if (normalized.enabled && !normalized.proxy_prefix) {
      setError('启用 GitHub 下载加速时必须填写加速节点')
      return
    }
    setSaving(true)
    setError(null)
    try {
      const response = await api.setGithubDownloadProxy(normalized)
      const saved = response.data ?? normalized
      applyLoadedConfig(saved)
      window.dispatchEvent(new CustomEvent(PROXY_UPDATED_EVENT, { detail: saved }))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  if (loading) {
    return <Box display="flex" alignItems="center" gap={1}><CircularProgress size={16} /><Typography variant="caption">读取加速设置</Typography></Box>
  }

  return (
    <Stack spacing={1.25}>
      <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1.25} alignItems={{ sm: 'center' }}>
        <FormControl size="small" sx={{ minWidth: compact ? 190 : 220 }}>
          <InputLabel id="github-download-proxy-preset">下载线路</InputLabel>
          <Select
            labelId="github-download-proxy-preset"
            label="下载线路"
            value={preset}
            disabled={saving}
            onChange={(event) => {
              const value = event.target.value
              if (value === DIRECT_PRESET) {
                void save({ ...config, enabled: false })
                return
              }
              if (value === CUSTOM_PRESET) {
                setCustomPrefix(preset === CUSTOM_PRESET ? config.proxy_prefix : '')
                setConfig((current) => ({ ...current, enabled: true, proxy_prefix: '' }))
                return
              }
              void save({ enabled: true, proxy_prefix: value })
            }}
          >
            <MenuItem value={DIRECT_PRESET}>GitHub 直连</MenuItem>
            {PROXY_PRESETS.map((option) => <MenuItem key={option.value} value={option.value}>{option.label}</MenuItem>)}
            <MenuItem value={CUSTOM_PRESET}>自定义加速节点</MenuItem>
          </Select>
        </FormControl>
        {config.enabled && preset === CUSTOM_PRESET && (
          <Box display="flex" alignItems="center" gap={1} minWidth={0} flex={1}>
            <TextField
              size="small"
              fullWidth
              label="自定义加速节点"
              value={customPrefix}
              disabled={saving}
              placeholder="https://proxy.example.com/"
              onChange={(event) => setCustomPrefix(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') void save({ enabled: true, proxy_prefix: customPrefix })
              }}
            />
            <Button
              size="small"
              variant="outlined"
              startIcon={saving ? <CircularProgress size={14} /> : <Save />}
              disabled={saving || !customPrefix.trim()}
              onClick={() => void save({ enabled: true, proxy_prefix: customPrefix })}
            >
              保存
            </Button>
          </Box>
        )}
      </Stack>
      {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
      {!config.enabled && <Typography variant="caption" color="text.secondary">OTA、运营商 Profile 和 lpac 下载将直接连接 GitHub。</Typography>}
    </Stack>
  )
}
