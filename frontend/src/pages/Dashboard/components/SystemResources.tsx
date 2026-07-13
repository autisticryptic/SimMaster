import { Box, Card, CardContent, LinearProgress, SvgIcon, Typography, useTheme, type Theme } from '@mui/material'
import Grid from '@mui/material/Grid'
import { Memory, Speed as CpuIcon, Storage, Thermostat } from '@mui/icons-material'
import { formatBytes, getCpuColor, getMemoryColor } from '../utils'
import type { SystemStatsResponse, ThermalZone } from '@/api/types'

interface SystemResourcesProps {
  systemStats: SystemStatsResponse | null
}

function CpuChipIcon() {
  return (
    <SvgIcon
      viewBox="0 0 24 24"
      sx={{
        fontSize: 22,
        color: 'primary.main',
        fill: 'none',
        '& path, & rect': {
          stroke: 'currentColor',
          strokeWidth: 2,
          strokeLinecap: 'round',
          strokeLinejoin: 'round',
          fill: 'none',
        },
      }}
    >
      <path d="M12 20v2" />
      <path d="M12 2v2" />
      <path d="M17 20v2" />
      <path d="M17 2v2" />
      <path d="M2 12h2" />
      <path d="M2 17h2" />
      <path d="M2 7h2" />
      <path d="M20 12h2" />
      <path d="M20 17h2" />
      <path d="M20 7h2" />
      <path d="M7 20v2" />
      <path d="M7 2v2" />
      <rect x="4" y="4" width="16" height="16" rx="2" />
      <rect x="8" y="8" width="8" height="8" rx="1" />
    </SvgIcon>
  )
}

const TEMPERATURE_DANGER_THRESHOLD = 100

function resourceGradient(start: string, end: string) {
  return {
    '& .MuiLinearProgress-bar': {
      backgroundImage: `linear-gradient(90deg, ${start}, ${end})`,
      borderRadius: 999,
    },
  }
}

function getTempPercent(temp: number) {
  return Math.min(Math.max((temp / TEMPERATURE_DANGER_THRESHOLD) * 100, 0), 100)
}

function getTempBarColor(currentTemp: number) {
  const clampedTemp = Math.max(0, currentTemp)
  const steppedTemp = Math.round(clampedTemp / 5) * 5

  let hue: number

  if (steppedTemp <= 50) {
    const ratio = steppedTemp / 50
    hue = Math.round(193 - (193 - 45) * ratio)
  } else if (steppedTemp <= 100) {
    const ratio = (steppedTemp - 50) / 50
    hue = Math.round(45 - 45 * ratio)
  } else {
    hue = 0
  }

  return `hsl(${hue}, 84%, 60%)`
}

function generateHeatmapGradient() {
  const stops = []

  for (let percent = 0; percent <= 100; percent += 5) {
    let hue: number
    if (percent <= 50) {
      hue = Math.round(193 - (193 - 45) * (percent / 50))
    } else {
      hue = Math.round(45 - 45 * ((percent - 50) / 50))
    }
    stops.push(`hsl(${hue}, 84%, 60%) ${percent}%`)
  }

  return `linear-gradient(to right, ${stops.join(', ')})`
}

function getTempDotColor(sensor: ThermalZone | null) {
  if (!sensor) return 'text.disabled'
  return getTempBarColor(sensor.temperature)
}

function getFriendlyTemperatureLabel(sensor: ThermalZone) {
  if (sensor.label?.trim()) return sensor.label.trim()
  const rawType = sensor.type.trim()
  const source = rawType || sensor.zone || 'unknown'
  const normalized = source.toLowerCase().replace(/_/g, '-')

  if (/(modem|baseband|wwan|qmi|mhi)/.test(normalized)) return '基带'
  if (/(gpu|adreno)/.test(normalized)) return 'GPU'
  if (/(camera|cam|isp)/.test(normalized)) return '摄像头'
  if (/(wifi|wlan)/.test(normalized)) return 'Wi-Fi'
  if (/(battery|batt)/.test(normalized)) return '电池'
  if (/(charger|charge)/.test(normalized)) return '充电'
  if (/(pmic|power)/.test(normalized)) return '电源管理'
  if (/(soc|tsens)/.test(normalized)) return 'SoC'
  if (/(skin|shell|case)/.test(normalized)) return '外壳'
  if (/(ambient|board)/.test(normalized)) return '环境'

  const cpuRange = normalized.match(/cpu[^0-9]*(\d+)(?:[^0-9]+(\d+))?/)
  if (cpuRange) {
    return cpuRange[2] ? `CPU ${cpuRange[1]}-${cpuRange[2]}` : `CPU ${cpuRange[1]}`
  }
  if (normalized.includes('cpu')) return 'CPU'

  const coreRange = normalized.match(/core[^0-9]*(\d+)(?:[^0-9]+(\d+))?/)
  if (coreRange) {
    return coreRange[2] ? `核心 ${coreRange[1]}-${coreRange[2]}` : `核心 ${coreRange[1]}`
  }
  if (normalized.includes('core')) return '核心'

  const cleaned = source
    .replace(/[-_\s]*(thermal|therm|temperature|temp|sensor|zone)[-_\s]*/gi, ' ')
    .trim()

  return cleaned || source
}

