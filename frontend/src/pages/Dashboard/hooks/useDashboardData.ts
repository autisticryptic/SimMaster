import { useState, useCallback, useEffect, useRef } from 'react'
import { api } from '@/api/current'
import type {
  DeviceInfo,
  NetworkInfo,
  CellsResponse,
  QosInfo,
  SimInfo,
  SystemStatsResponse,
  LineNetworkControlsResponse,
  ConnectionAddressesResponse,
  VowifiConfig,
  VowifiStatusResponse,
} from '@/api/types'
import { isTransientModemError, createThrottledWarner } from '@/utils/modemErrors'

export const SPEED_HISTORY_MAX_POINTS = 30

/** ModemManager 通常不暴露 QCI；在数据连接开启时从 WWAN 网卡字节速率估算上下行（kbps，与旧 QosInfo 字段一致）。 */
function qosFromWwanInterface(stats: SystemStatsResponse, dataActive: boolean): QosInfo | null {
  if (!dataActive || !stats.network_speed?.interfaces?.length) return null
  const wwan = stats.network_speed.interfaces.find(
    (i) =>
      i.interface.startsWith('wwan') ||
      i.interface.startsWith('wwp') ||
      i.interface.toLowerCase().includes('mbim'),
  )
  if (!wwan) return null
  return {
    qci: 0,
    dl_speed: (wwan.rx_bytes_per_sec * 8) / 1000,
    ul_speed: (wwan.tx_bytes_per_sec * 8) / 1000,
    source: 'interface',
  }
}

export interface InterfaceSpeedHistory {
  rx: number[]
  tx: number[]
  totalRx: number
  totalTx: number
}

export interface ConnectivityResult {
  ipv4: { success: boolean; latency_ms?: number }
  ipv6: { success: boolean; latency_ms?: number }
}

export type ConnectionAddresses = ConnectionAddressesResponse

export interface DashboardData {
  deviceInfo: DeviceInfo | null
  simInfo: SimInfo | null
  systemStats: SystemStatsResponse | null
  networkInfo: NetworkInfo | null
  dataStatus: boolean
  cellsInfo: CellsResponse | null
  qosInfo: QosInfo | null
  /// Per-line network state; airplane mode and roaming live here now.
  lineNetworkControls: LineNetworkControlsResponse[]
  connectivity: ConnectivityResult | null
  connectionAddresses: ConnectionAddresses
  speedHistory: Record<string, InterfaceSpeedHistory>
  vowifiControl: VowifiConfig | null
  vowifiStatus: VowifiStatusResponse | null
}

export interface DashboardActions {
  loadData: () => Promise<void>
}

const throttledWarn = createThrottledWarner(10_000)

