import type { ModemBinding, VolteLineControlResponse } from '../api/current'

export function shortLineId(lineId: string) {
  return lineId.startsWith('line-') ? lineId.slice(-6).toUpperCase() : lineId
}

export function maskedIccid(iccid?: string) {
  if (!iccid) return '未读取 ICCID'
  return iccid.length > 6 ? `•••• ${iccid.slice(-6)}` : iccid
}

export function modemLineLabel(line: VolteLineControlResponse, index?: number) {
  const prefix = index === undefined ? '线路' : `线路 ${index + 1}`
  const identity = line.modem.model || line.modem.manufacturer || `基带 ${line.modem.modem_id}`
  return `${prefix} · ${identity} · ${maskedIccid(line.modem.sim_iccid)}`
}

export function modemSlotLabel(modem: Pick<ModemBinding, 'display_order' | 'slot_label'>, fallbackIndex?: number) {
  return modem.slot_label || (modem.display_order > 0
    ? `基带 ${modem.display_order}`
    : `基带 ${(fallbackIndex ?? 0) + 1}`)
}

export function stableModemSort<T extends { modem: Pick<ModemBinding, 'display_order' | 'line_id' | 'present'> }>(lines: T[]) {
  return [...lines].sort((left, right) => {
    const leftOrder = left.modem.display_order || Number.MAX_SAFE_INTEGER
    const rightOrder = right.modem.display_order || Number.MAX_SAFE_INTEGER
    return leftOrder - rightOrder
      || Number(right.modem.present) - Number(left.modem.present)
      || left.modem.line_id.localeCompare(right.modem.line_id)
  })
}
