import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import {
  Alert,
  Box,
  Button,
  Card,
  CardContent,
  CardHeader,
  Chip,
  CircularProgress,
  IconButton,
  Stack,
  Switch,
  Tooltip,
  Typography,
} from '@mui/material'
import Grid from '@mui/material/Grid'
import { CellTower, FlightTakeoff, Lan, Memory, Refresh, Replay, SettingsEthernet, SimCard, SwapVert, TravelExplore, Usb, Wifi } from '@mui/icons-material'
import {
  api,
  type LineNetworkControlsResponse,
  type TrunkProfileResponse,
  type VolteLineControlResponse,
  type VowifiLineConfigResponse,
} from '../../api/current'
import { maskedIccid, modemSlotLabel, modemSlotSourceLabel, shortLineId, stableModemSort } from '../../components/modemLineFormat'
import TrunkProfileDialog from './TrunkProfileDialog'
import VowifiLineDialog from './VowifiLineDialog'
import VolteLineDialog from './VolteLineDialog'
import LineDetailsDialog, { type LineDetailTab } from './LineDetailsDialog'
import DataProxyDialog from './DataProxyDialog'
import EsimLineDialog from './EsimLineDialog'
import { formatBytes } from '../Dashboard/utils'

const stageLabels: Record<string, string> = {
  disabled: '未连接',
  starting: '正在启动',
  identity: '读取 SIM 身份',
  identity_aka: 'SIM AKA 鉴权',
  radio: '检查无线网络',
  ims_context: '建立 IMS 上下文',
  pcscf: '发现 P-CSCF',
  ipv6_preflight: 'IPv6 数据路径预检',
  modem: '准备基带',
  bearer: '建立 IMS Bearer',
  bearer_dual: '建立双栈 IMS Bearer',
  bearer_ipv4: '回退 IPv4 IMS Bearer',
  bearer_ipv6: '回退 IPv6 IMS Bearer',
  ip_config: '配置 IMS 网络',
  register_initial: '发送初始 REGISTER',
  ipsec: '建立 IMS IPsec',
  register_authenticated: '发送鉴权 REGISTER',
  register_refresh: '续期 IMS 注册',
  register_ipsec: 'IPsec 注册',
  register_udp: 'UDP 注册',
  registered: 'IMS 已注册',
  stopping: '正在断开',
}

function runtimeLabel(line: VolteLineControlResponse) {
  if (line.runtime.registered) return 'IMS 已注册'
  if (line.profile.volte_connection_enabled && line.runtime.last_error) return `${stageLabels[line.runtime.stage] ?? line.runtime.stage}失败`
  if (line.profile.volte_connection_enabled) return stageLabels[line.runtime.stage] ?? '等待重连'
  return 'IMS 未连接'
}

