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
  Tab,
  Tabs,
  Tooltip,
  Typography,
} from '@mui/material'
import Grid from '@mui/material/Grid'
import { CellTower, FlightTakeoff, Lan, Refresh, Replay, SettingsEthernet, TravelExplore, Tune, Usb, Wifi } from '@mui/icons-material'
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
import DataProxyDialog from './DataProxyDialog'
import { LineTrunkDetails, LineVolteDetails, LineVowifiDetails } from './LineRuntimeDetails'
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

type ModemLinesPanelProps = {
  basicInfoForLine?: (line: VolteLineControlResponse, controls?: ReactNode) => ReactNode
  workbench?: boolean
  workbenchHeader?: ReactNode
  workbenchEsim?: ReactNode
  workbenchSms?: ReactNode
  workbenchAutomation?: ReactNode
  workbenchNotifications?: ReactNode
  onSelectionChange?: (line: VolteLineControlResponse | null) => void
}

type WorkbenchTab = 'overview' | 'esim' | 'ims' | 'sms' | 'automation' | 'notifications'

export default function ModemLinesPanel({ basicInfoForLine, workbench = false, workbenchHeader, workbenchEsim, workbenchSms, workbenchAutomation, workbenchNotifications, onSelectionChange }: ModemLinesPanelProps) {
  const [lines, setLines] = useState<VolteLineControlResponse[]>([])
  const [trunkLines, setTrunkLines] = useState<TrunkProfileResponse[]>([])
  const [vowifiLines, setVowifiLines] = useState<VowifiLineConfigResponse[]>([])
  const [networkControls, setNetworkControls] = useState<LineNetworkControlsResponse[]>([])
  const [editingTrunkLine, setEditingTrunkLine] = useState<TrunkProfileResponse | null>(null)
  const [editingVowifiLine, setEditingVowifiLine] = useState<VowifiLineConfigResponse | null>(null)
  const [editingDataLineId, setEditingDataLineId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [savingKey, setSavingKey] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [selectedLineId, setSelectedLineId] = useState('')
  const [lineSearch, setLineSearch] = useState('')
  const [workbenchTab, setWorkbenchTab] = useState<WorkbenchTab>('overview')
  const [basebandRestartLine, setBasebandRestartLine] = useState<string | null>(null)

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
    setSelectedLineId((current) => {
      if (lines.some((line) => line.modem.line_id === current)) return current
      return lines[0]?.modem.line_id ?? ''
    })
  }, [lines])

  useEffect(() => {
    void load()
    const timer = window.setInterval(() => void load(true), 10_000)
    return () => window.clearInterval(timer)
  }, [load])

  const presentCount = useMemo(() => lines.filter((line) => line.modem.present).length, [lines])
  const trunkByLineId = useMemo(() => new Map(
    trunkLines.map((line) => [line.line_id, line]),
  ), [trunkLines])
  const vowifiByLineId = useMemo(() => new Map(
    vowifiLines.map((line) => [line.line_id, line]),
  ), [vowifiLines])
  const networkByLineId = useMemo(() => new Map(
    networkControls.map((controls) => [controls.line_id, controls]),
  ), [networkControls])

  const selectedLine = useMemo(
    () => lines.find((line) => line.modem.line_id === selectedLineId) ?? null,
    [lines, selectedLineId],
  )

  useEffect(() => {
    onSelectionChange?.(selectedLine)
  }, [onSelectionChange, selectedLine])

  useEffect(() => {
    setWorkbenchTab('overview')
  }, [selectedLineId])

  const filteredLines = useMemo(() => {
    const query = lineSearch.trim().toLocaleLowerCase()
    if (!query) return lines
    return lines.filter((line) => [
      line.modem.line_id,
      line.modem.modem_id,
      line.modem.slot_label,
      line.modem.manufacturer,
      line.modem.model,
      line.modem.sim_iccid,
    ].some((value) => value?.toLocaleLowerCase().includes(query)))
  }, [lineSearch, lines])

  const updateNetworkControl = (updated: LineNetworkControlsResponse) => {
    setNetworkControls((current) => current.map((item) => item.line_id === updated.line_id ? updated : item))
  }

  const lineIsPresent = (lineId: string) => (
    lines.find((line) => line.modem.line_id === lineId)?.modem.present ?? false
  )

  const toggleDataConnection = async (lineId: string, enabled: boolean) => {
    setSavingKey(`data:${lineId}`)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setLineDataConnection(lineId, enabled)
      if (response.data) updateNetworkControl(response.data)
      setSuccess(lineIsPresent(lineId)
        ? `${shortLineId(lineId)} ${enabled ? '已建立移动数据出口' : '已关闭移动数据出口'}`
        : `${shortLineId(lineId)} 的数据连接配置已保存，设备恢复后应用`)
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
      setSuccess(lineIsPresent(lineId)
        ? `${shortLineId(lineId)} 飞行模式已${enabled ? '开启，移动射频、数据与 VoLTE 已关闭' : '关闭'}`
        : `${shortLineId(lineId)} 的飞行模式配置已保存，设备恢复后应用`)
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
      setSuccess(lineIsPresent(lineId)
        ? `${shortLineId(lineId)} 已${allowed ? '允许' : '禁止'}漫游数据`
        : `${shortLineId(lineId)} 的漫游配置已保存，设备恢复后应用`)
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
      setSuccess(lineIsPresent(lineId)
        ? `${shortLineId(lineId)} ${enabled ? '已请求连接 IMS' : '已断开 IMS'}`
        : `${shortLineId(lineId)} 的 VoLTE IMS 配置已保存，设备恢复后应用`)
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
      setSuccess(lineIsPresent(lineId)
        ? `${shortLineId(lineId)} ${enabled ? '已提交 VoWiFi 连接请求' : '已关闭 VoWiFi'}`
        : `${shortLineId(lineId)} 的 VoWiFi 配置已保存，设备恢复后应用`)
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

  const handleDataProxySaved = (updated: LineNetworkControlsResponse) => {
    updateNetworkControl(updated)
    setSuccess(`${shortLineId(updated.line_id)} 的数据代理监听配置已保存`)
  }

  const restartBaseband = async (lineId: string) => {
    const present = lineIsPresent(lineId)
    const prompt = present
      ? '确认重启这条基带线路？网络注册和数据连接会短暂中断。'
      : '这条基带当前离线。确认使用保留的设备路径执行恢复？必要时会重启 ModemManager，并短暂影响其他基带。'
    if (!window.confirm(prompt)) return
    setBasebandRestartLine(lineId)
    setError(null)
    try {
      await api.restartBaseband(lineId)
      setSuccess(`${shortLineId(lineId)} 基带${present ? '重启' : '离线恢复'}流程已完成`)
      await load(true)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBasebandRestartLine(null)
    }
  }

  if (loading) {
    return <Box display="flex" justifyContent="center" alignItems="center" minHeight="35vh"><CircularProgress /></Box>
  }

  const renderLineList = () => (
    <Card sx={{ height: '100%', minHeight: { xs: 360, md: 560 }, maxHeight: { xs: 560, md: 'none' }, overflow: 'hidden', display: 'flex', flexDirection: 'column' }}>
      <CardHeader
        title="线路"
        subheader={`${lines.length} 条已发现 · ${presentCount} 条在线`}
        titleTypographyProps={{ variant: 'subtitle1', fontWeight: 700 }}
        action={<Tooltip title="刷新线路状态"><IconButton onClick={() => void load()} disabled={savingKey !== null}><Refresh /></IconButton></Tooltip>}
        sx={{ pb: 1.25, flexShrink: 0 }}
      />
      <CardContent sx={{ pt: 0, px: 1.25, flex: 1, minHeight: 0, overflowY: 'auto' }}>
        <Box
          component="input"
          value={lineSearch}
          onChange={(event) => setLineSearch(event.target.value)}
          placeholder="搜索线路 / ICCID"
          aria-label="搜索线路或 ICCID"
          sx={{
            width: '100%',
            mb: 1.25,
            px: 1.25,
            py: 0.9,
            borderRadius: 1,
            border: '1px solid',
            borderColor: 'divider',
            bgcolor: 'background.default',
            color: 'text.primary',
            outline: 'none',
            font: 'inherit',
            fontSize: 13,
            '&:focus': { borderColor: 'primary.main' },
          }}
        />
        <Stack spacing={0.75}>
          {filteredLines.map((line, index) => {
            const isSelected = line.modem.line_id === selectedLineId
            const vowifi = vowifiByLineId.get(line.modem.line_id)
            const active = line.runtime.registered || Boolean(vowifi?.runtime_registered)
            return (
              <Box
                key={line.modem.line_id}
                component="button"
                type="button"
                onClick={() => setSelectedLineId(line.modem.line_id)}
                sx={{
                  appearance: 'none',
                  width: '100%',
                  textAlign: 'left',
                  cursor: 'pointer',
                  border: '1px solid',
                  borderColor: isSelected ? 'primary.main' : 'divider',
                  borderRadius: 1.25,
                  bgcolor: isSelected ? 'action.selected' : 'transparent',
                  color: 'inherit',
                  px: 1.25,
                  py: 1.1,
                  opacity: line.modem.present ? 1 : 0.62,
                  '&:hover': { bgcolor: 'action.hover' },
                }}
              >
                <Box display="flex" justifyContent="space-between" gap={1} alignItems="center">
                  <Typography variant="body2" fontWeight={700} noWrap>
                    {line.modem.line_kind === 'reader' ? '读卡器' : modemSlotLabel(line.modem, index)}
                  </Typography>
                  <Chip size="small" label={line.modem.present ? (active ? '就绪' : '在线') : '离线'} color={active ? 'success' : line.modem.present ? 'info' : 'default'} sx={{ height: 19, fontSize: 10 }} />
                </Box>
                <Typography variant="caption" color="text.secondary" display="block" noWrap>
                  {line.modem.manufacturer || line.modem.model || '未知设备'} · {shortLineId(line.modem.line_id)}
                </Typography>
                <Typography variant="caption" color="text.disabled" display="block" noWrap>
                  {maskedIccid(line.modem.sim_iccid)}
                </Typography>
              </Box>
            )
          })}
          {filteredLines.length === 0 && <Typography variant="body2" color="text.secondary" sx={{ p: 1 }}>未找到匹配线路</Typography>}
        </Stack>
      </CardContent>
    </Card>
  )

  const lineContent = (
    <>
      {lines.length === 0 ? (
        <Alert severity="warning">当前没有发现基带或独立 SIM 读卡器。请检查设备连接和服务状态。</Alert>
      ) : (
        <Grid container spacing={2.5}>
          {(workbench ? lines.filter((line) => line.modem.line_id === selectedLineId) : lines).map((line, index) => {
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
            // A reader line shares VoWiFi, trunk, SMS, calls, and automation
            // with normal lines. Only controls that require a cellular radio or
            // ModemManager object are hidden.
            const isReader = line.modem.line_kind === 'reader'
            const overviewControls = !isReader ? (
              <Card>
                <CardHeader
                  avatar={<Tune color="primary" />}
                  title="线路控制"
                  titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
                  sx={{ pb: 0.75 }}
                />
                <CardContent sx={{ pt: 0, pb: '12px !important' }}>
                  <Box
                    display="grid"
                    gridTemplateColumns={{ xs: 'minmax(0, 1fr)', sm: 'repeat(2, minmax(0, 1fr))' }}
                    gridAutoRows="minmax(116px, auto)"
                    gap={1.25}
                  >
                <Box sx={{ p: 1.25, border: '1px solid', borderColor: 'divider', borderRadius: 1, height: '100%', display: 'flex', flexDirection: 'column' }}>
                  <Box display="flex" justifyContent="space-between" alignItems="flex-start" gap={1}>
                    <Box minWidth={0}>
                      <Box display="flex" alignItems="center" gap={0.75}>
                        <Lan color="action" fontSize="small" />
                        <Typography variant="body2" fontWeight={700}>数据连接</Typography>
                      </Box>
                      <Typography variant="caption" color="text.secondary" display="block" mt={0.5} sx={{ wordBreak: 'break-word' }}>
                        {!line.modem.present
                          ? '配置可修改，设备恢复后自动应用'
                          : network?.data.proxy.phase === 'failed'
                          ? network.data.proxy.stage
                          : network?.data.proxy.running && network.data.proxy.port
                            ? `${network.data.proxy.listen_ip || network.data.config.listen_ip}:${network.data.proxy.port} · ${network.data.proxy.interface_name || '移动数据网卡'}`
                            : network?.data.enabled ? network.data.proxy.stage || '正在建立移动数据出口' : '流量未启用'}
                      </Typography>
                    </Box>
                    <Switch
                      checked={network?.data.enabled ?? false}
                      onChange={(_, enabled) => void toggleDataConnection(line.modem.line_id, enabled)}
                      disabled={!network || (line.modem.present && airplaneEnabled) || savingKey !== null}
                    />
                  </Box>
                  <Box display="flex" alignItems="center" justifyContent="space-between" gap={1} mt="auto" pt={1}>
                    <Typography variant="caption" color="text.secondary">
                      {network?.data.proxy.traffic_used
                        ? `上行 ${formatBytes(network.data.proxy.traffic.uplink_bytes)} · 下行 ${formatBytes(network.data.proxy.traffic.downlink_bytes)}`
                        : '暂无代理流量'}
                    </Typography>
                    <Box display="flex" alignItems="center" gap={0.5}>
                      {(dataBusy || trafficBusy) && <CircularProgress size={16} />}
                      <Button size="small" onClick={() => setEditingDataLineId(line.modem.line_id)} disabled={!network || savingKey !== null}>配置</Button>
                      {network?.data.proxy.traffic_used && <Button size="small" onClick={() => void resetTraffic(line.modem.line_id)} disabled={savingKey !== null}>清零</Button>}
                    </Box>
                  </Box>
                </Box>

                <Box sx={{ p: 1.25, border: '1px solid', borderColor: 'divider', borderRadius: 1, height: '100%', display: 'flex', flexDirection: 'column' }}>
                  <Box display="flex" justifyContent="space-between" alignItems="flex-start" gap={1}>
                    <Box minWidth={0}>
                      <Box display="flex" alignItems="center" gap={0.75}>
                        <TravelExplore color={network?.roaming.roaming_allowed ? 'info' : 'disabled'} fontSize="small" />
                        <Typography variant="body2" fontWeight={700}>漫游数据</Typography>
                      </Box>
                      <Typography variant="caption" color="text.secondary" display="block" mt={0.5}>
                        {!line.modem.present ? '配置可修改，设备恢复后自动应用' : network?.roaming.is_roaming ? '当前网络处于漫游状态' : '当前未处于漫游状态'}
                      </Typography>
                    </Box>
                    <Switch
                      checked={network?.roaming.roaming_allowed ?? true}
                      onChange={(_, enabled) => void toggleRoaming(line.modem.line_id, enabled)}
                      disabled={!network || (line.modem.present && airplaneEnabled) || savingKey !== null}
                    />
                  </Box>
                  <Box display="flex" alignItems="center" gap={1} mt="auto" pt={1}>
                    {roamingBusy && <CircularProgress size={16} />}
                    <Chip size="small" label={network?.roaming.roaming_allowed ? '已允许' : '已禁止'} color={network?.roaming.roaming_allowed ? 'info' : 'default'} variant="outlined" />
                  </Box>
                </Box>

                <Box sx={{ p: 1.25, border: '1px solid', borderColor: 'divider', borderRadius: 1, height: '100%', display: 'flex', flexDirection: 'column' }}>
                  <Box display="flex" justifyContent="space-between" alignItems="flex-start" gap={1}>
                    <Box minWidth={0}>
                      <Box display="flex" alignItems="center" gap={0.75}>
                        <FlightTakeoff color={airplaneEnabled ? 'warning' : 'action'} fontSize="small" />
                        <Typography variant="body2" fontWeight={700}>飞行模式</Typography>
                      </Box>
                      <Typography variant="caption" color="text.secondary" display="block" mt={0.5}>
                        {!line.modem.present ? '配置可修改，设备恢复后自动应用' : network?.airplane_stage || (airplaneEnabled ? '移动射频已关闭' : '移动射频正常')}
                      </Typography>
                    </Box>
                    <Switch
                      color="warning"
                      checked={airplaneEnabled}
                      onChange={(_, enabled) => void toggleAirplaneMode(line.modem.line_id, enabled)}
                      disabled={!network || savingKey !== null}
                    />
                  </Box>
                  <Box display="flex" alignItems="center" gap={1} mt="auto" pt={1}>
                    {airplaneBusy && <CircularProgress size={16} />}
                    <Chip size="small" label={airplaneEnabled ? '已开启' : '已关闭'} color={airplaneEnabled ? 'warning' : 'default'} variant="outlined" />
                  </Box>
                </Box>

                <Box sx={{ p: 1.25, border: '1px solid', borderColor: 'divider', borderRadius: 1, height: '100%', display: 'flex', flexDirection: 'column' }}>
                  <Box display="flex" alignItems="center" gap={0.75}>
                    <Replay color="action" fontSize="small" />
                    <Typography variant="body2" fontWeight={700}>重启基带</Typography>
                  </Box>
                  <Typography variant="caption" color="text.secondary" display="block" mt={0.5}>
                    {line.modem.present
                      ? '仅重启当前线路的基带，驻网与数据连接会短暂中断。'
                      : '设备离线，使用保留的设备路径尝试恢复基带。'}
                  </Typography>
                  <Box display="flex" justifyContent="flex-end" mt="auto" pt={1}>
                    <Button
                      size="small"
                      color="warning"
                      startIcon={basebandRestartLine === line.modem.line_id ? <CircularProgress size={16} /> : <Replay />}
                      onClick={() => void restartBaseband(line.modem.line_id)}
                      disabled={basebandRestartLine !== null}
                    >
                      重启此基带
                    </Button>
                  </Box>
                </Box>
                  </Box>
                </CardContent>
              </Card>
            ) : undefined
            return (
              <Grid key={line.modem.line_id} size={workbench ? 12 : { xs: 12, lg: 6 }}>
                <Stack spacing={2}>
                {workbench && workbenchHeader}
                <Card variant="outlined" sx={{ height: '100%', opacity: line.modem.present ? 1 : 0.84 }}>
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
                  {workbench && (
                    <Tabs
                      value={workbenchTab}
                      onChange={(_, value: WorkbenchTab) => setWorkbenchTab(value)}
                      variant="scrollable"
                      scrollButtons="auto"
                      sx={{ px: 2, borderTop: 1, borderBottom: 1, borderColor: 'divider', minHeight: 44 }}
                    >
                      <Tab value="overview" label="概览" />
                      <Tab value="esim" label="eSIM" />
                      <Tab value="ims" label="IMS 与 Trunk" />
                      <Tab value="sms" label="短信" />
                      <Tab value="notifications" label="通知" />
                      <Tab value="automation" label="自动化" />
                    </Tabs>
                  )}
                  <CardContent sx={{ pt: 0 }}>
                    {workbench && workbenchTab === 'esim' && <Box mt={2}>{workbenchEsim}</Box>}
                    {workbench && workbenchTab === 'sms' && <Box mt={2}>{workbenchSms}</Box>}
                    {workbench && workbenchTab === 'automation' && <Box mt={2}>{workbenchAutomation}</Box>}
                    {workbench && workbenchTab === 'notifications' && <Box mt={2}>{workbenchNotifications}</Box>}

                    {(!workbench || workbenchTab === 'overview') && <Grid container spacing={1.75} mt={workbench ? 1.75 : 0}>
                      {basicInfoForLine && <Grid size={12}>{basicInfoForLine(line, overviewControls)}</Grid>}
                    </Grid>}

                    {(!workbench || workbenchTab === 'ims') && line.profile.volte_connection_enabled && (recovery || line.runtime.last_error) && (
                      <Alert severity={line.runtime.recovery_state === 'exhausted' ? 'error' : 'warning'} sx={{ mt: 2, py: 0.25 }}>
                        {recovery ?? line.runtime.last_error}
                        {line.runtime.next_retry_at && (
                          <Typography variant="caption" display="block">
                            下次尝试：{new Date(line.runtime.next_retry_at).toLocaleString()}
                          </Typography>
                        )}
                      </Alert>
                    )}

                    {(!workbench || workbenchTab === 'overview') && line.modem.slot_conflict && (
                      <Alert severity="error" sx={{ mt: 2, py: 0.25 }}>
                        多个基带解析到同一物理槽位，请检查 udev 的 MM_ID_PHYSDEV_UID 或设备树槽位映射。
                      </Alert>
                    )}

                    <Box display="flex" flexDirection="column">
                    {!isReader && (!workbench || workbenchTab === 'ims') && <Box display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider">
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
                        <Switch
                          checked={line.profile.volte_connection_enabled}
                          onChange={(_, enabled) => void toggleLine(line.modem.line_id, enabled)}
                          disabled={(line.modem.present && airplaneEnabled) || savingKey !== null}
                        />
                      </Box>
                    </Box>}
                    {(!workbench || workbenchTab === 'ims') && <Box display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
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
                          disabled={!vowifiLine || savingKey !== null}
                        />
                      </Box>
                    </Box>}
                    {(!workbench || workbenchTab === 'ims') && (
                    <Box order={0} display="flex" justifyContent="space-between" alignItems="center" mt={1.5} pt={1.5} borderTop={1} borderColor="divider" gap={1.5}>
                      <Box minWidth={0}>
                        <Box display="flex" alignItems="center" gap={0.75} flexWrap="wrap">
                          <SettingsEthernet color="action" fontSize="small" />
                          <Typography variant="body2" fontWeight={600}>Trunk 注册</Typography>
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
                    {!isReader && workbench && workbenchTab === 'ims' && line.profile.volte_connection_enabled && <Box mt={2} pt={2} borderTop={1} borderColor="divider"><Typography variant="subtitle2" fontWeight={700} mb={1.5}>VoLTE IMS 详情</Typography><LineVolteDetails line={line} /></Box>}
                    {workbench && workbenchTab === 'ims' && vowifiLine?.config.enabled && <Box mt={2} pt={2} borderTop={1} borderColor="divider"><Typography variant="subtitle2" fontWeight={700} mb={1.5}>VoWiFi 详情</Typography><LineVowifiDetails vowifi={vowifiLine} /></Box>}
                    {workbench && workbenchTab === 'ims' && trunkLine?.trunk.enabled && <Box mt={2} pt={2} borderTop={1} borderColor="divider"><Typography variant="subtitle2" fontWeight={700} mb={1.5}>Trunk 详情</Typography><LineTrunkDetails trunk={trunkLine} /></Box>}
                    </Box>
                  </CardContent>
                </Card>
                </Stack>
              </Grid>
            )
          })}
        </Grid>
      )}
    </>
  )

  return (
    <Stack spacing={2}>
      {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
      {success && <Alert severity="success" onClose={() => setSuccess(null)}>{success}</Alert>}

      {workbench ? (
        <Box sx={{
          display: 'grid',
          gridTemplateColumns: { xs: 'minmax(0, 1fr)', md: '260px minmax(0, 1fr)' },
          gap: 2,
          alignItems: 'stretch',
        }}>
          <Box minWidth={0} minHeight={0}>{renderLineList()}</Box>
          <Stack spacing={2} minWidth={0}>
            {lineContent}
          </Stack>
        </Box>
      ) : lineContent}

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
      <DataProxyDialog
        open={editingDataLineId !== null}
        lineId={editingDataLineId}
        controls={editingDataLineId ? networkByLineId.get(editingDataLineId) ?? null : null}
        onClose={() => setEditingDataLineId(null)}
        onSaved={handleDataProxySaved}
      />
    </Stack>
  )
}
