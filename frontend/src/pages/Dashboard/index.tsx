import {
  Box,
  Chip,
  CircularProgress,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Typography,
} from '@mui/material'
import Grid from '@mui/material/Grid'
import { useRefreshInterval } from '@/contexts/RefreshContext'
import ErrorSnackbar from '@/components/ErrorSnackbar'
import {
  SystemResources,
  NetworkSpeed,
  TemperatureMonitor,
} from './components'
import { maskedIccid, modemSlotLabel, shortLineId } from '@/components/modemLineFormat'
import { useDashboardData, type DashboardData, type DashboardLineInfo } from './hooks/useDashboardData'

function imsStatusLabel(line: DashboardLineInfo) {
  if (!line.modem.present || line.modem.line_kind === 'reader') return { label: '不适用', color: 'default' as const }
  if (line.volte.registered) return { label: '已注册', color: 'success' as const }
  if (line.volte.last_error) return { label: '异常', color: 'error' as const }
  if (line.volte.phase && !['idle', 'stopped', 'disabled'].includes(line.volte.phase.toLowerCase())) {
    return { label: '连接中', color: 'warning' as const }
  }
  return { label: '未注册', color: 'default' as const }
}

function trunkStatusLabel(line: DashboardLineInfo) {
  if (!line.trunk.enabled) return { label: '未启用', color: 'default' as const }
  if (line.trunk.registered || line.trunk.phase === 'ready') return { label: '就绪', color: 'success' as const }
  if (line.trunk.last_error || line.trunk.phase === 'degraded') return { label: '异常', color: 'error' as const }
  return { label: '连接中', color: 'warning' as const }
}

function LineStatusTable({ lines }: { lines: DashboardLineInfo[] }) {
  return (
    <TableContainer component={Paper} elevation={0}>
      <Box px={2} pt={1.5}>
        <Typography variant="subtitle1" fontWeight={700}>基带与读卡器线路</Typography>
      </Box>
      <Table size="small">
        <TableHead>
          <TableRow>
            <TableCell>线路</TableCell>
            <TableCell>设备</TableCell>
            <TableCell>SIM</TableCell>
            <TableCell>运营商</TableCell>
            <TableCell>信号</TableCell>
            <TableCell>IMS</TableCell>
            <TableCell>Trunk</TableCell>
            <TableCell align="right">设备</TableCell>
          </TableRow>
        </TableHead>
        <TableBody>
          {lines.map((line) => {
            const { modem, deviceInfo, simInfo, networkInfo } = line
            const isReader = modem.line_kind === 'reader'
            const online = modem.present && (isReader || Boolean(deviceInfo?.online))
            const ims = imsStatusLabel(line)
            const trunk = trunkStatusLabel(line)
            return (
              <TableRow key={modem.line_id} hover>
                <TableCell>
                  <Typography variant="body2" fontWeight={600}>{isReader ? '读卡器' : modemSlotLabel(modem)}</Typography>
                  <Typography variant="caption" color="text.secondary">{shortLineId(modem.line_id)}</Typography>
                </TableCell>
                <TableCell>{isReader ? modem.model || '独立读卡器' : `${deviceInfo?.manufacturer || modem.manufacturer || ''} ${deviceInfo?.model || modem.model || ''}`.trim() || '-'}</TableCell>
                <TableCell sx={{ fontFamily: 'monospace' }}>{maskedIccid(simInfo?.iccid || modem.sim_iccid)}</TableCell>
                <TableCell>{networkInfo?.operator_name || (isReader ? 'VoWiFi' : '-')}</TableCell>
                <TableCell>{networkInfo ? `${networkInfo.signal_strength}%` : '-'}</TableCell>
                <TableCell><Chip size="small" label={ims.label} color={ims.color} variant="outlined" /></TableCell>
                <TableCell><Chip size="small" label={trunk.label} color={trunk.color} variant="outlined" /></TableCell>
                <TableCell align="right"><Chip size="small" label={online ? '在线' : '离线'} color={online ? 'success' : 'default'} variant="outlined" /></TableCell>
              </TableRow>
            )
          })}
          {lines.length === 0 && <TableRow><TableCell colSpan={8} align="center">未发现线路</TableCell></TableRow>}
        </TableBody>
      </Table>
    </TableContainer>
  )
}

function StatusBar({ data }: { data: DashboardData }) {
  const onlineLines = data.lines.filter(({ modem, deviceInfo }) => (
    modem.present && (modem.line_kind === 'reader' || Boolean(deviceInfo?.online))
  )).length
  const hasOnlineLine = onlineLines > 0
  return (
    <Paper
      elevation={0}
      sx={{
        p: 2,
        display: 'flex',
        flexWrap: 'wrap',
        alignItems: 'center',
        justifyContent: 'flex-start',
        gap: 2,
      }}
    >
      <Stack direction="row" spacing={{ xs: 1, md: 2 }} alignItems="center" flexWrap="wrap" useFlexGap>
        <Box display="flex" alignItems="center" gap={1}>
          <Box sx={{ position: 'relative', width: 12, height: 12 }}>
            <Box
              sx={{
                position: 'absolute',
                inset: 0,
                borderRadius: '50%',
                bgcolor: hasOnlineLine ? 'success.main' : 'error.main',
                opacity: 0.3,
                animation: hasOnlineLine ? 'pulse 1.8s infinite' : 'none',
                '@keyframes pulse': {
                  '0%': { transform: 'scale(1)', opacity: 0.45 },
                  '70%': { transform: 'scale(2.1)', opacity: 0 },
                  '100%': { transform: 'scale(2.1)', opacity: 0 },
                },
              }}
            />
            <Box
              sx={{
                position: 'absolute',
                inset: 2,
                borderRadius: '50%',
                bgcolor: hasOnlineLine ? 'success.main' : 'error.main',
              }}
            />
          </Box>
          <Typography variant="subtitle2" fontWeight={800} sx={{ fontSize: 16 }}>
            {data.lines.length > 0 ? `${onlineLines}/${data.lines.length} 条线路在线` : '未发现线路'}
          </Typography>
        </Box>

        <Typography variant="caption" color="text.disabled">
          | 运行 {data.systemStats?.uptime?.uptime_formatted || '-'}
        </Typography>
      </Stack>
    </Paper>
  )
}

export default function DashboardPage() {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const { initialLoading, error, setError, data } = useDashboardData(refreshInterval, refreshKey)

  if (initialLoading) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" minHeight="60vh">
        <CircularProgress />
      </Box>
    )
  }

  return (
    <Box sx={{ maxWidth: 1600, mx: 'auto' }}>
      <ErrorSnackbar error={error} onClose={() => setError(null)} />

      <Stack spacing={2}>
        <StatusBar data={data} />

        <Grid container spacing={2}>
          <Grid size={12}>
            <SystemResources systemStats={data.systemStats} />
          </Grid>

          <Grid size={12}>
            <LineStatusTable lines={data.lines} />
          </Grid>

          <Grid size={{ xs: 12, lg: 8 }}>
            <NetworkSpeed systemStats={data.systemStats} speedHistory={data.speedHistory} />
          </Grid>

          <Grid size={{ xs: 12, lg: 4 }}>
            <TemperatureMonitor systemStats={data.systemStats} />
          </Grid>
        </Grid>
      </Stack>
    </Box>
  )
}
