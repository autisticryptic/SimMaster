import { useCallback, useEffect, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Card,
  CardContent,
  CardHeader,
  Chip,
  CircularProgress,
  Divider,
  FormControl,
  InputLabel,
  MenuItem,
  Select,
  Snackbar,
  Stack,
  Switch,
  TextField,
  Typography,
} from '@mui/material'
import Grid from '@mui/material/Grid'
import { Download, Refresh, Save, Shield, Storage } from '@mui/icons-material'
import { api } from '../api/current'
import type {
  DiagnosticLogConfig,
  DiagnosticLogSeverity,
  DiagnosticLogStatus,
} from '../api/types'
import ErrorSnackbar from '../components/ErrorSnackbar'

const SEVERITY_OPTIONS: Array<{ value: DiagnosticLogSeverity, label: string, hint: string }> = [
  { value: 'debug', label: '调试 (Debug)', hint: '记录全部事件，排查疑难问题时使用' },
  { value: 'info', label: '信息 (Info)', hint: '记录正常状态变化与失败，推荐' },
  { value: 'warn', label: '警告 (Warn)', hint: '只记录带错误串的事件' },
  { value: 'error', label: '错误 (Error)', hint: '只记录明确失败的事件' },
]

const RETENTION_MIN = 1
const RETENTION_MAX = 365
const SIZE_MIN = 1
const SIZE_MAX = 4096

function formatBytes(bytes: number) {
  if (bytes <= 0) return '0 B'
  const units = ['B', 'KB', 'MB', 'GB']
  const exponent = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1)
  const value = bytes / 1024 ** exponent
  return `${value >= 10 || exponent === 0 ? Math.round(value) : value.toFixed(1)} ${units[exponent]}`
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

function configsEqual(a: DiagnosticLogConfig, b: DiagnosticLogConfig) {
  return JSON.stringify(a) === JSON.stringify(b)
}

/**
 * Read a bounded integer out of a text field without fighting the user mid-edit.
 *
 * Clamping on every keystroke makes an empty field impossible to type into, so
 * blank input is preserved as-is and only committed values are clamped.
 */
function parseBoundedInt(value: string, min: number, max: number, fallback: number) {
  const digits = value.replace(/\D/g, '')
  if (!digits) return null
  return Math.min(Math.max(Number(digits), min), max) || fallback
}

