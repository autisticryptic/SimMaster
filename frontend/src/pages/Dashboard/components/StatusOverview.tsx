import { Box, Chip, Typography, Paper, useTheme, type Theme } from '@mui/material'
import { alpha } from '@/utils/theme'
import {
  SignalCellularAlt,
  WifiTethering,
  Router,
  PowerSettingsNew,
  FlightTakeoff,
  TravelExplore,
} from '@mui/icons-material'
import { formatCarrierName, getCarrierColor, getCarrierLogo } from '@/utils/carriers'
import { getSignalColor } from '../utils'
import type { DeviceInfo, NetworkInfo, CellsResponse, AirplaneModeResponse, RoamingResponse } from '@/api/types'

interface StatusOverviewProps {
  deviceInfo: DeviceInfo | null
  networkInfo: NetworkInfo | null
  cellsInfo: CellsResponse | null
  airplaneMode: AirplaneModeResponse | null
  roaming?: RoamingResponse | null
}

export function StatusOverview({
  deviceInfo,
  networkInfo,
  cellsInfo,
  airplaneMode,
  roaming,
}: StatusOverviewProps) {
  const theme = useTheme<Theme>()

  // 获取网络制式显示
  const getNetworkTech = () => {
    if (cellsInfo?.serving_cell?.tech) {
      return cellsInfo.serving_cell.tech.toUpperCase()
    }
    if (networkInfo?.technology_preference) {
      const tech = networkInfo.technology_preference.toLowerCase()
      if (tech.includes('nr')) return '5G'
      if (tech.includes('lte')) return 'LTE'
    }
    return 'N/A'
  }

  return (
    <Paper
      elevation={0}
      sx={{
        p: 2,
        mb: 2,
        borderRadius: 2,
        background: (() => {
          const primaryMain = (theme.palette.primary as { main: string }).main
          const secondaryMain = (theme.palette.secondary as { main: string }).main
          return `linear-gradient(135deg, ${alpha(primaryMain, 0.08)} 0%, ${alpha(secondaryMain, 0.03)} 100%)`
        })(),
        border: (() => {
          const primaryMain = (theme.palette.primary as { main: string }).main
          return `1px solid ${alpha(primaryMain, 0.1)}`
        })(),
      }}
    >
      <Box display="flex" flexWrap="wrap" alignItems="center" gap={2}>
        {/* 运营商 Logo + 信号 */}
        <Box display="flex" alignItems="center" gap={1.5}>
          {(() => {
            const logo = getCarrierLogo(networkInfo?.mcc, networkInfo?.mnc)
            return logo ? (
              <Box
                component="img"
                src={logo}
                alt={formatCarrierName(networkInfo?.mcc, networkInfo?.mnc)}
                sx={{ height: 32, width: 'auto', objectFit: 'contain' }}
              />
            ) : (
              <Chip
                label={formatCarrierName(networkInfo?.mcc, networkInfo?.mnc)}
                color={getCarrierColor(networkInfo?.mcc, networkInfo?.mnc)}
                size="small"
              />
            )
          })()}
          <Box display="flex" alignItems="center" gap={0.5}>
            <SignalCellularAlt sx={{ fontSize: 24, color: `${getSignalColor(networkInfo?.signal_strength || 0)}.main` }} />
            <Typography variant="h6" fontWeight="bold" color={`${getSignalColor(networkInfo?.signal_strength || 0)}.main`}>
              {networkInfo?.signal_strength || 0}%
            </Typography>
          </Box>
        </Box>

        {/* 网络制式 */}
        <Chip
          icon={<WifiTethering />}
          label={getNetworkTech()}
          color={getNetworkTech() === '5G' || getNetworkTech() === 'NR' ? 'success' : 'primary'}
          size="small"
          sx={{ fontWeight: 'bold' }}
        />

        {/* 注册状态 */}
        <Chip
          icon={<Router />}
          label={
            networkInfo?.registration_status === 'registered' ? '已注册' : 
            networkInfo?.registration_status === 'roaming' ? '漫游' :
            networkInfo?.registration_status || '未知'
          }
          color={
            networkInfo?.registration_status === 'registered' ? 'success' : 
            networkInfo?.registration_status === 'roaming' ? 'warning' :
            'default'
          }
          variant="outlined"
          size="small"
        />

        {/* 漫游状态 */}
        {roaming?.is_roaming && (
          <Chip
            icon={<TravelExplore />}
            label={roaming.roaming_allowed ? '漫游数据已开启' : '漫游数据已关闭'}
            color={roaming.roaming_allowed ? 'info' : 'error'}
            size="small"
          />
        )}

        {/* Modem 状态 */}
        <Chip
          icon={<PowerSettingsNew />}
          label={deviceInfo?.online ? '在线' : '离线'}
          color={deviceInfo?.online ? 'success' : 'error'}
          size="small"
        />

        {/* VoLTE */}

        {/* 飞行模式 */}
        {airplaneMode?.enabled && (
          <Chip icon={<FlightTakeoff />} label="飞行模式" color="warning" size="small" />
        )}
      </Box>
    </Paper>
  )
}