export function useDashboardData(refreshInterval: number, refreshKey: number) {
  const [initialLoading, setInitialLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [deviceInfo, setDeviceInfo] = useState<DeviceInfo | null>(null)
  const [simInfo, setSimInfo] = useState<SimInfo | null>(null)
  const [systemStats, setSystemStats] = useState<SystemStatsResponse | null>(null)
  const [networkInfo, setNetworkInfo] = useState<NetworkInfo | null>(null)
  const [dataStatus, setDataStatus] = useState(false)
  const [cellsInfo, setCellsInfo] = useState<CellsResponse | null>(null)
  const [qosInfo, setQosInfo] = useState<QosInfo | null>(null)
  const [lineNetworkControls, setLineNetworkControls] = useState<LineNetworkControlsResponse[]>([])
  const [connectivity, setConnectivity] = useState<ConnectivityResult | null>(null)
  const [connectionAddresses, setConnectionAddresses] = useState<ConnectionAddresses>({ ipv4: [], ipv6: [] })
  const [vowifiControl, setVowifiControl] = useState<VowifiConfig | null>(null)
  const [vowifiStatus, setVowifiStatus] = useState<VowifiStatusResponse | null>(null)
  const [speedHistory, setSpeedHistory] = useState<Record<string, InterfaceSpeedHistory>>({})
  const speedHistoryRef = useRef<Record<string, InterfaceSpeedHistory>>({})

  const updateSpeedHistory = useCallback((stats: SystemStatsResponse | null) => {
    if (!stats?.network_speed?.interfaces) return

    const nextHistory = { ...speedHistoryRef.current }

    for (const iface of stats.network_speed.interfaces) {
      const existing = nextHistory[iface.interface] || { rx: [], tx: [], totalRx: 0, totalTx: 0 }
      const rx = [...existing.rx, iface.rx_bytes_per_sec]
      const tx = [...existing.tx, iface.tx_bytes_per_sec]

      if (rx.length > SPEED_HISTORY_MAX_POINTS) {
        rx.shift()
        tx.shift()
      }

      nextHistory[iface.interface] = {
        rx,
        tx,
        totalRx: iface.total_rx_bytes,
        totalTx: iface.total_tx_bytes,
      }
    }

    speedHistoryRef.current = nextHistory
    setSpeedHistory(nextHistory)
  }, [])

  const loadData = useCallback(async (background = false) => {
    if (!background) setError(null)
    const failures: string[] = []

    const requestOrNull = async <T,>(promise: Promise<T>, label: string): Promise<T | null> => {
      try {
        return await promise
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        failures.push(`${label}: ${message}`)
        return null
      }
    }

    try {
      // 快速请求：决定 initialLoading，通常 <200ms 即可全部返回
      const fastPromise = Promise.all([
        requestOrNull(api.getDeviceInfo(), 'device'),
        requestOrNull(api.getSimInfo(), 'sim'),
        requestOrNull(api.getNetworkInfo(), 'network'),
        requestOrNull(api.getDataStatus(), 'data'),
        // Airplane mode is per line now, so it comes from the line controls
        // rather than a device-wide endpoint.
        requestOrNull(api.getLineNetworkControls(), 'line-controls'),
        requestOrNull(api.getNetworkConnectionAddresses(), 'connection-addresses'),
        requestOrNull(api.getVowifiControl(), 'vowifi-control'),
        requestOrNull(api.getCellsInfo(), 'cells'),
        requestOrNull(api.getVowifiStatus(), 'vowifi-status'),
      ])

      // 慢速请求：不阻塞页面渲染，异步填充数据
      const statsPromise = requestOrNull(api.getSystemStats(), 'stats')
      const connectivityPromise = requestOrNull(api.getConnectivity(), 'connectivity')

      // 等待快速请求完成即可渲染页面
      const [
        deviceRes,
        simRes,
        networkRes,
        dataRes,
        lineControlsRes,
        addressesRes,
        vowifiControlRes,
        cellsRes,
        vowifiStatusRes,
      ] = await fastPromise

      if (deviceRes?.data) setDeviceInfo(deviceRes.data)
      if (simRes?.data) setSimInfo(simRes.data)
      if (networkRes?.data) setNetworkInfo(networkRes.data)
      if (dataRes?.data) setDataStatus(dataRes.data.active)
      if (lineControlsRes?.data) setLineNetworkControls(lineControlsRes.data)
      if (addressesRes?.data) setConnectionAddresses(addressesRes.data)
      if (vowifiControlRes?.data) setVowifiControl(vowifiControlRes.data)
      if (cellsRes?.data) setCellsInfo(cellsRes.data)
      if (vowifiStatusRes?.data) setVowifiStatus(vowifiStatusRes.data)

      // 快速数据就绪，立即解除 loading
      setInitialLoading(false)

      // 异步等待慢速请求并填充数据
      const [statsRes, connectivityRes] = await Promise.all([statsPromise, connectivityPromise])

      if (statsRes?.data) {
        setSystemStats(statsRes.data)
        updateSpeedHistory(statsRes.data)
      }
      if (connectivityRes?.data) setConnectivity(connectivityRes.data)

      const dataActive = dataRes?.data?.active ?? false
      if (statsRes?.data) {
        setQosInfo(qosFromWwanInterface(statsRes.data, dataActive))
      } else {
        setQosInfo(null)
      }

      // 错误处理：区分后台轮询与首次/手动加载
      if (failures.length > 0) {
        if (background) {
          // 后台轮询：过滤 Modem 暂态错误，非暂态错误仍向用户展示
          const nonTransient = failures.filter((f) => !isTransientModemError(f))
          if (nonTransient.length > 0) {
            setError(nonTransient[0])
          } else {
            throttledWarn('Dashboard', failures.join('; '))
          }
        } else {
          // 首次加载 / 手动刷新：所有错误都应反馈给用户
          setError(failures[0])
        }
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setInitialLoading(false)
    }
  }, [updateSpeedHistory])

  useEffect(() => {
    // 首次加载：background = false，错误会展示给用户
    const timeout = window.setTimeout(() => {
      void loadData(false)
    }, 0)

    let interval: number | undefined
    if (refreshInterval > 0) {
      // 后台轮询：background = true，仅非暂态错误展示
      interval = window.setInterval(() => void loadData(true), refreshInterval)
    }

    return () => {
      window.clearTimeout(timeout)
      if (interval !== undefined) {
        window.clearInterval(interval)
      }
    }
  }, [refreshInterval, refreshKey, loadData])

  return {
    initialLoading,
    error,
    setError,
    data: {
      deviceInfo,
      simInfo,
      systemStats,
      networkInfo,
      dataStatus,
      cellsInfo,
      qosInfo,
      lineNetworkControls,
      connectivity,
      connectionAddresses,
      speedHistory,
      vowifiControl,
      vowifiStatus,
    } as DashboardData,
    actions: {
      loadData,
    } as DashboardActions,
  }
}
