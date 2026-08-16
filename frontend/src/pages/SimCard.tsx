import { useState, useEffect, useCallback, type ReactNode } from 'react'
import {
  Box,
  Typography,
  Tabs,
  Tab,
  Card,
  CardContent,
  CardHeader,
  Button,
  Chip,
  CircularProgress,
  IconButton,
  Tooltip,
  TextField,
  Snackbar,
  Alert,
  Stack,
  LinearProgress,
  MenuItem,
  Paper,
  Dialog,
  DialogContent,
  DialogTitle,
  ToggleButton,
  ToggleButtonGroup,
  FormControlLabel,
  Switch,
} from '@mui/material'
import Grid from '@mui/material/Grid'
import {
  SimCard as SimIcon,
  Visibility,
  VisibilityOff,
  Edit,
  Check,
  Close,
  Language as LanguageIcon,
  Lock as LockIcon,
  Storage as StorageIcon,
  CheckCircle,
  WarningAmber,
  Memory,
  Build,
} from '@mui/icons-material'
import { useSearchParams } from 'react-router-dom'
import { api, type VolteLineControlResponse } from '../api/current'
import type { AutomationTarget, DeviceInfo, EsimEuiccInfo, EsimLpacStatusResponse, EsimProfile, EsimReaderConfig, SimInfo } from '../api/types'
import ErrorSnackbar from '../components/ErrorSnackbar'
import GithubDownloadProxyControl from '../components/GithubDownloadProxyControl'
import ModemLinesPanel from './sim/ModemLinesPanel'
import CarrierProfilesPanel from './sim/CarrierProfilesPanel'
import SimReaderPanel from './sim/SimReaderPanel'
import { LineNetworkOverview } from './sim/LineCellularSettings'
import EsimManagerPage from './EsimManager'
import { maskedIccid, modemSlotLabel, shortLineId } from '../components/modemLineFormat'
import AutomationCenter from './AutomationCenter'
import NotificationCenterPage from './NotificationCenter'
import SMSPage from './SMS'

function lineNotificationScope(line: VolteLineControlResponse | null) {
  if (!line) return undefined
  return line.modem.line_kind === 'reader'
    ? `reader:${line.modem.modem_id.replace(/^reader:/, '')}`
    : line.modem.line_id
}

function lineAutomationTarget(line: VolteLineControlResponse | null): AutomationTarget | undefined {
  if (!line) return undefined
  return line.modem.line_kind === 'reader'
    ? { kind: 'standalone_sim_slot', slot_id: line.modem.modem_id.replace(/^reader:/, '') }
    : { kind: 'modem_line', line_id: line.modem.line_id }
}

function getSensitiveStyle(show: boolean) {
  return {
    filter: show ? 'none' : 'blur(5px)',
    transition: 'filter 0.3s ease',
    userSelect: show ? 'auto' : 'none',
    cursor: show ? 'text' : 'default',
  } as const
}

function formatSimType(simType?: string, esimStatus?: string) {
  // 1. 优先取 simType (如果明确是 physical 或 esim)
  if (simType === 'physical') return '物理 SIM 卡';
  if (simType === 'esim') return 'eSIM 卡';

  // 2. 其次根据有没有 eUICC 芯片判断 (esimStatus 有明确的 eUICC 状态)
  if (esimStatus && esimStatus !== 'unknown') {
    return 'eSIM 卡';
  }

  return '未知';
}

function formatLockStatus(lockStatus?: string) {
  if (!lockStatus) return '未知';
  switch (lockStatus) {
    case 'none': return '未加锁';
    case 'sim-pin': return 'PIN1 已锁定';
    case 'sim-pin2': return 'PIN2 已锁定';
    case 'sim-puk': return 'PIN1 已锁死，需 PUK1 解锁';
    case 'sim-puk2': return 'PIN2 已锁死，需 PUK2 解锁';
    default: return `已锁定 (${lockStatus})`;
  }
}

function formatUnlockRetries(pin1?: number, puk1?: number, pin2?: number, puk2?: number) {
  if (pin1 === undefined && puk1 === undefined && pin2 === undefined && puk2 === undefined) return 'N/A';

  const isPin1Low = pin1 !== undefined && pin1 < 3;
  const isPuk1Low = puk1 !== undefined && puk1 < 5;
  const isPin2Low = pin2 !== undefined && pin2 < 3;
  const isPuk2Low = puk2 !== undefined && puk2 < 5;

  const renderItem = (label: string, value?: number, isLow?: boolean) => {
    const displayVal = value !== undefined ? `${value}次` : '-';
    return (
      <Typography
        variant="body2"
        component="span"
        sx={{
          fontSize: '0.825rem',
          color: isLow ? 'error.main' : 'text.primary',
          fontWeight: isLow ? 600 : 400,
        }}
      >
        {label}: {displayVal}
      </Typography>
    );
  };

  return (
    <Stack spacing={0.5}>
      <Box display="flex" gap={1.25} alignItems="center">
        {renderItem('PIN1', pin1, isPin1Low)}
        {renderItem('PUK1', puk1, isPuk1Low)}
      </Box>
      <Box display="flex" gap={1.25} alignItems="center">
        {renderItem('PIN2', pin2, isPin2Low)}
        {renderItem('PUK2', puk2, isPuk2Low)}
      </Box>
    </Stack>
  );
}

function formatOperator(name?: string, code?: string) {
  if (!name && !code) return 'N/A';
  if (name && code) return `${name} (${code})`;
  return name || code || 'N/A';
}

function InfoField({ label, value, sensitive = false, showSensitive, extra }: {
  label: string
  value: React.ReactNode
  sensitive?: boolean
  showSensitive?: boolean
  extra?: React.ReactNode
}) {
  return (
    <Box>
      <Typography variant="caption" color="text.secondary" sx={{ fontWeight: 500 }}>
        {label}
      </Typography>
      <Box display="flex" alignItems="center" gap={0.5} mt={0.25} minHeight="20px">
        <Typography
          variant="body2"
          component="div"
          sx={{
            fontSize: '0.825rem',
            wordBreak: 'break-all',
            ...(sensitive ? getSensitiveStyle(!!showSensitive) : {})
          }}
        >
          {value}
        </Typography>
        {extra}
      </Box>
    </Box>
  )
}

