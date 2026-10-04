import { useEffect, useSyncExternalStore } from 'react'
import { api } from '../api/current'
import { createEsimManager } from '../utils/esimManagerStore'
import type { EsimManager } from '../utils/esimManagerStore'

const managers = new Map<string, EsimManager>()
function requireData<T>(response: { data?: T | null }): T {
  if (response.data === null || response.data === undefined) throw new Error('未读取到 eSIM 响应')
  return response.data
}

function getManager(lineId: string) {
  let manager = managers.get(lineId)
  if (!manager) {
    manager = createEsimManager(lineId, {
      status: async () => requireData(await api.getEsimLpacStatus()),
      euicc: async (id, force) => requireData(await api.getEsimEuicc(id, force)),
      profiles: async (id) => requireData(await api.getEsimProfiles(id)).profiles,
      progress: async (id) => requireData(await api.getBasebandRestartStatus(id)),
      enable: async (id, iccid) => requireData(await api.enableEsimProfile(id, iccid)),
      rename: async (id, iccid, name) => requireData(await api.renameEsimProfile(id, iccid, name)),
      delete: async (id, iccid) => requireData(await api.deleteEsimProfile(id, iccid)),
      download: async (id, form) => requireData(await api.downloadEsimProfile(id, form)),
      delay: (ms) => new Promise((resolve) => window.setTimeout(resolve, ms)),
    })
    managers.set(lineId, manager)
  }
  return manager
}

export function useEsimManager(lineId: string, enabled = true) {
  const manager = getManager(lineId)
  const snapshot = useSyncExternalStore(manager.subscribe, manager.getSnapshot)
  useEffect(() => { if (enabled && lineId) void manager.load() }, [manager, enabled, lineId])
  return { manager, ...snapshot }
}