function voiceAccessLabel(line: VolteLineControlResponse, vowifi?: VowifiLineConfigResponse) {
  if (vowifi?.runtime_registered) return 'VoWiFi'
  if (line.runtime.registered) return 'VoLTE'
  return 'CS 语音'
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

function trunkRuntimeLabel(line?: TrunkProfileResponse) {
  if (!line || !line.trunk.enabled) return 'Trunk 未启用'
  if (line.runtime.registered) return 'Asterisk 已注册'
  if (line.runtime.phase === 'ready') return '静态 Peer 已监听'
  if (line.runtime.phase === 'configured') return '已配置，等待启动'
  if (line.runtime.phase === 'degraded') return '连接异常'
  if (line.runtime.last_error) return `${line.runtime.stage || '连接'}失败`
  return line.runtime.stage || '等待启动'
}

const vowifiStageLabels: Record<string, string> = {
  disabled: '未启用', starting: '正在启动', identity_ready: 'SIM 身份已读取',
  profile_matched: '运营商配置已匹配', sim_auth_ready: 'SIM AKA 已就绪',
  epdg_ready: 'ePDG 已连接', ike_ready: 'IKE 已建立', child_sa_ready: 'CHILD SA 已建立',
  esp_ready: 'ESP 数据通道已建立', ims_registered: 'IMS 已注册', sms_ready: '短信已就绪',
  voice_ready: '语音已就绪', not_started: '等待启动',
}

function vowifiRuntimeLabel(line?: VowifiLineConfigResponse) {
  if (!line?.config.enabled) return 'VoWiFi未启用'
  if (line.runtime_registered) return 'VoWiFi IMS 已注册'
  const stage = vowifiStageLabels[line.runtime_stage] ?? line.runtime_stage
  return line.runtime_error ? `${stage}失败` : stage
}

function recoveryMessage(line: VolteLineControlResponse) {
  const runtime = line.runtime
  switch (runtime.recovery_state) {
    case 'waiting_modem':
      return '长时间未检测到基带，正在等待设备重新出现'
    case 'restarting_baseband':
      return `正在恢复基带（${runtime.modem_restart_attempt}/${runtime.modem_restart_max}）`
    case 'connecting':
      return runtime.retry_attempt > 0
        ? `正在执行第 ${runtime.retry_attempt}/${runtime.retry_max} 次完整 IMS 注册尝试`
        : '正在准备 IMS 注册重试'
    case 'exhausted':
      return runtime.modem_restart_attempt >= runtime.modem_restart_max
        ? `基带恢复 ${runtime.modem_restart_max} 次后仍不可用，已停止自动恢复`
        : `连续 ${runtime.retry_max} 次完整 IMS 注册尝试均失败，已停止自动恢复`
    default:
      return null
  }
}

/** Whether the line's SIM advertises a eUICC chip, without paying an lpac probe. */
function lineReportsEuicc(line: VolteLineControlResponse) {
  return line.modem.sim_type === 'esim'
    || line.modem.esim_status === 'no-profiles'
    || line.modem.esim_status === 'with-profiles'
}

/** Effective eSIM management state: explicit override, else auto-detection. */
function esimActive(line: VolteLineControlResponse) {
  const control = line.profile.esim_control
  if (control === true) return true
  if (control === false) return false
  return lineReportsEuicc(line)
}

/** One-line hint describing why eSIM management is on or off for this line. */
function esimStatusHint(line: VolteLineControlResponse) {
  const control = line.profile.esim_control
  if (control === false) return '已按普通 SIM 处理（手动关闭）'
  if (control === true) return lineReportsEuicc(line) ? 'eSIM 控制已手动开启' : '手动开启，将通过外置 lpac 管理'
  return lineReportsEuicc(line) ? '已自动探测到 eUICC' : '未探测到 eUICC，按普通 SIM 处理'
}

/** Chip label reflecting the card's reported eUICC profile state, else on/off. */
function esimChipLabel(line: VolteLineControlResponse) {
  const status = line.modem.esim_status
  if (esimActive(line)) {
    if (status === 'with-profiles') return '有 Profile'
    if (status === 'no-profiles') return '无 Profile'
    return '已启用'
  }
  return '未启用'
}

export default function ModemLinesPanel({ basicInfoForLine }: { basicInfoForLine?: (lineId: string) => ReactNode }) {
  const [lines, setLines] = useState<VolteLineControlResponse[]>([])
  const [trunkLines, setTrunkLines] = useState<TrunkProfileResponse[]>([])
  const [vowifiLines, setVowifiLines] = useState<VowifiLineConfigResponse[]>([])
  const [networkControls, setNetworkControls] = useState<LineNetworkControlsResponse[]>([])
  const [editingTrunkLine, setEditingTrunkLine] = useState<TrunkProfileResponse | null>(null)
  const [editingVowifiLine, setEditingVowifiLine] = useState<VowifiLineConfigResponse | null>(null)
  const [editingVolteLine, setEditingVolteLine] = useState<VolteLineControlResponse | null>(null)
  const [editingDataLineId, setEditingDataLineId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [savingKey, setSavingKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [detailLine, setDetailLine] = useState<VolteLineControlResponse | null>(null)
  const [detailTab, setDetailTab] = useState<LineDetailTab>('basic')
  const [esimDialogModem, setEsimDialogModem] = useState<VolteLineControlResponse['modem'] | null>(null)

  const openDetails = (line: VolteLineControlResponse, tab: LineDetailTab) => {
    setDetailLine(line)
    setDetailTab(tab)
  }

  const load = useCallback(async (background = false) => {
    if (!background) setLoading(true)
    try {
      const [lineResponse, trunkResponse, vowifiResponse, networkResponse] = await Promise.all([
        api.getVolteLines(),
        api.getTrunkLines(),
        api.getVowifiLines(),
        api.getLineNetworkControls(),
      ])
      setLines(stableModemSort(lineResponse.data ?? []))
      setTrunkLines(stableModemSort(trunkResponse.data ?? []))
      setVowifiLines(stableModemSort(vowifiResponse.data ?? []))
      setNetworkControls(networkResponse.data ?? [])
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
  const trunkByLineId = useMemo(() => new Map(
    trunkLines.map((line) => [line.line_id, line]),
  ), [trunkLines])
  const vowifiByLineId = useMemo(() => new Map(
    vowifiLines.map((line) => [line.line_id, line]),
  ), [vowifiLines])
  const networkByLineId = useMemo(() => new Map(
    networkControls.map((controls) => [controls.line_id, controls]),
  ), [networkControls])

  const updateNetworkControl = (updated: LineNetworkControlsResponse) => {
    setNetworkControls((current) => current.map((item) => item.line_id === updated.line_id ? updated : item))
  }

  const toggleDataConnection = async (lineId: string, enabled: boolean) => {
    setSavingKey(`data:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setLineDataConnection(lineId, enabled)
      if (response.data) updateNetworkControl(response.data)
      setSuccess(`${shortLineId(lineId)} ${enabled ? '已建立移动数据出口' : '已关闭移动数据出口'}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  const toggleAirplaneMode = async (lineId: string, enabled: boolean) => {
    setSavingKey(`airplane:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setLineAirplaneMode(lineId, enabled)
      if (response.data) updateNetworkControl(response.data)
      await load(true)
      setSuccess(`${shortLineId(lineId)} 飞行模式已${enabled ? '开启，移动射频、数据与 VoLTE 已关闭' : '关闭'}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  const resetTraffic = async (lineId: string) => {
    setSavingKey(`traffic:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.resetLineDataTraffic(lineId)
      if (response.data) updateNetworkControl(response.data)
      setSuccess(`${shortLineId(lineId)} 流量统计已清零`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  const toggleRoaming = async (lineId: string, allowed: boolean) => {
    setSavingKey(`roaming:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setLineRoaming(lineId, allowed)
      if (response.data) updateNetworkControl(response.data)
      setSuccess(`${shortLineId(lineId)} 已${allowed ? '允许' : '禁止'}漫游数据`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  const toggleLine = async (lineId: string, enabled: boolean) => {
    setSavingKey(`volte:${lineId}`)
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

  const toggleVowifi = async (lineId: string, enabled: boolean) => {
    setSavingKey(`vowifi:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setVowifiLineConnection(lineId, enabled)
      if (response.data) {
        const updatedLine = response.data
        setVowifiLines((current) => current.map((line) => line.line_id === lineId ? updatedLine : line))
      }
      setSuccess(`${shortLineId(lineId)} ${enabled ? '已提交 VoWiFi 连接请求' : '已关闭 VoWiFi'}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  const handleVowifiSaved = (updated: VowifiLineConfigResponse) => {
    setVowifiLines((current) => current.map((line) => line.line_id === updated.line_id ? updated : line))
    setEditingVowifiLine(updated)
    setSuccess(`${shortLineId(updated.line_id)} 的 VoWiFi 配置已保存`)
  }

  const handleVolteSaved = (updated: VolteLineControlResponse) => {
    setLines((current) => current.map((line) => line.modem.line_id === updated.modem.line_id ? updated : line))
    setEditingVolteLine(updated)
    setSuccess(`${shortLineId(updated.modem.line_id)} 的 VoLTE 地址族已保存`)
  }

  const retryLine = async (lineId: string) => {
    setSavingKey(`retry:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.retryVolteLine(lineId)
      if (response.data) {
        const updatedLine = response.data
        setLines((current) => current.map((line) => (
          line.modem.line_id === lineId ? updatedLine : line
        )))
      }
      setSuccess(`${shortLineId(lineId)} 已开始新的五次 VoLTE 恢复批次`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  const toggleTrunk = async (lineId: string, enabled: boolean) => {
    setSavingKey(`trunk:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setTrunkLineEnabled(lineId, enabled)
      if (response.data) {
        const updated = response.data
        setTrunkLines((current) => current.map((line) => line.line_id === lineId ? updated : line))
      }
      setSuccess(`${shortLineId(lineId)} ${enabled ? '已保存 Trunk 启用意图' : '已关闭 Trunk'}`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  const handleTrunkSaved = (updated: TrunkProfileResponse) => {
    setTrunkLines((current) => current.map((line) => line.line_id === updated.line_id ? updated : line))
    setEditingTrunkLine(updated)
    setSuccess(`${shortLineId(updated.line_id)} 的 Asterisk Trunk 配置已保存`)
  }

  // Set the line's eSIM management override. `null` returns it to automatic
  // detection (managed only when the SIM reports a eUICC), while `true`/`false`
  // force the lpac controls on/off. The backend echoes the resolved profile, so
  // we patch the line's `esim_control` locally from the request value.
  const applyEsimControl = async (lineId: string, control: boolean | null) => {
    setSavingKey(`esim:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      await api.setLineEsimControl(lineId, control)
      setLines((current) => current.map((line) => (
        line.modem.line_id === lineId
          ? { ...line, profile: { ...line.profile, esim_control: control } }
          : line
      )))
      setSuccess(
        control === null
          ? `${shortLineId(lineId)} eSIM 控制已恢复自动`
          : `${shortLineId(lineId)} 已${control ? '开启' : '关闭'} eSIM 控制`,
      )
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      await load(true)
    } finally {
      setSavingKey(null)
    }
  }

  // Toggling the switch on returns the line to automatic detection when the SIM
  // already reports a eUICC (so the "auto" state is preserved); otherwise it is a
  // manual force-on. Turning it off always writes an explicit disable so a plain
  // SIM never gets probed by lpac.
  const toggleEsimControl = (line: VolteLineControlResponse, enabled: boolean) => {
    if (!enabled) return applyEsimControl(line.modem.line_id, false)
    return applyEsimControl(line.modem.line_id, lineReportsEuicc(line) ? null : true)
  }

  const handleDataProxySaved = (updated: LineNetworkControlsResponse) => {
    updateNetworkControl(updated)
    setSuccess(`${shortLineId(updated.line_id)} 的数据代理监听配置已保存`)
  }

  if (loading) {
    return <Box display="flex" justifyContent="center" alignItems="center" minHeight="35vh"><CircularProgress /></Box>
  }

  return (
    <Stack spacing={2.5}>
      {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
      {success && <Alert severity="success" onClose={() => setSuccess(null)}>{success}</Alert>}

      <Alert severity="info">
        页面顺序绑定物理基带卡槽；每条线路由“物理槽位 + UIM 卡槽 + 当前 SIM”唯一识别。更换 SIM 会生成新的线路，不会自动继承旧线路的 IMS 或后续 Trunk 配置。
      </Alert>

      <Card>
        <CardHeader
          avatar={<CellTower color="primary" />}
          title="基带线路"
          subheader={`${presentCount} 个基带在线 · ${registeredCount} 条线路已注册 IMS`}
          titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
          action={<Tooltip title="刷新线路状态"><IconButton onClick={() => void load()} disabled={savingKey !== null}><Refresh /></IconButton></Tooltip>}
        />
        <CardContent sx={{ pt: 0 }}>
          <Alert severity="info" sx={{ py: 0.25 }}>
            每个物理基带和 UIM 卡槽分别配置 VoLTE 与 VoWiFi。VoWiFi 的 DNS、代理和 ePDG 覆盖项可在线路配置中单独保存。
          </Alert>
        </CardContent>
      </Card>

      {lines.length === 0 ? (
        <Alert severity="warning">当前没有发现 ModemManager 基带。请检查设备连接和 ModemManager 服务。</Alert>
      ) : (
        <Grid container spacing={2.5}>
          {lines.map((line, index) => {
            const volteBusy = savingKey === `volte:${line.modem.line_id}`
            const retryBusy = savingKey === `retry:${line.modem.line_id}`
            const vowifiBusy = savingKey === `vowifi:${line.modem.line_id}`
            const trunkBusy = savingKey === `trunk:${line.modem.line_id}`
            const dataBusy = savingKey === `data:${line.modem.line_id}`
            const trafficBusy = savingKey === `traffic:${line.modem.line_id}`
            const roamingBusy = savingKey === `roaming:${line.modem.line_id}`
            const airplaneBusy = savingKey === `airplane:${line.modem.line_id}`
            const trunkLine = trunkByLineId.get(line.modem.line_id)
            const vowifiLine = vowifiByLineId.get(line.modem.line_id)
            const network = networkByLineId.get(line.modem.line_id)
            const airplaneEnabled = network?.airplane_mode_requested ?? line.profile.airplane_mode_enabled
            const recovery = recoveryMessage(line)
            const recoveryRunning = ['waiting_modem', 'restarting_baseband', 'connecting'].includes(line.runtime.recovery_state)
            // A standalone reader has no cellular baseband: it only participates
            // in VoWiFi and eSIM management, so the cellular controls (VoLTE,
            // data, roaming, airplane, Trunk) are hidden for it.
            const isReader = line.modem.line_kind === 'reader'
            return (
              <Grid key={line.modem.line_id} size={{ xs: 12, lg: 6 }}>
                <Card variant="outlined" sx={{ height: '100%', opacity: line.modem.present ? 1 : 0.68 }}>
                  <CardHeader
                    avatar={isReader ? <Usb color={line.modem.present ? 'primary' : 'disabled'} /> : <CellTower color={line.modem.present ? 'primary' : 'disabled'} />}
                    title={isReader
                      ? `读卡器 ${line.modem.slot_label || ''} · 卡槽 ${line.modem.uim_slot}`.trim()
                      : `${modemSlotLabel(line.modem, index)} · 卡槽 ${line.modem.uim_slot} · ${line.modem.manufacturer || '未知厂商'} ${line.modem.model || ''}`}
                    subheader={isReader
                      ? `线路 ${shortLineId(line.modem.line_id)} · ${line.modem.model || '独立读卡器'}`
                      : `线路 ${shortLineId(line.modem.line_id)} · ModemManager ${line.modem.modem_id}`}
                    sx={{
                      alignItems: 'flex-start',
                      flexWrap: { xs: 'wrap', sm: 'nowrap' },
                      '& .MuiCardHeader-content': { minWidth: 0, flexBasis: { xs: 'calc(100% - 52px)', sm: 'auto' } },
                      '& .MuiCardHeader-action': { margin: 0, marginLeft: { xs: '52px', sm: 'auto' }, width: { xs: 'calc(100% - 52px)', sm: 'auto' } },
                    }}
                    titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
                    action={
                      <Stack direction="row" spacing={0.75} mt={{ xs: 1, sm: 0.5 }} flexWrap="wrap" justifyContent={{ xs: 'flex-start', sm: 'flex-end' }}>
                        <Chip size="small" label={line.modem.present ? '在线' : '离线'} color={line.modem.present ? 'success' : 'default'} variant="outlined" />
                        {line.modem.slot_conflict && <Chip size="small" label="槽位冲突" color="error" />}
                        <Chip size="small" label={modemSlotSourceLabel(line.modem.slot_source, line.modem.slot_stable)} color={line.modem.slot_stable ? 'success' : 'warning'} variant="outlined" />
                        <Chip size="small" label={`主线路 · ${voiceAccessLabel(line, vowifiLine)}`} color={vowifiLine?.runtime_registered || line.runtime.registered ? 'primary' : 'default'} variant="outlined" />
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
                      {!isReader && <Grid size={6}>
                        <Typography variant="caption" color="text.secondary">驻网状态</Typography>
                        <Typography variant="body2" mt={0.25}>{modemStateLabel(line.modem.state)}</Typography>
                      </Grid>}
                      {!isReader && <Grid size={6}>
                        <Typography variant="caption" color="text.secondary">运营商 PLMN</Typography>
                        <Typography variant="body2" mt={0.25}>{line.modem.operator_id || '未读取'}</Typography>
                      </Grid>}
                      <Grid size={6}>
                        <Typography variant="caption" color="text.secondary">{isReader ? '读卡器 / UIM' : 'QMI / UIM'}</Typography>
                        <Typography variant="body2" mt={0.25} sx={{ wordBreak: 'break-all' }}>
                          {(isReader ? line.modem.model : line.modem.qmi_device) || '未发现'} · Slot {line.modem.uim_slot}
                        </Typography>
                      </Grid>
                      {!isReader && <Grid size={{ xs: 12, sm: 6 }}>
                        <Typography variant="caption" color="text.secondary">IMS 数据路径</Typography>
                        <Typography variant="body2" mt={0.25} sx={{ wordBreak: 'break-all' }}>
                          {line.runtime.data_path_mode || '尚未建立'}{line.runtime.pcscf ? ` · P-CSCF ${line.runtime.pcscf}` : ''}
                        </Typography>
                      </Grid>}
                      <Grid size={{ xs: 12, sm: 6 }} display="flex" alignItems="center" justifyContent="center">
                        <Typography
                          component="button"
                          type="button"
                          variant="subtitle1"
                          color="primary"
                          onClick={() => openDetails(line, 'basic')}
                          sx={{ border: 0, bgcolor: 'transparent', px: 1, py: 0.75, cursor: 'pointer', fontWeight: 600, textAlign: 'center' }}
                        >
                          SIM 卡详情
                        </Typography>
                      </Grid>
                    </Grid>

                    {(recovery || line.runtime.last_error) && (
                      <Alert severity={line.runtime.recovery_state === 'exhausted' ? 'error' : 'warning'} sx={{ mt: 2, py: 0.25 }}>
                        {recovery ?? line.runtime.last_error}
                        {line.runtime.next_retry_at && (
                          <Typography variant="caption" display="block">
                            下次尝试：{new Date(line.runtime.next_retry_at).toLocaleString()}
                          </Typography>
                        )}
                      </Alert>
                    )}

                    {line.modem.slot_conflict && (
                      <Alert severity="error" sx={{ mt: 2, py: 0.25 }}>
                        多个基带解析到同一物理槽位，请检查 udev 的 MM_ID_PHYSDEV_UID 或设备树槽位映射。
                      </Alert>
                    )}

                    <Box display="flex" flexDirection="column">
                    {!isReader && (<>
                    <Box order={3} display="flex" justifyContent="space-between" alignItems="center" mt={2} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
                      <Box minWidth={0}>
                        <Box display="flex" alignItems="center" gap={0.75}>
                          <Lan color="action" fontSize="small" />
                          <Typography variant="body2" fontWeight={600}>数据连接与代理出口</Typography>
                        </Box>
                        <Typography variant="caption" color="text.secondary" display="block" mt={0.25} sx={{ wordBreak: 'break-word' }}>
                          {network?.data.proxy.phase === 'failed'
                            ? network.data.proxy.stage
                            : network?.data.proxy.running && network.data.proxy.port
                              ? `代理已就绪：${network.data.proxy.listen_ip || network.data.config.listen_ip}:${network.data.proxy.port} · ${network.data.proxy.interface_name || '移动数据网卡'}`
                              : network?.data.enabled
                                ? network.data.proxy.stage || '正在建立移动数据出口'
                                : '流量未启用'}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={1}>
                        <Chip
                          size="small"
                          label={network?.data.proxy.running ? `端口 ${network.data.proxy.port}` : network?.data.proxy.phase === 'failed' ? '连接失败' : network?.data.enabled ? '连接中' : '流量未启用'}
                          color={network?.data.proxy.running ? 'success' : network?.data.proxy.phase === 'failed' ? 'error' : network?.data.enabled ? 'warning' : 'default'}
                          variant="outlined"
                        />
                        <Button size="small" variant="text" onClick={() => setEditingDataLineId(line.modem.line_id)} disabled={!network || savingKey !== null}>配置</Button>
                        {dataBusy && <CircularProgress size={18} />}
                        <Switch
                          checked={network?.data.enabled ?? false}
                          onChange={(_, enabled) => void toggleDataConnection(line.modem.line_id, enabled)}
                          disabled={!network || !line.modem.present || airplaneEnabled || savingKey !== null}
                        />
                      </Box>
                    </Box>

                    {/* 流量只展示当前启用会话；每次重新启用由后端自动清零。 */}
                    {network?.data.enabled && <Box order={3} display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
                      <Box minWidth={0}>
                        <Box display="flex" alignItems="center" gap={0.75}>
                          <SwapVert color={network?.data.proxy.traffic_used ? 'primary' : 'disabled'} fontSize="small" />
                          <Typography variant="body2" fontWeight={600}>流量用量</Typography>
                        </Box>
                        <Typography variant="caption" color="text.secondary" display="block" mt={0.25} sx={{ wordBreak: 'break-word' }}>
                          {network?.data.proxy.traffic_used
                            ? `上行 ${formatBytes(network.data.proxy.traffic.uplink_bytes)} · 下行 ${formatBytes(network.data.proxy.traffic.downlink_bytes)} · 连接 ${network.data.proxy.traffic.total_connections} 次${network.data.proxy.traffic.active_connections > 0 ? `（活跃 ${network.data.proxy.traffic.active_connections}）` : ''}`
                            : '这张卡还没有走过流量'}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={1}>
                        <Chip
                          size="small"
                          label={network?.data.proxy.traffic_used ? formatBytes((network.data.proxy.traffic.uplink_bytes ?? 0) + (network.data.proxy.traffic.downlink_bytes ?? 0)) : '未使用'}
                          color={network?.data.proxy.traffic_used ? 'primary' : 'default'}
                          variant="outlined"
                        />
                        {trafficBusy && <CircularProgress size={18} />}
                        <Button
                          size="small"
                          variant="text"
                          onClick={() => void resetTraffic(line.modem.line_id)}
                          disabled={!network?.data.proxy.traffic_used || savingKey !== null}
                        >
                          清零
                        </Button>
                      </Box>
                    </Box>}

                    <Box order={4} display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
                      <Box minWidth={0}>
                        <Box display="flex" alignItems="center" gap={0.75}>
                          <TravelExplore color={network?.roaming.roaming_allowed ? 'info' : 'disabled'} fontSize="small" />
                          <Typography variant="body2" fontWeight={600}>漫游数据</Typography>
                          {network?.roaming.is_roaming && <Chip label="漫游中" size="small" color="warning" sx={{ height: 18, fontSize: '0.65rem' }} />}
                        </Box>
                        <Typography variant="caption" color="text.secondary" display="block" mt={0.25}>
                          {network?.roaming.roaming_allowed ? '允许该线路使用漫游数据' : '已禁止该线路使用漫游数据'}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={0.5}>
                        <Chip size="small" label={network?.roaming.roaming_allowed ? '已允许' : '已禁止'} color={network?.roaming.roaming_allowed ? 'info' : 'default'} variant="outlined" />
                        {roamingBusy && <CircularProgress size={18} />}
                        <Switch
                          checked={network?.roaming.roaming_allowed ?? true}
                          onChange={(_, enabled) => void toggleRoaming(line.modem.line_id, enabled)}
                          disabled={!network || !line.modem.present || airplaneEnabled || savingKey !== null}
                        />
                      </Box>
                    </Box>

                    <Box order={5} display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
                      <Box minWidth={0}>
                        <Box display="flex" alignItems="center" gap={0.75}>
                          <FlightTakeoff color={airplaneEnabled ? 'warning' : 'action'} fontSize="small" />
                          <Typography variant="body2" fontWeight={600}>飞行模式</Typography>
                        </Box>
                        <Typography variant="caption" color="text.secondary" display="block" mt={0.25}>
                          {network?.airplane_stage || (airplaneEnabled ? '正在关闭移动射频' : '移动射频正常')}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={1}>
                        <Chip size="small" label={network?.airplane_phase === 'enabling' ? '开启中' : network?.airplane_phase === 'disabling' ? '关闭中' : airplaneEnabled ? '已开启' : '已关闭'} color={airplaneEnabled ? 'warning' : 'default'} variant="outlined" />
                        {airplaneBusy && <CircularProgress size={18} />}
                        <Switch
                          color="warning"
                          checked={airplaneEnabled}
                          onChange={(_, enabled) => void toggleAirplaneMode(line.modem.line_id, enabled)}
                          disabled={!network || (!line.modem.present && !airplaneEnabled) || savingKey !== null}
                        />
                      </Box>
                    </Box>

                    <Box order={2} display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider">
                      <Box>
                        <Typography variant="body2" fontWeight={600}>VoLTE IMS 连接</Typography>
                        <Typography variant="caption" color="text.secondary">
                          {line.runtime.registration_mode ? `注册方式：${line.runtime.registration_mode.toUpperCase()}` : '独立于其他基带管理'}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={1}>
                        <Chip size="small" label={line.profile.volte_connection_enabled ? runtimeLabel(line) : 'IMS 未连接'} color={line.runtime.registered ? 'success' : line.runtime.last_error ? 'error' : line.profile.volte_connection_enabled ? 'warning' : 'default'} variant="outlined" />
                        {(volteBusy || retryBusy) && <CircularProgress size={18} />}
                        {line.profile.volte_connection_enabled && line.runtime.manual_retry_available && (
                          <Tooltip title={recoveryRunning ? '自动恢复正在进行' : '立即开始新的五次恢复批次'}>
                            <span>
                              <Button
                                size="small"
                                variant="contained"
                                startIcon={<Replay />}
                                onClick={() => void retryLine(line.modem.line_id)}
                                disabled={airplaneEnabled || line.runtime.registered || recoveryRunning || savingKey !== null}
                              >
                                重试
                              </Button>
                            </span>
                          </Tooltip>
                        )}
                        <Button
                          size="small"
                          variant="text"
                          onClick={() => setEditingVolteLine(line)}
                          disabled={savingKey !== null}
                        >
                          地址族
                        </Button>
                        <Switch
                          checked={line.profile.volte_connection_enabled}
                          onChange={(_, enabled) => void toggleLine(line.modem.line_id, enabled)}
                          disabled={!line.modem.present || airplaneEnabled || savingKey !== null}
                        />
                      </Box>
                    </Box>
                    </>)}

                    <Box order={1} display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
                      <Box minWidth={0}>
                        <Box display="flex" alignItems="center" gap={0.75} flexWrap="wrap">
                          <Wifi color="action" fontSize="small" />
                          <Typography variant="body2" fontWeight={600}>VoWiFi / WiFi Calling</Typography>
                        </Box>
                        <Typography variant="caption" color="text.secondary" display="block" mt={0.25}>
                          {vowifiLine?.matched_profile_id ? `运营商 profile ${vowifiLine.matched_profile_id}` : '等待匹配运营商 profile'}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={0.5}>
                        <Chip size="small" label={vowifiRuntimeLabel(vowifiLine)} color={vowifiLine?.runtime_registered ? 'success' : vowifiLine?.runtime_error ? 'error' : vowifiLine?.config.enabled ? 'warning' : 'default'} variant="outlined" />
                        <Button
                          size="small"
                          variant="text"
                          onClick={() => vowifiLine && setEditingVowifiLine(vowifiLine)}
                          disabled={!vowifiLine || savingKey !== null}
                        >
                          配置
                        </Button>
                        {vowifiBusy && <CircularProgress size={18} />}
                        <Switch
                          checked={vowifiLine?.config.enabled ?? false}
                          onChange={(_, enabled) => void toggleVowifi(line.modem.line_id, enabled)}
                          disabled={!vowifiLine || !line.modem.present || savingKey !== null}
                        />
                      </Box>
                    </Box>

                    <Box order={6} display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
                      <Box minWidth={0}>
                        <Box display="flex" alignItems="center" gap={0.75} flexWrap="wrap">
                          <Memory color={esimActive(line) ? 'primary' : 'action'} fontSize="small" />
                          <Typography variant="body2" fontWeight={600}>eSIM 管理</Typography>
                        </Box>
                        <Typography variant="caption" color="text.secondary" display="block" mt={0.25} sx={{ wordBreak: 'break-word' }}>
                          {esimStatusHint(line)}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={0.5}>
                        <Chip size="small" label={esimChipLabel(line)} color={esimActive(line) ? 'primary' : 'default'} variant="outlined" />
                        <Button
                          size="small"
                          variant="text"
                          onClick={() => setEsimDialogModem(line.modem)}
                          disabled={!esimActive(line) || savingKey !== null}
                        >
                          管理
                        </Button>
                        {savingKey === `esim:${line.modem.line_id}` && <CircularProgress size={18} />}
                        <Switch
                          checked={esimActive(line)}
                          onChange={(_, enabled) => void toggleEsimControl(line, enabled)}
                          disabled={savingKey !== null}
                        />
                      </Box>
                    </Box>

                    {!isReader && (
                    <Box order={7} display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
                      <Box minWidth={0}>
                        <Box display="flex" alignItems="center" gap={0.75} flexWrap="wrap">
                          <SettingsEthernet color="action" fontSize="small" />
                          <Typography variant="body2" fontWeight={600}>Asterisk Trunk</Typography>
                        </Box>
                        <Typography variant="caption" color="text.secondary" display="block" mt={0.25} noWrap>
                          {trunkLine?.trunk.asterisk_host
                            ? `${trunkLine.trunk.registration_mode === 'outbound_register' ? '主动注册' : '静态 Peer'} · ${trunkLine.trunk.asterisk_host}:${trunkLine.trunk.asterisk_port}`
                            : '尚未配置远程 Asterisk'}
                        </Typography>
                      </Box>
                      <Box display="flex" alignItems="center" gap={0.5}>
                        <Chip size="small" label={trunkRuntimeLabel(trunkLine)} color={trunkLine?.runtime.registered || trunkLine?.runtime.phase === 'ready' ? 'success' : trunkLine?.runtime.last_error ? 'error' : trunkLine?.trunk.enabled ? 'warning' : 'default'} variant="outlined" />
                        <Button
                          size="small"
                          variant="text"
                          onClick={() => trunkLine && setEditingTrunkLine(trunkLine)}
                          disabled={!trunkLine || savingKey !== null}
                        >
                          配置
                        </Button>
                        {trunkBusy && <CircularProgress size={18} />}
                        <Switch
                          checked={trunkLine?.trunk.enabled ?? false}
                          onChange={(_, enabled) => void toggleTrunk(line.modem.line_id, enabled)}
                          disabled={!trunkLine || savingKey !== null}
                        />
                      </Box>
                    </Box>
                    )}
                    </Box>
                  </CardContent>
                </Card>
              </Grid>
            )
          })}
        </Grid>
      )}

      <TrunkProfileDialog
        open={editingTrunkLine !== null}
        line={editingTrunkLine}
        onClose={() => setEditingTrunkLine(null)}
        onSaved={handleTrunkSaved}
      />
      <VowifiLineDialog
        open={editingVowifiLine !== null}
        line={editingVowifiLine}
        onClose={() => setEditingVowifiLine(null)}
        onSaved={handleVowifiSaved}
      />
      {editingVolteLine && (
        <VolteLineDialog
          open
          lineId={editingVolteLine.modem.line_id}
          families={editingVolteLine.profile.volte_ip_families}
          onClose={() => setEditingVolteLine(null)}
          onSaved={handleVolteSaved}
        />
      )}
      <DataProxyDialog
        open={editingDataLineId !== null}
        lineId={editingDataLineId}
        controls={editingDataLineId ? networkByLineId.get(editingDataLineId) ?? null : null}
        onClose={() => setEditingDataLineId(null)}
        onSaved={handleDataProxySaved}
      />
      <EsimLineDialog
        open={esimDialogModem !== null}
        modem={esimDialogModem}
        onClose={() => setEsimDialogModem(null)}
      />
      <LineDetailsDialog
        key={`${detailLine?.modem.line_id ?? 'closed'}:${detailTab}`}
        open={detailLine !== null}
        line={detailLine}
        trunk={detailLine ? trunkByLineId.get(detailLine.modem.line_id) : undefined}
        vowifi={detailLine ? vowifiByLineId.get(detailLine.modem.line_id) : undefined}
        initialTab={detailTab}
        basicInfo={detailLine ? basicInfoForLine?.(detailLine.modem.line_id) : undefined}
        onClose={() => setDetailLine(null)}
      />
    </Stack>
  )
}