function SmsCapacityProgress({ used, total }: { used?: number, total?: number }) {
  if (used === undefined || total === undefined) return <Typography variant="body2" sx={{ fontSize: '0.825rem' }}>N/A</Typography>;
  const percentage = Math.min((used / total) * 100, 100);
  const isFull = used >= total;
  return (
    <Box display="flex" flexDirection="column" width="100%" gap={0.25}>
      <Box display="flex" justifyContent="space-between" alignItems="center">
        <Typography variant="body2" sx={{ fontWeight: 600, fontSize: '0.825rem' }}>
          {used} / {total} 条
        </Typography>
        {isFull && (
          <Chip label="已满" color="error" size="small" sx={{ height: 16, fontSize: '0.65rem' }} />
        )}
      </Box>
      <LinearProgress
        variant="determinate"
        value={percentage}
        color={isFull ? "error" : percentage > 80 ? "warning" : "primary"}
        sx={{ height: 5, borderRadius: 3, mt: 0.5 }}
      />
    </Box>
  );
}

function WorkbenchOverview({ line }: { line: VolteLineControlResponse }) {
  const [simInfo, setSimInfo] = useState<SimInfo | null>(null)
  const [networkInfo, setNetworkInfo] = useState<{ operator_name: string, registration_status: string, signal_strength: number } | null>(null)
  const [vowifi, setVowifi] = useState<Awaited<ReturnType<typeof api.getVowifiLine>>['data'] | null>(null)

  useEffect(() => {
    let active = true
    void Promise.allSettled([
      api.getSimInfo(line.modem.line_id),
      api.getNetworkInfo(line.modem.line_id),
      api.getVowifiLine(line.modem.line_id),
    ]).then(([simResult, networkResult, vowifiResult]) => {
      if (!active) return
      setSimInfo(simResult.status === 'fulfilled' ? simResult.value.data ?? null : null)
        setNetworkInfo(networkResult.status === 'fulfilled' ? networkResult.value.data ?? null : null)
        setVowifi(vowifiResult.status === 'fulfilled' ? vowifiResult.value.data ?? null : null)
      })
    return () => { active = false }
  }, [line.modem.line_id])

  const progress = (() => {
    const showVowifi = vowifi?.runtime_registered
      || (!line.runtime.registered && (vowifi?.config.enabled || line.modem.line_kind === 'reader'))
    if (showVowifi) {
      const stages = ['SIM 身份', 'ePDG 接入', 'IKE 隧道', 'ESP 通道', 'IMS 注册', '语音就绪']
      const stageIndex: Record<string, number> = {
        identity_ready: 0, profile_matched: 0, sim_auth_ready: 0,
        epdg_ready: 1, ike_ready: 2, child_sa_ready: 2, esp_ready: 3,
        ims_registered: 4, sms_ready: 4, voice_ready: 5,
      }
      return {
        access: 'VoWiFi',
        stages,
        current: stageIndex[vowifi?.runtime_stage ?? ''] ?? (vowifi?.runtime_registered ? 4 : -1),
        status: vowifi?.runtime_stage || '等待连接',
      }
    }
    if (line.runtime.registered || line.profile.volte_connection_enabled) {
      const stages = ['SIM 身份', '无线接入', 'IMS Bearer', 'P-CSCF', 'IMS 注册', '注册就绪']
      const stageIndex: Record<string, number> = {
        identity: 0, identity_aka: 0, radio: 1, modem: 1,
        ims_context: 2, bearer: 2, bearer_dual: 2, bearer_ipv4: 2, bearer_ipv6: 2, ip_config: 2,
        pcscf: 3, ipv6_preflight: 3, register_initial: 4, ipsec: 4, register_authenticated: 4,
        registered: 5, register_refresh: 5, register_ipsec: 5, register_udp: 5,
      }
      return { access: 'VoLTE', stages, current: line.runtime.registered ? 5 : (stageIndex[line.runtime.stage] ?? -1), status: line.runtime.stage || '等待连接' }
    }
    const stages = ['SIM 就绪', '已驻网', '数据已连', '语音可用']
    const hasSim = Boolean(simInfo?.present || line.modem.sim_iccid)
    const registered = ['registered', 'connected'].includes(line.modem.state)
    const connected = line.modem.state === 'connected'
    const current = !hasSim ? -1 : !registered ? 0 : !connected ? 1 : 2
    return { access: 'CS', stages, current, status: line.modem.state || '等待连接' }
  })()
  const connectionReady = progress.current === progress.stages.length - 1
  const connectionWaiting = line.modem.present && progress.current >= 0
  const connectionLabel = connectionReady
    ? `${progress.access} 已就绪`
    : progress.access === 'VoWiFi' && vowifi?.runtime_registered
      ? 'VoWiFi · IMS 已注册'
      : progress.access === 'CS' && line.modem.state === 'connected'
        ? 'CS · 数据已连接'
    : line.modem.present
      ? `${progress.access} · 等待连接`
      : '设备离线'

  return (
    <Stack spacing={1.5}>
      <Paper variant="outlined" sx={{ p: { xs: 1.5, sm: 2 }, bgcolor: 'background.default' }}>
        <Box display="flex" justifyContent="space-between" alignItems="flex-start" gap={2} flexWrap="wrap">
          <Box minWidth={0}>
            <Typography variant="h6" fontWeight={800} noWrap>{modemSlotLabel(line.modem)} · {simInfo?.operator_name || line.modem.operator_id || '未知运营商'}</Typography>
            <Typography variant="caption" color="text.secondary">线路 {shortLineId(line.modem.line_id)} · {networkInfo?.signal_strength ?? 0}% 信号 · {networkInfo?.registration_status || '未注册'}</Typography>
          </Box>
          <Chip
            icon={connectionReady ? <CheckCircle /> : <WarningAmber />}
            label={connectionLabel}
            color={connectionReady ? 'success' : connectionWaiting ? 'warning' : 'default'}
          />
        </Box>
        <Box display="grid" gridTemplateColumns={`repeat(${progress.stages.length}, minmax(0, 1fr))`} gap={0.75} mt={2}>
          {progress.stages.map((stage, index) => <Tooltip key={stage} title={stage}><Box sx={{ height: 8, borderRadius: 1, bgcolor: index <= progress.current ? 'success.main' : 'action.disabledBackground' }} /></Tooltip>)}
        </Box>
        <Box display="flex" justifyContent="space-between" mt={0.75} gap={1}>
          <Typography variant="caption" color="text.secondary">{progress.access} · {progress.status}</Typography>
          <Typography variant="caption" color="text.secondary">{Math.max(0, progress.current + 1)}/{progress.stages.length} 阶段</Typography>
        </Box>
      </Paper>
    </Stack>
  )
}