export default function DiagnosticLogPage() {
  const [config, setConfig] = useState<DiagnosticLogConfig | null>(null)
  const [savedConfig, setSavedConfig] = useState<DiagnosticLogConfig | null>(null)
  const [status, setStatus] = useState<DiagnosticLogStatus | null>(null)
  const [retentionInput, setRetentionInput] = useState('')
  const [sizeInput, setSizeInput] = useState('')
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [downloading, setDownloading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const applyResponse = useCallback((next: { config: DiagnosticLogConfig, status: DiagnosticLogStatus }) => {
    setConfig(next.config)
    setSavedConfig(next.config)
    setStatus(next.status)
    setRetentionInput(String(next.config.retention_days))
    setSizeInput(String(next.config.max_total_mb))
  }, [])

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const response = await api.getDiagnosticLog()
      if (response.data) applyResponse(response.data)
    } catch (err) {
      setError(errorText(err))
    } finally {
      setLoading(false)
    }
  }, [applyResponse])

  useEffect(() => { void load() }, [load])

  const patch = (next: Partial<DiagnosticLogConfig>) => {
    setConfig((current) => (current ? { ...current, ...next } : current))
  }

  const save = async () => {
    if (!config) return
    setSaving(true)
    setError(null)
    try {
      const response = await api.setDiagnosticLog(config)
      if (response.data) applyResponse(response.data)
      setSuccess('诊断日志设置已保存')
    } catch (err) {
      setError(errorText(err))
    } finally {
      setSaving(false)
    }
  }

  const download = async () => {
    setDownloading(true)
    setError(null)
    try {
      const blob = await api.downloadDiagnosticLog()
      const url = URL.createObjectURL(blob)
      const link = document.createElement('a')
      link.href = url
      link.download = `simadmin-diagnostics-${new Date().toISOString().slice(0, 10)}.log`
      document.body.appendChild(link)
      link.click()
      link.remove()
      // Revoking immediately can cancel the download in some browsers; the next
      // tick is enough for the click to have been dispatched.
      window.setTimeout(() => URL.revokeObjectURL(url), 1000)
    } catch (err) {
      setError(errorText(err))
    } finally {
      setDownloading(false)
    }
  }

  if (loading || !config || !savedConfig) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" minHeight="60vh">
        <CircularProgress />
      </Box>
    )
  }

  const dirty = !configsEqual(config, savedConfig)
  const hasRecords = (status?.file_count ?? 0) > 0

  return (
    <Box>
      <Box mb={2}>
        <Typography variant="h5" gutterBottom fontWeight={700}>诊断日志</Typography>
        <Typography variant="body2" color="text.secondary">
          界面上的线路活动日志只保留最近 20 条并会截断长错误串；完整历史与未截断的原始错误记录在设备本地日志文件里。
        </Typography>
      </Box>

      <ErrorSnackbar error={error} onClose={() => setError(null)} />
      {success && (
        <Snackbar
          open
          autoHideDuration={3000}
          onClose={() => setSuccess(null)}
          anchorOrigin={{ vertical: 'top', horizontal: 'center' }}
        >
          <Alert severity="success" variant="filled" onClose={() => setSuccess(null)}>
            {success}
          </Alert>
        </Snackbar>
      )}

      <Stack spacing={3}>
        <Card>
          <CardHeader
            avatar={<Storage color="primary" />}
            title="日志文件"
            titleTypographyProps={{ variant: 'h6', fontWeight: 600 }}
            action={
              <Chip
                size="small"
                label={config.enabled ? '记录中' : '已停止'}
                color={config.enabled ? 'success' : 'default'}
                variant={config.enabled ? 'outlined' : undefined}
              />
            }
          />
          <CardContent>
            <Grid container spacing={2}>
              <Grid size={{ xs: 12, sm: 6 }}>
                <Typography variant="caption" color="text.secondary">存放目录</Typography>
                <Typography variant="body2" sx={{ mt: 0.25, wordBreak: 'break-all', fontFamily: 'monospace' }}>
                  {status?.directory || '未确定'}
                </Typography>
              </Grid>
              <Grid size={{ xs: 6, sm: 3 }}>
                <Typography variant="caption" color="text.secondary">占用体积</Typography>
                <Typography variant="body2" sx={{ mt: 0.25 }}>
                  {formatBytes(status?.total_bytes ?? 0)} · {status?.file_count ?? 0} 个文件
                </Typography>
              </Grid>
              <Grid size={{ xs: 6, sm: 3 }}>
                <Typography variant="caption" color="text.secondary">最早记录</Typography>
                <Typography variant="body2" sx={{ mt: 0.25 }}>{status?.earliest_date || '暂无'}</Typography>
              </Grid>
            </Grid>

            {(status?.dropped_records ?? 0) > 0 && (
              <Alert severity="warning" sx={{ mt: 2 }}>
                有 {status?.dropped_records} 条记录因写入速度跟不上被丢弃，日志存在缺口。持续出现说明磁盘写入过慢。
              </Alert>
            )}

            <Box display="flex" gap={1.5} mt={2.5} flexWrap="wrap">
              <Button
                variant="contained"
                startIcon={downloading ? <CircularProgress size={16} color="inherit" /> : <Download />}
                onClick={() => void download()}
                disabled={downloading || !hasRecords}
              >
                下载日志
              </Button>
              <Button
                variant="outlined"
                startIcon={<Refresh />}
                onClick={() => void load()}
                disabled={loading}
              >
                刷新
              </Button>
            </Box>
            {!hasRecords && (
              <Typography variant="caption" color="text.secondary" display="block" mt={1}>
                尚未产生日志文件，发生一次线路事件后即可下载。
              </Typography>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader
            avatar={<Shield color="primary" />}
            title="记录与保留"
            titleTypographyProps={{ variant: 'h6', fontWeight: 600 }}
          />
          <CardContent>
            <Box
              sx={{
                p: 2,
                border: '1px solid',
                borderColor: 'divider',
                borderRadius: 1.5,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: 2,
              }}
            >
              <Box minWidth={0}>
                <Typography fontWeight={700}>写入诊断日志</Typography>
                <Typography variant="body2" color="text.secondary">
                  关闭后不再写入新记录，已有文件保留。
                </Typography>
              </Box>
              <Switch
                checked={config.enabled}
                onChange={(event) => patch({ enabled: event.target.checked })}
              />
            </Box>

            <Box
              sx={{
                mt: 2,
                p: 2,
                border: '1px solid',
                borderColor: 'divider',
                borderRadius: 1.5,
                display: 'flex',
                alignItems: 'center',
                justifyContent: 'space-between',
                gap: 2,
              }}
            >
              <Box minWidth={0}>
                <Typography fontWeight={700}>脱敏敏感信息</Typography>
                <Typography variant="body2" color="text.secondary">
                  遮蔽 IMSI、手机号、短信正文与 P-CSCF 地址。错误码、命令与 stderr 始终完整保留，不受影响。
                </Typography>
              </Box>
              <Switch
                checked={config.redact_sensitive}
                onChange={(event) => patch({ redact_sensitive: event.target.checked })}
              />
            </Box>

            {!config.redact_sensitive && (
              <Alert severity="warning" sx={{ mt: 2 }}>
                关闭脱敏后，日志会包含 IMSI、手机号与短信正文明文。任何能登录本系统的人都可以下载该文件。
              </Alert>
            )}

            <Divider sx={{ my: 3 }} />

            <Grid container spacing={2}>
              <Grid size={{ xs: 12, md: 4 }}>
                <FormControl fullWidth>
                  <InputLabel>记录级别</InputLabel>
                  <Select
                    value={config.min_severity}
                    label="记录级别"
                    onChange={(event) => patch({ min_severity: event.target.value as DiagnosticLogSeverity })}
                  >
                    {SEVERITY_OPTIONS.map((option) => (
                      <MenuItem key={option.value} value={option.value}>{option.label}</MenuItem>
                    ))}
                  </Select>
                </FormControl>
                <Typography variant="caption" color="text.secondary" display="block" mt={0.75}>
                  {SEVERITY_OPTIONS.find((option) => option.value === config.min_severity)?.hint}
                </Typography>
              </Grid>
              <Grid size={{ xs: 12, md: 4 }}>
                <TextField
                  fullWidth
                  label="保留天数"
                  value={retentionInput}
                  onChange={(event) => {
                    setRetentionInput(event.target.value.replace(/\D/g, '').slice(0, 3))
                    const parsed = parseBoundedInt(event.target.value, RETENTION_MIN, RETENTION_MAX, config.retention_days)
                    if (parsed !== null) patch({ retention_days: parsed })
                  }}
                  onBlur={() => setRetentionInput(String(config.retention_days))}
                  slotProps={{ htmlInput: { inputMode: 'numeric' } }}
                  helperText={`${RETENTION_MIN}-${RETENTION_MAX} 天，超期文件自动删除`}
                />
              </Grid>
              <Grid size={{ xs: 12, md: 4 }}>
                <TextField
                  fullWidth
                  label="体积上限 (MB)"
                  value={sizeInput}
                  onChange={(event) => {
                    setSizeInput(event.target.value.replace(/\D/g, '').slice(0, 4))
                    const parsed = parseBoundedInt(event.target.value, SIZE_MIN, SIZE_MAX, config.max_total_mb)
                    if (parsed !== null) patch({ max_total_mb: parsed })
                  }}
                  onBlur={() => setSizeInput(String(config.max_total_mb))}
                  slotProps={{ htmlInput: { inputMode: 'numeric' } }}
                  helperText={`${SIZE_MIN}-${SIZE_MAX} MB，超出后从最旧的文件开始删除`}
                />
              </Grid>
            </Grid>

            <Typography variant="caption" color="text.secondary" display="block" mt={2}>
              两条限制以先触发的为准：保留天数控制历史长度，体积上限防止重试风暴写满设备存储。
            </Typography>

            <Box display="flex" justifyContent="flex-end" gap={1.5} mt={3}>
              <Button
                variant="outlined"
                disabled={!dirty || saving}
                onClick={() => {
                  setConfig(savedConfig)
                  setRetentionInput(String(savedConfig.retention_days))
                  setSizeInput(String(savedConfig.max_total_mb))
                }}
              >
                还原
              </Button>
              <Button
                variant="contained"
                startIcon={saving ? <CircularProgress size={16} color="inherit" /> : <Save />}
                disabled={!dirty || saving}
                onClick={() => void save()}
              >
                保存设置
              </Button>
            </Box>
          </CardContent>
        </Card>
      </Stack>
    </Box>
  )
}
