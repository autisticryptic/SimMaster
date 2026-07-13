import { createContext, useContext } from 'react'

// 刷新间隔 Context
interface RefreshContextType {
  refreshInterval: number
  setRefreshInterval: (interval: number) => void
  refreshKey: number
  triggerRefresh: () => void
}

export const RefreshContext = createContext<RefreshContextType>({
  refreshInterval: 1000,
  setRefreshInterval: () => {},
  refreshKey: 0,
  triggerRefresh: () => {},
})

export const useRefreshInterval = () => useContext(RefreshContext)