type EsimControlMode = 'auto' | 'enabled' | 'disabled'

const DEFAULT_ESIM_READER_CONFIG: EsimReaderConfig = {
  apdu_backend: 'qmi',
  http_backend: 'curl',
  at_device: '',
  qmi_device: '',
  qmi_uim_slot: 0,
  pcsc_reader_name: '',
  pcsc_reader_index: null,
  mbim_device: '',
  mbim_uim_slot: 0,
  mbim_use_proxy: false,
  mbim_skip_slot_mapping: false,
}

function EsimWorkbenchPanel({ line, onControlChanged }: { line: VolteLineControlResponse | null, onControlChanged: (control: boolean | null) => void }) {
  const esimDetected = Boolean(line && (line.modem.sim_type === 'esim' || line.modem.esim_status === 'no-profiles' || line.modem.esim_status === 'with-profiles'))
  const initialMode: EsimControlMode = line?.profile.esim_control === true ? 'enabled' : line?.profile.esim_control === false ? 'disabled' : 'auto'
  const [controlMode, setControlMode] = useState<EsimControlMode>(initialMode)
  const [euicc, setEuicc] = useState<EsimEuiccInfo | null>(null)
  const [profiles, setProfiles] = useState<EsimProfile[]>([])
  const [lpac, setLpac] = useState<EsimLpacStatusResponse | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [controlError, setControlError] = useState<string | null>(null)
  const [controlSaving, setControlSaving] = useState(false)
  const [loading, setLoading] = useState(false)
  const [lpacRepairing, setLpacRepairing] = useState(false)
  const [lpacAssetUrl, setLpacAssetUrl] = useState('')
  const [lpacConfig, setLpacConfig] = useState<EsimReaderConfig>(DEFAULT_ESIM_READER_CONFIG)
  const [lpacConfigSaving, setLpacConfigSaving] = useState(false)
  const [lpacSuccess, setLpacSuccess] = useState<string | null>(null)
  const [lpacConfigError, setLpacConfigError] = useState<string | null>(null)
  const [reloadKey, setReloadKey] = useState(0)
  const [managerOpen, setManagerOpen] = useState(false)
  const [lpacSettingsOpen, setLpacSettingsOpen] = useState(false)
  const esimEnabled = controlMode === 'enabled' || (controlMode === 'auto' && esimDetected)
  const lineId = line?.modem.line_id

  useEffect(() => {
    setControlMode(initialMode)
    setEuicc(null)
    setProfiles([])
    setLpac(null)
    setError(null)
    setControlError(null)
    setLpacSuccess(null)
    setLpacConfigError(null)
    setManagerOpen(false)
    setLpacSettingsOpen(false)
    setLpacConfig(DEFAULT_ESIM_READER_CONFIG)
    if (lineId) {
      void api.getLineEsimReaderConfig(lineId).then((response) => {
        if (response.data) setLpacConfig(response.data)
      }).catch((err) => setLpacConfigError(err instanceof Error ? err.message : String(err)))
    }
  }, [initialMode, lineId])

  useEffect(() => {
    let active = true
    if (!lineId) return () => { active = false }
    setLoading(esimEnabled)
    setError(null)
    void (async () => {
      try {
        const lpacResponse = await api.getEsimLpacStatus()
        if (!active) return
        const nextLpac = lpacResponse.data ?? null
        setLpac(nextLpac)
        if (!nextLpac) {
          setError('暂无法读取 lpac 状态，请稍后重试。')
          return
        }
        if (!nextLpac.usable || !esimEnabled) return

        const [euiccResponse, profileResponse] = await Promise.all([
          api.getEsimEuicc(lineId),
          api.getEsimProfiles(lineId),
        ])
        if (!active) return
        setEuicc(euiccResponse.data ?? null)
        setProfiles(profileResponse.data?.profiles ?? [])
      } catch (err) {
        if (active) setError(err instanceof Error ? err.message : String(err))
      } finally {
        if (active) setLoading(false)
      }
    })()
    return () => { active = false }
  }, [esimEnabled, lineId, reloadKey])

  const repairLpac = async () => {
    setLpacRepairing(true)
    setLpacConfigError(null)
    setLpacSuccess(null)
    try {
      const response = await api.repairEsimLpac({
        asset_url: lpacAssetUrl.trim() || undefined,
      })
      setLpacSuccess(response.data?.message || 'lpac 安装/修复完成')
      setReloadKey((value) => value + 1)
    } catch (err) {
      setLpacConfigError(err instanceof Error ? err.message : String(err))
    } finally {
      setLpacRepairing(false)
    }
  }

  const saveLpacConfig = async () => {
    if (!lineId) return
    setLpacConfigSaving(true)
    setLpacConfigError(null)
    try {
      const response = await api.setLineEsimReaderConfig(lineId, {
        ...lpacConfig,
        qmi_uim_slot: Number(lpacConfig.qmi_uim_slot) || 0,
        pcsc_reader_index: lpacConfig.pcsc_reader_index === null || lpacConfig.pcsc_reader_index === undefined ? null : Number(lpacConfig.pcsc_reader_index),
        mbim_uim_slot: Number(lpacConfig.mbim_uim_slot) || 0,
      })
      if (response.data) setLpacConfig(response.data)
      setLpacSuccess('当前线路的 lpac 接口配置已保存')
      setReloadKey((value) => value + 1)
    } catch (err) {
      setLpacConfigError(err instanceof Error ? err.message : String(err))
    } finally {
      setLpacConfigSaving(false)
    }
  }

  const updateControlMode = async (nextMode: EsimControlMode) => {
    if (!lineId || nextMode === controlMode) return
    const control = nextMode === 'auto' ? null : nextMode === 'enabled'
    setControlSaving(true)
    setControlError(null)
    try {
      await api.setLineEsimControl(lineId, control)
      setControlMode(nextMode)
      setEuicc(null)
      setProfiles([])
      setError(null)
      onControlChanged(control)
    } catch (err) {
      setControlError(err instanceof Error ? err.message : String(err))
    } finally {
      setControlSaving(false)
    }
  }

  if (!line) return <Typography color="text.secondary">选择线路后查看 eSIM 状态</Typography>

  const used = euicc?.memory_total_kb !== undefined && euicc.memory_available_kb !== undefined ? Math.max(0, euicc.memory_total_kb - euicc.memory_available_kb) : null
  const usage = used !== null && euicc?.memory_total_kb ? Math.min(100, (used / euicc.memory_total_kb) * 100) : null
  return (
    <Box sx={{ display: 'flex', flexDirection: 'column' }}>
      <Box display="flex" justifyContent="space-between" alignItems="flex-start" gap={1} mb={2}>
        <Box><Box display="flex" alignItems="center" gap={1}><Memory color={esimEnabled ? 'primary' : 'disabled'} /><Typography variant="subtitle1" fontWeight={800}>eSIM 管理</Typography></Box><Typography variant="caption" color="text.secondary">{line.modem.model || '当前线路'} · {shortLineId(line.modem.line_id)}</Typography></Box>
        <Stack direction="row" spacing={1} flexWrap="wrap" justifyContent="flex-end">
          <Button size="small" variant="outlined" startIcon={<Build />} onClick={() => setLpacSettingsOpen(true)}>lpac 接口</Button>
          <Button size="small" variant="outlined" onClick={() => setManagerOpen(true)} disabled={!esimEnabled}>完整管理</Button>
        </Stack>
      </Box>

      <ToggleButtonGroup
        exclusive
        size="small"
        value={controlMode}
        onChange={(_, value: EsimControlMode | null) => value && void updateControlMode(value)}
        disabled={controlSaving}
        aria-label="eSIM 控制模式"
        sx={{ mb: 1.5, alignSelf: 'flex-start' }}
      >
        <ToggleButton value="auto" sx={{ px: 2.5 }}>自动</ToggleButton>
        <ToggleButton value="enabled" sx={{ px: 2.5 }}>开启</ToggleButton>
        <ToggleButton value="disabled" sx={{ px: 2.5 }}>关闭</ToggleButton>
      </ToggleButtonGroup>
      <Box display="flex" alignItems="center" gap={1} mb={1.5}>
        {controlSaving && <CircularProgress size={18} />}
        <Typography variant="caption" color="text.secondary">
          {controlMode === 'auto'
            ? esimDetected ? '已自动检测到 eUICC，管理面板已启用' : '未检测到 eUICC，管理面板保持关闭'
            : controlMode === 'enabled' ? '已强制显示并启用该线路的 eSIM 管理' : '已强制关闭该线路的 eSIM 管理'}
        </Typography>
      </Box>
      {controlError && <Alert severity="error" sx={{ mb: 1.5 }}>{controlError}</Alert>}
      {!esimEnabled && <Alert severity={controlMode === 'disabled' ? 'warning' : 'info'}>{controlMode === 'disabled' ? 'eSIM 管理已强制关闭。' : '自动模式下未识别到 eSIM；可切换到“开启”以管理外置 eUICC。'}</Alert>}
      {esimEnabled && loading && <Box display="grid" sx={{ placeItems: 'center', minHeight: 180 }}><CircularProgress size={26} /></Box>}
      {esimEnabled && !loading && error && <Alert severity="warning" sx={{ mb: 1.5 }}>{error}</Alert>}
      {esimEnabled && !loading && !error && lpac?.usable && <>
        <Box display="grid" gridTemplateColumns={{ xs: 'minmax(0, 1fr)', sm: 'repeat(3, minmax(0, 1fr))' }} gap={1.25} mb={2}>
          <Paper variant="outlined" sx={{ p: 1.25 }}>
            <Typography variant="caption" color="text.secondary">EID</Typography>
            <Typography variant="body2" fontFamily="monospace" sx={{ wordBreak: 'break-all' }}>{euicc?.eid ? `${euicc.eid.slice(0, 6)}···${euicc.eid.slice(-6)}` : '未读取'}</Typography>
          </Paper>
          <Paper variant="outlined" sx={{ p: 1.25 }}>
            <Typography variant="caption" color="text.secondary">Profile 数量</Typography>
            <Typography variant="body2" fontWeight={700}>{profiles.length}</Typography>
          </Paper>
          <Paper variant="outlined" sx={{ p: 1.25 }}>
            <Typography variant="caption" color="text.secondary">存储占用</Typography>
            {usage !== null ? (<>
              <Typography variant="body2">{used} / {euicc?.memory_total_kb} KB</Typography>
              <LinearProgress variant="determinate" value={usage} color={usage > 85 ? 'warning' : 'primary'} sx={{ mt: 0.5, height: 6, borderRadius: 1 }} />
            </>) : <Typography variant="body2" color="text.secondary">未读取</Typography>}
          </Paper>
        </Box>
        <Box display="grid" gridTemplateColumns={{ xs: 'minmax(0, 1fr)', lg: 'repeat(2, minmax(0, 1fr))' }} gap={0.75} sx={{ overflowY: 'auto', maxHeight: 320 }}>
          {profiles.map((profile) => <Box key={profile.iccid} display="flex" alignItems="center" justifyContent="space-between" gap={1} sx={{ p: 1, border: '1px solid', borderColor: 'divider', borderRadius: 1 }}><Box minWidth={0}><Typography variant="body2" fontWeight={600} noWrap>{profile.name || profile.provider || '未命名 Profile'}</Typography><Typography variant="caption" color="text.secondary" noWrap>{maskedIccid(profile.iccid)} · {profile.state}</Typography></Box><Chip size="small" label={profile.state === 'enabled' || profile.state === 'active' ? '启用' : '可用'} color={profile.state === 'enabled' || profile.state === 'active' ? 'success' : 'default'} /></Box>)}
          {profiles.length === 0 && <Typography variant="body2" color="text.secondary">尚未读取到 Profile</Typography>}
        </Box>
      </>}
      <Dialog open={managerOpen} onClose={() => setManagerOpen(false)} fullWidth maxWidth="lg"><DialogTitle sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>eSIM 完整管理<IconButton onClick={() => setManagerOpen(false)} aria-label="关闭"><Close /></IconButton></DialogTitle><DialogContent dividers><EsimManagerPage lineId={line.modem.line_id} /></DialogContent></Dialog>
      <Dialog open={lpacSettingsOpen} onClose={() => setLpacSettingsOpen(false)} fullWidth maxWidth="lg">
        <DialogTitle sx={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          lpac 工具与当前线路接口
          <IconButton onClick={() => setLpacSettingsOpen(false)} aria-label="关闭"><Close /></IconButton>
        </DialogTitle>
        <DialogContent dividers>
          <Stack spacing={2.5}>
            {lpac ? (
              <Alert severity={lpac.usable ? 'success' : 'warning'}>
                {lpac.usable
                  ? `lpac 已就绪：${lpac.path}${lpac.source ? ` · ${lpac.source}` : ''}`
                  : `未检测到可用 lpac。架构：${lpac.arch || '不支持'}；glibc：${lpac.glibc_version || '未知'}；安装包：${lpac.asset_name || '无匹配资源'}。${lpac.message}`}
              </Alert>
            ) : <Box display="flex" justifyContent="center" py={3}><CircularProgress size={24} /></Box>}
            {lpacConfigError && <Alert severity="error" onClose={() => setLpacConfigError(null)}>{lpacConfigError}</Alert>}
            {lpacSuccess && <Alert severity="success" onClose={() => setLpacSuccess(null)}>{lpacSuccess}</Alert>}

            <Box>
              <Typography variant="subtitle2" fontWeight={700} mb={1.5}>安装与修复</Typography>
              <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1.5} alignItems={{ sm: 'center' }}>
                <Button variant="contained" startIcon={lpacRepairing ? <CircularProgress size={16} color="inherit" /> : <Build />} disabled={lpacRepairing || !lpac || (!lpac.asset_name && !lpacAssetUrl.trim())} onClick={() => void repairLpac()}>
                  {lpac?.usable ? '重新下载并修复' : '下载并自动安装'}
                </Button>
              </Stack>
              <Box mt={1.5}><GithubDownloadProxyControl compact /></Box>
              <TextField fullWidth size="small" label="第三方 lpac 压缩包 URL（可选）" value={lpacAssetUrl} onChange={(event) => setLpacAssetUrl(event.target.value)} placeholder="https://example.com/lpac-linux-aarch64.zip" sx={{ mt: 1.5 }} />
              <Typography variant="caption" color="text.secondary" display="block" mt={1.25}>安装位置：{lpac?.path || '由后端自动选择'}</Typography>
            </Box>

            <Box borderTop={1} borderColor="divider" pt={2.5}>
              <Typography variant="subtitle2" fontWeight={700} mb={1.5}>当前线路接口</Typography>
              <Grid container spacing={1.5}>
                <Grid size={{ xs: 12, sm: 6 }}>
                  <TextField select fullWidth size="small" label="APDU 后端" value={lpacConfig.apdu_backend} onChange={(event) => setLpacConfig((current) => ({ ...current, apdu_backend: event.target.value }))}>
                    <MenuItem value="qmi">QMI</MenuItem>
                    <MenuItem value="qmi_qrtr">QMI over QRTR</MenuItem>
                    <MenuItem value="at">AT 逻辑通道</MenuItem>
                    <MenuItem value="at_csim">AT+CSIM</MenuItem>
                    <MenuItem value="pcsc">PC/SC 读卡器</MenuItem>
                    <MenuItem value="mbim">MBIM</MenuItem>
                  </TextField>
                </Grid>
                <Grid size={{ xs: 12, sm: 6 }}><TextField select fullWidth size="small" label="HTTP 后端" value={lpacConfig.http_backend} onChange={(event) => setLpacConfig((current) => ({ ...current, http_backend: event.target.value }))}><MenuItem value="curl">cURL</MenuItem><MenuItem value="stdio">标准输入输出</MenuItem></TextField></Grid>

                {['qmi', 'qmi_qrtr'].includes(lpacConfig.apdu_backend) && <>
                  <Grid size={{ xs: 12, sm: 6 }}><TextField fullWidth size="small" label="QMI 设备覆盖（可选）" value={lpacConfig.qmi_device} onChange={(event) => setLpacConfig((current) => ({ ...current, qmi_device: event.target.value }))} placeholder="留空使用当前线路" /></Grid>
                  <Grid size={{ xs: 12, sm: 6 }}><TextField fullWidth size="small" type="number" label="QMI UIM 槽位覆盖" value={lpacConfig.qmi_uim_slot || ''} onChange={(event) => setLpacConfig((current) => ({ ...current, qmi_uim_slot: Number(event.target.value) || 0 }))} helperText="0 = 使用当前线路槽位" /></Grid>
                </>}
                {['at', 'at_csim'].includes(lpacConfig.apdu_backend) && <Grid size={12}><TextField required fullWidth size="small" label="AT 设备端口" value={lpacConfig.at_device} onChange={(event) => setLpacConfig((current) => ({ ...current, at_device: event.target.value }))} placeholder="/dev/ttyUSB2" /></Grid>}
                {lpacConfig.apdu_backend === 'pcsc' && <>
                  <Grid size={{ xs: 12, sm: 8 }}><TextField fullWidth size="small" label="PC/SC 读卡器名称（可选）" value={lpacConfig.pcsc_reader_name} onChange={(event) => setLpacConfig((current) => ({ ...current, pcsc_reader_name: event.target.value }))} placeholder="留空自动选择" /></Grid>
                  <Grid size={{ xs: 12, sm: 4 }}><TextField fullWidth size="small" type="number" label="接口索引（可选）" value={lpacConfig.pcsc_reader_index ?? ''} onChange={(event) => setLpacConfig((current) => ({ ...current, pcsc_reader_index: event.target.value === '' ? null : Number(event.target.value) }))} /></Grid>
                </>}
                {lpacConfig.apdu_backend === 'mbim' && <>
                  <Grid size={{ xs: 12, sm: 8 }}><TextField required fullWidth size="small" label="MBIM 设备" value={lpacConfig.mbim_device} onChange={(event) => setLpacConfig((current) => ({ ...current, mbim_device: event.target.value }))} placeholder="/dev/cdc-wdm0" /></Grid>
                  <Grid size={{ xs: 12, sm: 4 }}><TextField fullWidth size="small" type="number" label="MBIM UIM 槽位" value={lpacConfig.mbim_uim_slot || ''} onChange={(event) => setLpacConfig((current) => ({ ...current, mbim_uim_slot: Number(event.target.value) || 0 }))} helperText="0 = lpac 默认槽位" /></Grid>
                  <Grid size={{ xs: 12, sm: 6 }}><FormControlLabel control={<Switch checked={lpacConfig.mbim_use_proxy} onChange={(_, checked) => setLpacConfig((current) => ({ ...current, mbim_use_proxy: checked }))} />} label="使用 mbim-proxy" /></Grid>
                  <Grid size={{ xs: 12, sm: 6 }}><FormControlLabel control={<Switch checked={lpacConfig.mbim_skip_slot_mapping} onChange={(_, checked) => setLpacConfig((current) => ({ ...current, mbim_skip_slot_mapping: checked }))} />} label="保留当前槽位映射" /></Grid>
                </>}
              </Grid>
              <Box display="flex" justifyContent="flex-end" mt={2}>
                <Button variant="contained" onClick={() => void saveLpacConfig()} disabled={lpacConfigSaving}>{lpacConfigSaving ? '保存中…' : '保存当前线路接口'}</Button>
              </Box>
            </Box>
          </Stack>
        </DialogContent>
      </Dialog>
    </Box>
  )
}

