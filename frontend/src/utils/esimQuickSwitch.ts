import type { BasebandRestartResponse, EsimCommandResponse, EsimProfile } from '../api/contracts'

export function esimProfileActive(profile: Pick<EsimProfile, 'state'>): boolean {
  return ['active', 'enabled', '1'].includes(profile.state.trim().toLowerCase())
}

export function esimSwitchBlockReason(profile: EsimProfile, profiles: readonly EsimProfile[]): string | null {
  if (esimProfileActive(profile)) return '已启用'
  if (!profile.iccid || !['disabled', 'inactive', '0'].includes(profile.state.trim().toLowerCase())) return '状态不可切换'
  if (profiles.some((item) => esimProfileActive(item) && item.disable_allowed === false)) return '当前配置不允许停用'
  return null
}

export type EsimQuickSwitchApi = {
  profiles: (lineId: string) => Promise<EsimProfile[]>
  progress: (lineId: string) => Promise<BasebandRestartResponse>
  enable: (lineId: string, iccid: string) => Promise<EsimCommandResponse>
  delay: (milliseconds: number) => Promise<void>
}

/** Exactly one enable request, followed only by bounded read-only observation.
 * "task accepted" is not "profile enabled"; only fresh readback confirms it. */
export async function switchEsimProfile(
  api: EsimQuickSwitchApi,
  lineId: string,
  iccid: string,
  isCurrent: () => boolean,
  options: { maxPolls?: number; now?: () => number } = {},
): Promise<EsimProfile[]> {
  const ensureCurrent = () => { if (!isCurrent()) throw new Error('线路已变化，停止界面等待；已提交的切换可能仍在后台执行') }
  const now = options.now ?? Date.now
  const deadline = now() + 180_000
  ensureCurrent()
  const before = await api.profiles(lineId)
  ensureCurrent()
  const target = before.find((profile) => profile.iccid === iccid)
  if (!target) throw new Error('未找到此配置，请刷新列表')
  const blocked = esimSwitchBlockReason(target, before)
  if (blocked) throw new Error(blocked)
  const idle = await api.progress(lineId)
  ensureCurrent()
  if (idle.running) throw new Error('当前线路正在切换或恢复，请等待完成')
  const accepted = await api.enable(lineId, iccid)
  ensureCurrent()
  if (accepted.code !== 0 || !['', 'ok', 'success'].includes(accepted.status.toLowerCase())) {
    throw new Error(accepted.msg || '切换请求未被接受')
  }
  for (let count = 0; count < (options.maxPolls ?? 90) && now() < deadline; count++) {
    ensureCurrent()
    const progress = await api.progress(lineId)
    ensureCurrent()
    const failure = progress.steps.find((step) => step.status === 'error')
    if (failure) throw new Error(failure.detail || `${failure.step}失败，请刷新确认当前配置`)
    if (!progress.running) {
      const profiles = await api.profiles(lineId)
      ensureCurrent()
      if (profiles.some((profile) => profile.iccid === iccid && esimProfileActive(profile))) return profiles
    }
    await api.delay(Math.max(0, Math.min(2000, deadline - now())))
  }
  throw new Error('切换尚未确认，后台可能仍在执行；请刷新查看，不要重复提交')
}