function formatTemperatureSource(sensor: ThermalZone | null) {
  if (!sensor) return '-'
  return `${getFriendlyTemperatureLabel(sensor)}: ${sensor.temperature.toFixed(1)}°`
}

export function SystemResources({ systemStats }: SystemResourcesProps) {
  const theme = useTheme<Theme>()
  const rootDisk = systemStats?.disk?.find((disk) => disk.mount_point === '/') ?? systemStats?.disk?.[0]
  const sortedTemperatureSensors = [...(systemStats?.temperature ?? [])]
    .filter((sensor) => Number.isFinite(sensor.temperature))
    .sort((a, b) => b.temperature - a.temperature)
  const hottestSensor = sortedTemperatureSensors[0] ?? null
  const coldestSensor = sortedTemperatureSensors.length > 0 ? sortedTemperatureSensors[sortedTemperatureSensors.length - 1] : null
  const hottestPercent = hottestSensor ? getTempPercent(hottestSensor.temperature) : 0
  const hottestColor = hottestSensor ? getTempBarColor(hottestSensor.temperature) : getTempBarColor(0)
  const temperatureGradientSize = hottestPercent > 0 ? `${10000 / hottestPercent}% 100%` : '100% 100%'

  const progressSx = {
    height: 10,
    borderRadius: 999,
    bgcolor: theme.palette.mode === 'light' ? 'rgba(226,232,240,0.72)' : 'rgba(30,41,59,0.72)',
  }
  const cpuProgressSx = { ...progressSx, ...resourceGradient('#34d399', '#10b981') }
  const memoryProgressSx = { ...progressSx, ...resourceGradient('#60a5fa', '#1296db') }
  const diskProgressSx = { ...progressSx, ...resourceGradient('#a78bfa', '#8b5cf6') }
  const temperatureProgressSx = {
    ...progressSx,
    '& .MuiLinearProgress-bar': {
      width: `${hottestPercent}%`,
      backgroundImage: hottestSensor ? generateHeatmapGradient() : 'none',
      backgroundSize: temperatureGradientSize,
      backgroundPosition: 'left center',
      bgcolor: hottestSensor ? getTempBarColor(0) : theme.palette.action.disabled,
      borderRadius: 999,
      transform: 'none !important',
      boxShadow: hottestSensor ? `0 0 10px ${hottestColor}55` : 'none',
    },
  }

  return (
    <Card sx={{ height: '100%' }}>
      <CardContent>
        <Box display="flex" alignItems="center" justifyContent="space-between" mb={2}>
          <Box display="flex" alignItems="center" gap={1}>
            <CpuChipIcon />
            <Typography variant="subtitle1" fontWeight={700}>系统资源</Typography>
          </Box>
          <Typography variant="caption" color="text.disabled">
            架构: {systemStats?.system_info?.machine || '-'}
          </Typography>
        </Box>

        <Grid container spacing={2}>
          <Grid size={{ xs: 12, sm: 6 }}>
            <Box>
              <Box display="flex" justifyContent="space-between" alignItems="center" mb={0.75}>
                <Box display="flex" alignItems="center" gap={0.75}>
                  <CpuIcon fontSize="small" sx={{ color: 'text.secondary' }} />
                  <Typography variant="caption" fontWeight={700}>
                    CPU ({systemStats?.cpu_load?.core_count || '-'}核)
                  </Typography>
                </Box>
                <Typography variant="caption" fontFamily="monospace" fontWeight={700}>
                  {systemStats?.cpu_load ? `${systemStats.cpu_load.load_percent.toFixed(0)}%` : '-'}
                </Typography>
              </Box>
              <LinearProgress
                variant="determinate"
                value={systemStats?.cpu_load?.load_percent || 0}
                color={getCpuColor(systemStats?.cpu_load?.load_percent || 0)}
                sx={cpuProgressSx}
              />
              <Typography variant="caption" color="text.disabled" sx={{ display: 'block', mt: 0.5, textAlign: 'right' }}>
                负载: {systemStats?.cpu_load?.load_1min.toFixed(2) || '-'} / {systemStats?.cpu_load?.load_5min.toFixed(2) || '-'} / {systemStats?.cpu_load?.load_15min.toFixed(2) || '-'}
              </Typography>
            </Box>
          </Grid>

          <Grid size={{ xs: 12, sm: 6 }}>
            <Box>
              <Box display="flex" justifyContent="space-between" alignItems="center" mb={0.75}>
                <Box display="flex" alignItems="center" gap={0.75}>
                  <Memory fontSize="small" sx={{ color: 'text.secondary' }} />
                  <Typography variant="caption" fontWeight={700}>内存</Typography>
                </Box>
                <Typography variant="caption" fontFamily="monospace" fontWeight={700}>
                  {systemStats?.memory ? `${systemStats.memory.used_percent.toFixed(0)}%` : '-'}
                </Typography>
              </Box>
              <LinearProgress
                variant="determinate"
                value={systemStats?.memory?.used_percent || 0}
                color={getMemoryColor(systemStats?.memory?.used_percent || 0)}
                sx={memoryProgressSx}
              />
              <Typography variant="caption" color="text.disabled" sx={{ display: 'block', mt: 0.5, textAlign: 'right' }}>
                {systemStats?.memory ? `已用 ${formatBytes(systemStats.memory.used_bytes)} / 可用 ${formatBytes(systemStats.memory.available_bytes)}` : '-'}
              </Typography>
            </Box>
          </Grid>

          <Grid size={{ xs: 12, sm: 6 }}>
            <Box>
              <Box display="flex" justifyContent="space-between" alignItems="center" mb={0.75}>
                <Box display="flex" alignItems="center" gap={0.75}>
                  <Storage fontSize="small" sx={{ color: 'text.secondary' }} />
                  <Typography variant="caption" fontWeight={700}>磁盘 (根目录)</Typography>
                </Box>
                <Typography variant="caption" fontFamily="monospace" fontWeight={700}>
                  {rootDisk ? `${rootDisk.used_percent.toFixed(0)}%` : '-'}
                </Typography>
              </Box>
              <LinearProgress
                variant="determinate"
                value={rootDisk?.used_percent || 0}
                color={getMemoryColor(rootDisk?.used_percent || 0)}
                sx={diskProgressSx}
              />
              <Typography variant="caption" color="text.disabled" sx={{ display: 'block', mt: 0.5, textAlign: 'right' }}>
                {rootDisk ? `${formatBytes(rootDisk.used_bytes)} / ${formatBytes(rootDisk.total_bytes)}` : '-'}
              </Typography>
            </Box>
          </Grid>

          <Grid size={{ xs: 12, sm: 6 }}>
            <Box>
              <Box display="flex" justifyContent="space-between" alignItems="center" mb={0.75}>
                <Box display="flex" alignItems="center" gap={0.75}>
                  <Thermostat fontSize="small" sx={{ color: 'text.secondary' }} />
                  <Typography variant="caption" fontWeight={700}>最高温</Typography>
                </Box>
                <Typography variant="caption" fontFamily="monospace" fontWeight={700} sx={{ color: hottestSensor ? hottestColor : 'text.secondary' }}>
                  {hottestSensor ? `${hottestSensor.temperature.toFixed(1)}°C` : '-'}
                </Typography>
              </Box>
              <LinearProgress
                variant="determinate"
                value={hottestPercent}
                sx={temperatureProgressSx}
              />
              <Box display="flex" alignItems="center" justifyContent="space-between" gap={1.5} mt={0.5}>
                <Box display="flex" alignItems="center" gap={0.5} minWidth={0}>
                  <Box
                    sx={{
                      width: 5,
                      height: 5,
                      borderRadius: '50%',
                      bgcolor: getTempDotColor(coldestSensor),
                      flex: '0 0 auto',
                    }}
                  />
                  <Typography variant="caption" fontFamily="monospace" color="text.secondary" noWrap>
                    {formatTemperatureSource(coldestSensor)}
                  </Typography>
                </Box>
                <Box display="flex" alignItems="center" justifyContent="flex-end" gap={0.5} minWidth={0}>
                  <Box
                    sx={{
                      width: 5,
                      height: 5,
                      borderRadius: '50%',
                      bgcolor: getTempDotColor(hottestSensor),
                      flex: '0 0 auto',
                    }}
                  />
                  <Typography variant="caption" fontFamily="monospace" color="text.secondary" noWrap textAlign="right">
                    {formatTemperatureSource(hottestSensor)}
                  </Typography>
                </Box>
              </Box>
            </Box>
          </Grid>
        </Grid>
      </CardContent>
    </Card>
  )
}