function SimBasicInfo({ line, controls }: { line: VolteLineControlResponse, controls?: ReactNode }) {
  const lineId = line.modem.line_id
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [showSensitive, setShowSensitive] = useState(false)
  const [simInfo, setSimInfo] = useState<SimInfo | null>(null)
  const [deviceInfo, setDeviceInfo] = useState<DeviceInfo | null>(null)

  const [editingPhone, setEditingPhone] = useState(false)
  const [editingSmsc, setEditingSmsc] = useState(false)
  const [phoneInput, setPhoneInput] = useState('')
  const [smscInput, setSmscInput] = useState('')
  const [savingPhone, setSavingPhone] = useState(false)
  const [savingSmsc, setSavingSmsc] = useState(false)

  const isPhoneEmpty = !simInfo?.phone_numbers?.length
  const isSmscEmpty = !simInfo?.sms_center

  const [snackbar, setSnackbar] = useState<{ open: boolean; message: string; severity: 'success' | 'error' }>({
    open: false,
    message: '',
    severity: 'success',
  })

  const showMsg = (message: string, severity: 'success' | 'error') => {
    setSnackbar({ open: true, message, severity })
  }

  const validatePhoneStr = (val: string) => /^\+?\d+$/.test(val.trim())

  const loadData = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [simResult, deviceResult] = await Promise.allSettled([
        api.getSimInfo(lineId),
        api.getDeviceInfo(lineId),
      ])
      if (simResult.status === 'rejected') throw simResult.reason
      if (simResult.value.data) setSimInfo(simResult.value.data)
      setDeviceInfo(deviceResult.status === 'fulfilled' ? deviceResult.value.data ?? null : null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [lineId])

  const handleSavePhone = async () => {
    if (!phoneInput.trim()) {
      setEditingPhone(false)
      return
    }
    if (!validatePhoneStr(phoneInput)) {
      showMsg('号码格式错误，只能包含数字和开头的+', 'error')
      return
    }
    setSavingPhone(true)
    try {
      await api.updateSimCache(lineId, { phone_number: phoneInput.trim() })
      showMsg('号码缓存已更新', 'success')
      setEditingPhone(false)
      void loadData()
    } catch (err) {
      showMsg(err instanceof Error ? err.message : String(err), 'error')
    } finally {
      setSavingPhone(false)
    }
  }

  const handleSaveSmsc = async () => {
    if (!smscInput.trim()) {
      setEditingSmsc(false)
      return
    }
    if (!validatePhoneStr(smscInput)) {
      showMsg('号码格式错误，只能包含数字和开头的+', 'error')
      return
    }
    setSavingSmsc(true)
    try {
      await api.updateSimCache(lineId, { sms_center: smscInput.trim() })
      showMsg('短信中心缓存已更新', 'success')
      setEditingSmsc(false)
      void loadData()
    } catch (err) {
      showMsg(err instanceof Error ? err.message : String(err), 'error')
    } finally {
      setSavingSmsc(false)
    }
  }

  useEffect(() => {
    void loadData()
  }, [loadData])

  if (loading) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" minHeight="30vh">
        <CircularProgress />
      </Box>
    )
  }

  return (
    <Box>
      <ErrorSnackbar error={error} onClose={() => setError(null)} />
      <Grid container spacing={3} alignItems="stretch">
        <Grid size={{ xs: 12, md: 5 }} sx={{ display: 'flex' }}>
          <Box display="flex" flexDirection="column" gap={2} sx={{ flexGrow: 1, minWidth: 0 }}>
            {/* Card 1: SIM卡基本标识 */}
            <Card>
              <CardHeader
                avatar={<SimIcon color="primary" />}
                title="SIM 卡基本标识"
                titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
                action={
                  <Tooltip title={showSensitive ? '隐藏敏感信息' : '显示完整信息'}>
                    <IconButton
                      size="small"
                      onClick={() => setShowSensitive((value) => !value)}
                      color="primary"
                    >
                      {showSensitive ? <VisibilityOff fontSize="small" /> : <Visibility fontSize="small" />}
                    </IconButton>
                  </Tooltip>
                }
              />
              <CardContent sx={{ pt: 0 }}>
                <Grid container spacing={2}>
                  <Grid size={6}>
                    <InfoField
                      label="SIM 状态"
                      value={
                        <Chip
                          label={simInfo?.present ? '已插入' : '未插入'}
                          color={simInfo?.present ? 'success' : 'error'}
                          size="small"
                          sx={{ height: 20, fontSize: '0.75rem' }}
                        />
                      }
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="SIM 卡类型"
                      value={formatSimType(simInfo?.sim_type, simInfo?.esim_status)}
                    />
                  </Grid>
                  <Grid size={6}>
                    {editingPhone ? (
                      <Box>
                        <Typography variant="caption" color="text.secondary" sx={{ fontWeight: 500 }}>
                          手机号
                        </Typography>
                        <Box display="flex" alignItems="center" gap={0.5} mt={0.25}>
                          <TextField
                            size="small"
                            variant="standard"
                            placeholder="+86..."
                            value={phoneInput}
                            onChange={(e) => setPhoneInput(e.target.value)}
                            disabled={savingPhone}
                            inputProps={{ style: { fontSize: '0.825rem' } }}
                          />
                          <IconButton size="small" color="success" onClick={() => void handleSavePhone()} disabled={savingPhone}>
                            {savingPhone ? <CircularProgress size={14} /> : <Check fontSize="small" />}
                          </IconButton>
                          <IconButton size="small" color="error" onClick={() => setEditingPhone(false)} disabled={savingPhone}>
                            <Close fontSize="small" />
                          </IconButton>
                        </Box>
                      </Box>
                    ) : (
                      <InfoField
                        label="手机号"
                        sensitive
                        showSensitive={showSensitive}
                        value={simInfo?.phone_numbers?.length ? simInfo.phone_numbers.join(', ') : 'N/A'}
                        extra={
                          showSensitive && (isPhoneEmpty || simInfo?.phone_number_is_manual) && simInfo?.present && (
                            <IconButton size="small" sx={{ p: 0.25 }} onClick={() => { setPhoneInput(simInfo?.phone_numbers?.[0] || ''); setEditingPhone(true); }}>
                              <Edit sx={{ fontSize: '0.9rem' }} />
                            </IconButton>
                          )
                        }
                      />
                    )}
                  </Grid>
                  <Grid size={6}>
                    {editingSmsc ? (
                      <Box>
                        <Typography variant="caption" color="text.secondary" sx={{ fontWeight: 500 }}>
                          短信中心号码
                        </Typography>
                        <Box display="flex" alignItems="center" gap={0.5} mt={0.25}>
                          <TextField
                            size="small"
                            variant="standard"
                            placeholder="+86..."
                            value={smscInput}
                            onChange={(e) => setSmscInput(e.target.value)}
                            disabled={savingSmsc}
                            inputProps={{ style: { fontSize: '0.825rem' } }}
                          />
                          <IconButton size="small" color="success" onClick={() => void handleSaveSmsc()} disabled={savingSmsc}>
                            {savingSmsc ? <CircularProgress size={14} /> : <Check fontSize="small" />}
                          </IconButton>
                          <IconButton size="small" color="error" onClick={() => setEditingSmsc(false)} disabled={savingSmsc}>
                            <Close fontSize="small" />
                          </IconButton>
                        </Box>
                      </Box>
                    ) : (
                      <InfoField
                        label="短信中心号码"
                        sensitive
                        showSensitive={showSensitive}
                        value={simInfo?.sms_center || '未读取到'}
                        extra={
                          showSensitive && (isSmscEmpty || simInfo?.sms_center_is_manual) && simInfo?.present && (
                            <IconButton size="small" sx={{ p: 0.25 }} onClick={() => { setSmscInput(simInfo?.sms_center || ''); setEditingSmsc(true); }}>
                              <Edit sx={{ fontSize: '0.9rem' }} />
                            </IconButton>
                          )
                        }
                      />
                    )}
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="ICCID"
                      sensitive
                      showSensitive={showSensitive}
                      value={simInfo?.iccid || 'N/A'}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="IMSI"
                      sensitive
                      showSensitive={showSensitive}
                      value={simInfo?.imsi || 'N/A'}
                    />
                  </Grid>
                </Grid>
              </CardContent>
            </Card>

            {/* Device paths and storage remain here after the line summary is removed. */}
            <Card sx={{ flex: 1, minHeight: 248, display: 'flex', flexDirection: 'column' }}>
              <CardHeader
                avatar={<StorageIcon color="primary" />}
                title="设备、路径与存储"
                titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
              />
              <CardContent sx={{ pt: 0, flex: 1 }}>
                <Grid container spacing={2}>
                  <Grid size={12}>
                    <Box>
                      <Typography variant="caption" color="text.secondary" sx={{ fontWeight: 500 }}>
                        SIM 卡短信容量
                      </Typography>
                      <Box display="flex" alignItems="center" mt={0.5} width="100%">
                        <SmsCapacityProgress used={simInfo?.sms_used} total={simInfo?.sms_total} />
                      </Box>
                    </Box>
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="IMEI"
                      sensitive
                      showSensitive={showSensitive}
                      value={deviceInfo?.imei || 'N/A'}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="QMI / UIM"
                      value={`${line.modem.qmi_device || '未发现'} · Slot ${line.modem.uim_slot}`}
                    />
                  </Grid>
                  <Grid size={6}><InfoField label="硬件家族" value={line.modem.device_family || 'generic_modem'} /></Grid>
                  <Grid size={6}><InfoField label="控制通道" value={line.modem.control_transport || 'modemmanager'} /></Grid>
                  <Grid size={6}><InfoField label="主控制端口" value={line.modem.primary_port || '未发现'} /></Grid>
                  <Grid size={6}><InfoField label="SIM 路径" value={simInfo?.sim_path || line.modem.sim_path || 'N/A'} /></Grid>
                  <Grid size={12}><InfoField label="ModemManager 路径" value={simInfo?.modem_path || line.modem.modem_path || 'N/A'} /></Grid>
                </Grid>
              </CardContent>
            </Card>
          </Box>
        </Grid>

        <Grid size={{ xs: 12, md: 7 }} sx={{ display: 'flex' }}>
          <Box display="flex" flexDirection="column" gap={2} sx={{ flexGrow: 1, minWidth: 0 }}>
            {controls}

            <Card sx={{ flex: 1, minHeight: 248, display: 'flex', flexDirection: 'column' }}>
              <CardHeader
                avatar={<LockIcon color="primary" />}
                title="安全与锁卡状态"
                titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
              />
              <CardContent sx={{ pt: 0, flex: 1 }}>
                <Grid container spacing={2} sx={{ width: '100%' }}>
                  <Grid size={6}>
                    <InfoField
                      label="锁卡状态"
                      value={
                        <Box display="flex" alignItems="center" gap={1}>
                          <Typography variant="body2" component="span" sx={{ fontSize: '0.825rem' }}>
                            {formatLockStatus(simInfo?.lock_status)}
                          </Typography>
                          {simInfo?.lock_status && simInfo.lock_status !== 'none' && simInfo.lock_status !== 'unknown' && (
                            <Chip label="有锁" color="warning" size="small" sx={{ height: 18, fontSize: '0.65rem' }} />
                          )}
                        </Box>
                      }
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="解锁剩余重试次数"
                      value={formatUnlockRetries(
                        simInfo?.pin1_retries,
                        simInfo?.puk1_retries,
                        simInfo?.pin2_retries,
                        simInfo?.puk2_retries
                      )}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="SIM 可用状态"
                      value={simInfo?.present ? (simInfo.active ? '已插入并启用' : '已插入但未启用') : '未检测到 SIM'}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="卡片类型"
                      value={formatSimType(simInfo?.sim_type, simInfo?.esim_status)}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="eUICC 状态"
                      value={simInfo?.esim_status && simInfo.esim_status !== 'unknown' ? simInfo.esim_status : '未检测到 eUICC'}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="身份读取状态"
                      value={simInfo?.iccid && simInfo?.imsi ? 'ICCID / IMSI 已读取' : '身份信息不完整'}
                    />
                  </Grid>
                </Grid>
              </CardContent>
            </Card>
          </Box>
        </Grid>
      </Grid>

      {/* The carrier and serving-network view owns the full lower section. */}
      <Card sx={{ mt: 3 }}>
        <CardHeader
          avatar={<LanguageIcon color="primary" />}
          title="运营商与网络信息"
          titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
        />
        <CardContent sx={{ pt: 0 }}>
          <Grid container spacing={2}>
            <Grid size={{ xs: 12, sm: 6, lg: 3 }}>
              <InfoField
                label="SIM 归属运营商"
                value={formatOperator(simInfo?.operator_name, simInfo?.mcc ? `${simInfo.mcc}${simInfo.mnc}` : '')}
              />
            </Grid>
            <Grid size={{ xs: 12, sm: 6, lg: 3 }}>
              <InfoField
                label="当前注册网络"
                value={formatOperator(simInfo?.registered_operator_name, simInfo?.registered_operator_code)}
              />
            </Grid>
            <Grid size={{ xs: 12, sm: 6, lg: 3 }}>
              <InfoField
                label="运营商配置文件"
                value={simInfo?.carrier_config || 'Default'}
              />
            </Grid>
            <Grid size={{ xs: 12, sm: 6, lg: 3 }}>
              <InfoField
                label="配置文件版本"
                value={simInfo?.carrier_config_revision || 'N/A'}
              />
            </Grid>
            <Grid size={12}>
              <Box borderTop={1} borderColor="divider" pt={2}>
                <LineNetworkOverview
                  lineId={lineId}
                  lineLabel={simInfo?.registered_operator_name || line.modem.operator_id || '当前线路'}
                />
              </Box>
            </Grid>
          </Grid>
        </CardContent>
      </Card>

      <Snackbar
        open={snackbar.open}
        autoHideDuration={4000}
        onClose={() => setSnackbar((prev) => ({ ...prev, open: false }))}
        anchorOrigin={{ vertical: 'bottom', horizontal: 'center' }}
      >
        <Alert severity={snackbar.severity} sx={{ width: '100%' }}>
          {snackbar.message}
        </Alert>
      </Snackbar>
    </Box>
  )
}

