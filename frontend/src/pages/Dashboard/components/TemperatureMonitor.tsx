import { Box, Card, CardContent, Typography, useTheme, type Theme } from '@mui/material'
import { LocalFireDepartment } from '@mui/icons-material'
import type { SystemStatsResponse } from '@/api/types'

interface TemperatureMonitorProps {
  systemStats: SystemStatsResponse | null
}

function getTempPercent(temp: number) {
  return Math.min(Math.max(temp, 0), 100)
}

function getTempBarColor(percent: number) {
  const steppedPercent = Math.round(percent / 5) * 5
  const hue = 150 - steppedPercent * 2
  return `hsl(${hue}, 82%, 48%)`
}

export function TemperatureMonitor({ systemStats }: TemperatureMonitorProps) {
  const theme = useTheme<Theme>()

  return (
    <Card sx={{ height: '100%' }}>
      <CardContent>
        <Box display="flex" alignItems="center" gap={1} mb={2} pb={1.25} borderBottom={1} borderColor="divider">
          <LocalFireDepartment sx={{ color: '#f97316' }} />
          <Typography variant="subtitle1" fontWeight={700}>温度监控</Typography>
        </Box>

        {systemStats?.temperature && systemStats.temperature.length > 0 ? (
          <Box display="flex" flexDirection="column" gap={1.5}>
            {systemStats.temperature.map((sensor, idx) => {
              const percent = getTempPercent(sensor.temperature)
              const color = getTempBarColor(percent)
              return (
                <Box key={`${sensor.type}-${idx}`} display="flex" alignItems="center" justifyContent="space-between" gap={1.5}>
                  <Typography
                    variant="body2"
                    color="text.secondary"
                    noWrap
                    sx={{ minWidth: 0, flex: '1 1 auto', fontSize: '0.82rem' }}
                  >
                    {sensor.label || sensor.type}
                  </Typography>
                  <Box display="flex" alignItems="center" gap={1.25} sx={{ flex: '0 0 auto' }}>
                    <Box
                      sx={{
                        width: 96,
                        height: 6,
                        borderRadius: 999,
                        bgcolor: theme.palette.mode === 'light' ? 'rgba(255,255,255,0.62)' : 'rgba(30,41,59,0.72)',
                        overflow: 'hidden',
                      }}
                    >
                      <Box
                        sx={{
                          width: `${percent}%`,
                          height: '100%',
                          borderRadius: 999,
                          bgcolor: color,
                          boxShadow: `0 0 10px ${color}55`,
                        }}
                      />
                    </Box>
                    <Typography
                      variant="body2"
                      fontFamily="monospace"
                      fontWeight={700}
                      sx={{ color, width: 48, textAlign: 'right' }}
                    >
                      {sensor.temperature.toFixed(1)}°
                    </Typography>
                  </Box>
                </Box>
              )
            })}
          </Box>
        ) : (
          <Typography variant="body2" color="text.secondary">暂无数据</Typography>
        )}
      </CardContent>
    </Card>
  )
}
