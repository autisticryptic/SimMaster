import { useState, useEffect } from 'react'
import {
  Box,
  Typography,
  Tabs,
  Tab,
  Card,
  CardContent,
  CardHeader,
  Chip,
  CircularProgress,
  IconButton,
  Tooltip,
  TextField,
  Snackbar,
  Alert,
  LinearProgress,
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
} from '@mui/icons-material'
import { useSearchParams } from 'react-router-dom'
import { api } from '../api/current'
import type { SimInfo } from '../api/types'
import ErrorSnackbar from '../components/ErrorSnackbar'
import EsimManagerPage from './EsimManager'
import VowifiDiagnosticsPage from './VowifiDiagnostics'
import { useWorkMode } from '../contexts/WorkModeContext'

function getSensitiveStyle(show: boolean) {
  return {
    filter: show ? 'none' : 'blur(5px)',
    transition: 'filter 0.3s ease',
    userSelect: show ? 'auto' : 'none',
    cursor: show ? 'text' : 'default',
  } as const
}

function formatSimType(simType?: string, esimStatus?: string, workMode?: string) {
  // 1. 优先取 simType (如果明确是 physical 或 esim)
  if (simType === 'physical') return '物理 SIM 卡';
  if (simType === 'esim') return 'eSIM 卡';

  // 2. 其次根据有没有 euicc 芯片判断 (esimStatus 有明确 of eUICC 状态)
  if (esimStatus && esimStatus !== 'unknown') {
    return 'eSIM 卡';
  }

  // 3. 最后根据工作模式兜底
  if (workMode === 'sim') {
    return '物理 SIM 卡';
  } else if (workMode === 'esim') {
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
    <Box display="flex" flexWrap="wrap" gap={0.5} alignItems="center">
      {renderItem('PIN', pin1, isPin1Low)}
      <Typography variant="body2" sx={{ fontSize: '0.825rem', color: 'text.secondary', mx: 0.5 }}>|</Typography>
      {renderItem('PUK', puk1, isPuk1Low)}
      <Typography variant="body2" sx={{ fontSize: '0.825rem', color: 'text.secondary', mx: 0.5 }}>|</Typography>
      {renderItem('PIN2', pin2, isPin2Low)}
      <Typography variant="body2" sx={{ fontSize: '0.825rem', color: 'text.secondary', mx: 0.5 }}>|</Typography>
      {renderItem('PUK2', puk2, isPuk2Low)}
    </Box>
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

function SimBasicInfo() {
  const { mode } = useWorkMode()
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [showSensitive, setShowSensitive] = useState(false)
  const [simInfo, setSimInfo] = useState<SimInfo | null>(null)

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

  const loadData = async () => {
    setLoading(true)
    setError(null)
    try {
      const simRes = await api.getSimInfo()
      if (simRes.data) setSimInfo(simRes.data)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }

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
      await api.updateSimCache({ phone_number: phoneInput.trim() })
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
      await api.updateSimCache({ sms_center: smscInput.trim() })
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
  }, [])

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
      <Grid container spacing={3}>
        {/* Left Column */}
        <Grid size={{ xs: 12, md: 6 }} sx={{ display: 'flex' }}>
          <Box display="flex" flexDirection="column" gap={3} sx={{ flexGrow: 1 }}>
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
                      value={formatSimType(simInfo?.sim_type, simInfo?.esim_status, mode)}
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

            {/* Card 3: 安全与锁卡状态 */}
            <Card sx={{ flexGrow: 1, display: 'flex', flexDirection: 'column' }}>
              <CardHeader
                avatar={<LockIcon color="primary" />}
                title="安全与锁卡状态"
                titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
              />
              <CardContent sx={{ pt: 0, flexGrow: 1 }}>
                <Grid container spacing={2}>
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
                </Grid>
              </CardContent>
            </Card>
          </Box>
        </Grid>

        {/* Right Column */}
        <Grid size={{ xs: 12, md: 6 }} sx={{ display: 'flex' }}>
          <Box display="flex" flexDirection="column" gap={3} sx={{ flexGrow: 1 }}>
            {/* Card 2: 运营商与网络信息 */}
            <Card>
              <CardHeader
                avatar={<LanguageIcon color="primary" />}
                title="运营商与网络信息"
                titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
              />
              <CardContent sx={{ pt: 0 }}>
                <Grid container spacing={2}>
                  <Grid size={6}>
                    <InfoField
                      label="SIM 归属运营商"
                      value={formatOperator(simInfo?.operator_name, simInfo?.mcc ? `${simInfo.mcc}${simInfo.mnc}` : '')}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="当前注册网络"
                      value={formatOperator(simInfo?.registered_operator_name, simInfo?.registered_operator_code)}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="运营商配置文件"
                      value={simInfo?.carrier_config || 'Default'}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="配置文件版本"
                      value={simInfo?.carrier_config_revision || 'N/A'}
                    />
                  </Grid>
                </Grid>
              </CardContent>
            </Card>

            {/* Card 4: 短信存储与系统信息 */}
            <Card sx={{ flexGrow: 1, display: 'flex', flexDirection: 'column' }}>
              <CardHeader
                avatar={<StorageIcon color="primary" />}
                title="短信存储与系统信息"
                titleTypographyProps={{ variant: 'subtitle1', fontWeight: 600 }}
              />
              <CardContent sx={{ pt: 0, flexGrow: 1 }}>
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
                      label="SIM 路径"
                      value={simInfo?.sim_path || 'N/A'}
                    />
                  </Grid>
                  <Grid size={6}>
                    <InfoField
                      label="Modem 路径"
                      value={simInfo?.modem_path || 'N/A'}
                    />
                  </Grid>
                </Grid>
              </CardContent>
            </Card>
          </Box>
        </Grid>
      </Grid>

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
  const { mode, loading } = useWorkMode()
  const [searchParams, setSearchParams] = useSearchParams()
  const [vowifiEnabled, setVowifiEnabled] = useState(false)
  const [configLoading, setConfigLoading] = useState(true)

  useEffect(() => {
    let active = true
    api.getVowifiControl()
      .then((res) => {
        if (active && res.data) {
          setVowifiEnabled(res.data.feature_enabled)
        }
      })
      .catch((err) => {
        console.error('Failed to load VoWiFi control config:', err)
      })
      .finally(() => {
        if (active) {
          setConfigLoading(false)
        }
      })
    return () => {
      active = false
    }
  }, [])

  let activeTab = searchParams.get('tab') || 'basic'

  if (mode !== 'esim' && activeTab === 'esim') {
    activeTab = 'basic'
  }

  if (!configLoading && !vowifiEnabled && activeTab === 'vowifi') {
    activeTab = 'basic'
  }

  const handleTabChange = (_event: React.SyntheticEvent, newValue: string) => {
    const params = new URLSearchParams(searchParams)
    if (newValue === 'basic') {
      params.delete('tab')
    } else {
      params.set('tab', newValue)
    }
    setSearchParams(params)
  }

  if (loading || configLoading) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" minHeight="50vh">
        <CircularProgress size={32} />
      </Box>
    )
  }

  return (
    <Box>
      <Box mb={2}>
        <Typography variant="h5" fontWeight={700}>
          SIM 卡管理
        </Typography>
      </Box>

      <Box sx={{ borderBottom: 1, borderColor: 'divider', mb: 2 }}>
        <Tabs value={activeTab} onChange={handleTabChange} variant="scrollable" scrollButtons="auto">
          <Tab label="基本信息" value="basic" />
          {mode === 'esim' && <Tab label="eSIM 管理" value="esim" sx={{ textTransform: 'none' }} />}
          {vowifiEnabled && <Tab label="WiFi Calling" value="vowifi" sx={{ textTransform: 'none' }} />}
        </Tabs>
      </Box>

      <Box sx={{ mt: 2 }}>
        {activeTab === 'basic' && <SimBasicInfo />}
        {activeTab === 'esim' && mode === 'esim' && <EsimManagerPage />}
        {activeTab === 'vowifi' && vowifiEnabled && <VowifiDiagnosticsPage />}
      </Box>
    </Box>
  )
}