export default function SimCardPage() {
  const [searchParams, setSearchParams] = useSearchParams()
  const [selectedLine, setSelectedLine] = useState<VolteLineControlResponse | null>(null)

  const requestedTab = searchParams.get('tab')
  const activeTab = requestedTab === 'carrier-profiles' || requestedTab === 'readers' ? requestedTab : 'lines'

  const handleTabChange = (_event: React.SyntheticEvent, newValue: string) => {
    const params = new URLSearchParams(searchParams)
    if (newValue === 'lines') {
      params.delete('tab')
    } else {
      params.set('tab', newValue)
    }
    setSearchParams(params)
  }

  const handleEsimControlChanged = (control: boolean | null) => {
    setSelectedLine((current) => current ? { ...current, profile: { ...current.profile, esim_control: control } } : null)
  }

  return (
    <Box>
      <Box mb={2} display="flex" justifyContent="space-between" alignItems="flex-end" gap={2} flexWrap="wrap">
        <Box><Typography variant="h5" fontWeight={800}>SIM 卡管理</Typography><Typography variant="body2" color="text.secondary">多线路 IMS、SIM 与 eSIM 运行工作台</Typography></Box>
      </Box>

      <Box sx={{ borderBottom: 1, borderColor: 'divider', mb: 2 }}>
        <Tabs value={activeTab} onChange={handleTabChange} variant="scrollable" scrollButtons="auto">
          <Tab label="线路与 SIM" value="lines" />
          <Tab label="USB SIM 读卡器" value="readers" />
          <Tab label="运营商 Profile" value="carrier-profiles" sx={{ textTransform: 'none' }} />
        </Tabs>
      </Box>

      <Box sx={{ mt: 2 }}>
        {activeTab === 'lines' && <ModemLinesPanel workbench onSelectionChange={setSelectedLine} workbenchHeader={selectedLine ? <WorkbenchOverview line={selectedLine} /> : undefined} workbenchEsim={<EsimWorkbenchPanel key={selectedLine?.modem.line_id ?? 'no-line'} line={selectedLine} onControlChanged={handleEsimControlChanged} />} workbenchSms={selectedLine ? <SMSPage embeddedLineId={selectedLine.modem.line_id} /> : undefined} workbenchAutomation={selectedLine ? <AutomationCenter key={selectedLine.modem.line_id} lineId={lineNotificationScope(selectedLine)} fixedTarget={lineAutomationTarget(selectedLine)} embedded /> : undefined} workbenchNotifications={selectedLine ? <NotificationCenterPage key={selectedLine.modem.line_id} lineId={lineNotificationScope(selectedLine)} embedded /> : undefined} basicInfoForLine={(line, controls) => <SimBasicInfo line={line} controls={controls} />} />}
        {activeTab === 'readers' && <SimReaderPanel />}
        {activeTab === 'carrier-profiles' && <CarrierProfilesPanel />}
      </Box>
    </Box>
  )
}
