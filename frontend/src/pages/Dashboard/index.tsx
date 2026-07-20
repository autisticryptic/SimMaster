import { Box, CircularProgress, Paper, Stack, Typography } from '@mui/material'
import Grid from '@mui/material/Grid'
import { useRefreshInterval } from '@/contexts/RefreshContext'
import ErrorSnackbar from '@/components/ErrorSnackbar'
import {
  QuickControls,
  SystemResources,
  NetworkSpeed,
  SimCardInfo,
  TemperatureMonitor,
  DeviceInfoCard,
} from './components'
import { useDashboardData, type DashboardData } from './hooks/useDashboardData'

function StatusBar({ data }: { data: DashboardData }) {
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
                bgcolor: data.deviceInfo?.online ? 'success.main' : 'error.main',
                opacity: 0.3,
                animation: data.deviceInfo?.online ? 'pulse 1.8s infinite' : 'none',
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
                bgcolor: data.deviceInfo?.online ? 'success.main' : 'error.main',
              }}
            />
          </Box>
          <Typography variant="subtitle2" fontWeight={800} sx={{ fontSize: 16 }}>
            {data.deviceInfo?.online ? '系统在线' : '系统离线'}
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
  const { initialLoading, error, setError, data, actions } = useDashboardData(refreshInterval, refreshKey)

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
          <Grid size={{ xs: 12, md: 6, lg: 3 }}>
            <QuickControls />
          </Grid>

          <Grid size={{ xs: 12, md: 6, lg: 3 }}>
            <SimCardInfo simInfo={data.simInfo} onRefresh={() => void actions.loadData()} />
          </Grid>

          <Grid size={{ xs: 12, lg: 6 }}>
            <SystemResources systemStats={data.systemStats} />
          </Grid>

          <Grid size={{ xs: 12, lg: 8 }}>
            <NetworkSpeed systemStats={data.systemStats} speedHistory={data.speedHistory} />
          </Grid>

          <Grid size={{ xs: 12, lg: 4 }}>
            <TemperatureMonitor systemStats={data.systemStats} />
          </Grid>

          <Grid size={12}>
            <DeviceInfoCard deviceInfo={data.deviceInfo} systemStats={data.systemStats} />
          </Grid>
        </Grid>
      </Stack>
    </Box>
  )
}
