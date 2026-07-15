import type { VolteLineControlResponse } from '../api/current'

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
