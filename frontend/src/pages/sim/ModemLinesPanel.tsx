import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Card,
  CardContent,
  CardHeader,
  Chip,
  CircularProgress,
  FormControl,
  FormControlLabel,
  IconButton,
  InputLabel,
  MenuItem,
  Select,
  Stack,
  Switch,
  Tooltip,
  Typography,
} from '@mui/material'
import Grid from '@mui/material/Grid'
import { CellTower, Refresh, Router, SimCard } from '@mui/icons-material'
import {
  api,
  type VolteControlResponse,
  type VolteIpFamilyPreference,
  type VolteLineControlResponse,
} from '../../api/current'
import { maskedIccid, shortLineId } from '../../components/modemLineFormat'

const familyLabels: Record<VolteIpFamilyPreference, string> = {
  ipv4_first: 'IPv4 优先，失败后尝试 IPv6',
  ipv6_first: 'IPv6 优先，失败后尝试 IPv4',
  ipv4_only: '仅 IPv4',
  ipv6_only: '仅 IPv6',
}

const stageLabels: Record<string, string> = {
  disabled: '未连接',
  starting: '正在启动',
  identity: '读取 SIM 身份',
  identity_aka: 'SIM AKA 鉴权',
  radio: '检查无线网络',
  pcscf: '发现 P-CSCF',
  modem: '准备基带',
  bearer: '建立 IMS Bearer',
  register_ipsec: 'IPsec 注册',
  register_udp: 'UDP 注册',
  registered: 'IMS 已注册',
  stopping: '正在断开',
}

function runtimeLabel(line: VolteLineControlResponse) {
  if (line.runtime.registered) return 'IMS 已注册'
  if (line.profile.volte_connection_enabled) return stageLabels[line.runtime.stage] ?? '等待重连'
  return 'IMS 未连接'
}

function modemStateLabel(state: string) {
  const labels: Record<string, string> = {
    registered: '已驻网',
    connected: '数据已连接',
    enabled: '已启用',
    searching: '正在搜网',
    locked: 'SIM 已锁定',
    disabled: '已禁用',
    failed: '基带异常',
  }
  return (labels[state] ?? state) || '未知'
}

export default function ModemLinesPanel() {
  const [lines, setLines] = useState<VolteLineControlResponse[]>([])
  const [control, setControl] = useState<VolteControlResponse | null>(null)
  const [familyDraft, setFamilyDraft] = useState<VolteIpFamilyPreference>('ipv6_first')
  const [loading, setLoading] = useState(true)
  const [savingKey, setSavingKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const load = useCallback(async (background = false) => {
    if (!background) setLoading(true)
    try {
      const [controlResponse, lineResponse] = await Promise.all([
        api.getVolteControl(),
        api.getVolteLines(),
      ])
      if (controlResponse.data) {
        setControl(controlResponse.data)
        setFamilyDraft(controlResponse.data.ip_family_preference)
      }
      setLines(lineResponse.data ?? [])
      setError(null)
    } catch (err) {
      if (!background) setError(err instanceof Error ? err.message : String(err))
    } finally {
      if (!background) setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
    const timer = window.setInterval(() => void load(true), 10_000)
    return () => window.clearInterval(timer)
  }, [load])

  const presentCount = useMemo(() => lines.filter((line) => line.modem.present).length, [lines])
  const registeredCount = useMemo(() => lines.filter((line) => line.runtime.registered).length, [lines])

  const toggleFeature = async (enabled: boolean) => {
    setSavingKey('feature')
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setVolteFeature(enabled)
      if (response.data) setControl(response.data)
      setSuccess(enabled ? 'VoLTE 总开关已启用' : 'VoLTE 已关闭，所有线路连接已停止')
      await load(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSavingKey(null)
    }
  }

  const applyFamily = async () => {
    setSavingKey('family')
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setVolteIpFamily(familyDraft)
      if (response.data) setControl(response.data)
      setSuccess('IMS 地址族策略已更新')
      await load(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSavingKey(null)
    }
  }

  const toggleLine = async (lineId: string, enabled: boolean) => {
    setSavingKey(lineId)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setVolteLineConnection(lineId, enabled)
      if (response.data) {
        const updatedLine = response.data
        setLines((current) => current.map((line) => (
          line.modem.line_id === lineId ? updatedLine : line
        )))
      }
      setSuccess(`${shortLineId(lineId)} ${enabled ? '已请求连接 IMS' : '已断开 IMS'}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  if (loading) {
    return <Box display="flex" justifyContent="center" alignItems="center" minHeight="35vh"><CircularProgress /></Box>
  }

  return (
    <Stack spacing={2.5}>
      {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
      {success && <Alert severity="success" onClose={() => setSuccess(null)}>{success}</Alert>}

      <Alert severity="info">
        每条线路由“物理基带 + 当前 SIM”唯一识别。更换 SIM 会生成新的线路，不会自动继承旧线路的 IMS 或后续 Trunk 配置。
      </Alert>

      <Card>
        <CardHeader
          avatar={<Router color="primary" />}
          title="多基带与 VoLTE 总设置"
          subheader={`${presentCount} 个基带在线 · ${registeredCount} 条线路已注册 IMS`}
          titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
          action={
            <Tooltip title="刷新线路状态">
              <IconButton onClick={() => void load()} disabled={savingKey !== null}><Refresh /></IconButton>
            </Tooltip>
          }
        />
        <CardContent sx={{ pt: 0 }}>
          <Box display="grid" gridTemplateColumns={{ xs: '1fr', md: 'minmax(220px, 1fr) minmax(260px, 1.5fr) auto' }} gap={2} alignItems="center">
            <FormControlLabel
              control={
                <Switch
                  checked={control?.feature_enabled ?? false}
                  onChange={(_, enabled) => void toggleFeature(enabled)}
                  disabled={savingKey !== null}
                />
              }
              label="启用 VoLTE IMS 能力"
            />
            <FormControl size="small" fullWidth disabled={!control?.feature_enabled || savingKey !== null}>
              <InputLabel>IMS 地址族策略</InputLabel>
              <Select
                value={familyDraft}
                label="IMS 地址族策略"
                onChange={(event) => setFamilyDraft(event.target.value as VolteIpFamilyPreference)}
              >
                {Object.entries(familyLabels).map(([value, label]) => (
                  <MenuItem key={value} value={value}>{label}</MenuItem>
                ))}
              </Select>
            </FormControl>
            <Button
              variant="outlined"
              onClick={() => void applyFamily()}
              disabled={!control?.feature_enabled || savingKey !== null || familyDraft === control?.ip_family_preference}
            >
              {savingKey === 'family' ? '应用中…' : '应用策略'}
            </Button>
          </Box>
          <Typography variant="caption" color="text.secondary" display="block" mt={1.5}>
            修改地址族策略会重建已启用线路的 IMS 连接；支持 IPv4/IPv6 自动回退及单栈限制。
          </Typography>
        </CardContent>
      </Card>

      {lines.length === 0 ? (
        <Alert severity="warning">当前没有发现 ModemManager 基带。请检查设备连接和 ModemManager 服务。</Alert>
      ) : (
        <Grid container spacing={2.5}>
          {lines.map((line, index) => {
            const busy = savingKey === line.modem.line_id
            const runtimeColor = line.runtime.registered
              ? 'success'
              : line.profile.volte_connection_enabled
                ? 'warning'
                : 'default'
            return (
              <Grid key={line.modem.line_id} size={{ xs: 12, lg: 6 }}>
                <Card variant="outlined" sx={{ height: '100%', opacity: line.modem.present ? 1 : 0.68 }}>
                  <CardHeader
                    avatar={<CellTower color={line.modem.present ? 'primary' : 'disabled'} />}
                    title={`线路 ${index + 1} · ${line.modem.manufacturer || '未知厂商'} ${line.modem.model || ''}`}
                    subheader={`ID ${shortLineId(line.modem.line_id)} · 基带 ${line.modem.modem_id}`}
                    titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
                    action={
                      <Stack direction="row" spacing={0.75} mt={0.5}>
                        <Chip size="small" label={line.modem.present ? '在线' : '离线'} color={line.modem.present ? 'success' : 'default'} variant="outlined" />
                        <Chip size="small" label={runtimeLabel(line)} color={runtimeColor} />
                      </Stack>
                    }
                  />
                  <CardContent sx={{ pt: 0 }}>
                    <Grid container spacing={1.75}>
                      <Grid size={6}>
                        <Typography variant="caption" color="text.secondary">SIM 卡</Typography>
                        <Box display="flex" alignItems="center" gap={0.75} mt={0.25}>
                          <SimCard color="action" fontSize="small" />
                          <Typography variant="body2">{maskedIccid(line.modem.sim_iccid)}</Typography>
                        </Box>
                      </Grid>
                      <Grid size={6}>
                        <Typography variant="caption" color="text.secondary">驻网状态</Typography>
                        <Typography variant="body2" mt={0.25}>{modemStateLabel(line.modem.state)}</Typography>
                      </Grid>
                      <Grid size={6}>
                        <Typography variant="caption" color="text.secondary">运营商 PLMN</Typography>
                        <Typography variant="body2" mt={0.25}>{line.modem.operator_id || '未读取'}</Typography>
                      </Grid>
                      <Grid size={6}>
                        <Typography variant="caption" color="text.secondary">QMI / UIM</Typography>
                        <Typography variant="body2" mt={0.25} sx={{ wordBreak: 'break-all' }}>
                          {line.modem.qmi_device || '未发现'} · Slot {line.modem.uim_slot}
                        </Typography>
                      </Grid>
                      <Grid size={12}>
                        <Typography variant="caption" color="text.secondary">IMS 数据路径</Typography>
                        <Typography variant="body2" mt={0.25} sx={{ wordBreak: 'break-all' }}>
                          {line.runtime.data_path_mode || '尚未建立'}{line.runtime.pcscf ? ` · P-CSCF ${line.runtime.pcscf}` : ''}
                        </Typography>
                      </Grid>
                    </Grid>

                    {line.runtime.last_error && (
                      <Alert severity="warning" sx={{ mt: 2, py: 0.25 }}>
                        {line.runtime.last_error}
                      </Alert>
                    )}

                    <Box display="flex" justifyContent="space-between" alignItems="center" mt={2} pt={1.5} borderTop={1} borderColor="divider">
                      <Box>
                        <Typography variant="body2" fontWeight={600}>VoLTE IMS 连接</Typography>
                        <Typography variant="caption" color="text.secondary">
                          {line.runtime.registration_mode ? `注册方式：${line.runtime.registration_mode.toUpperCase()}` : '独立于其他基带管理'}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={1}>
                        {busy && <CircularProgress size={18} />}
                        <Switch
                          checked={line.profile.volte_connection_enabled}
                          onChange={(_, enabled) => void toggleLine(line.modem.line_id, enabled)}
                          disabled={!control?.feature_enabled || !line.modem.present || savingKey !== null}
                        />
                      </Box>
                    </Box>
                  </CardContent>
                </Card>
              </Grid>
            )
          })}
        </Grid>
      )}
    </Stack>
  )
}
