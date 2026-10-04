import type { EsimDownloadRequest, EsimEuiccInfo, EsimLpacStatusResponse, EsimProfile } from '../api/contracts'
import type { EsimQuickSwitchApi } from './esimQuickSwitch.ts'
import { switchEsimProfile } from './esimQuickSwitch.ts'
import { commandSucceeded, CONFIRM_DELETE_PROFILE, profileDeleteBlockReason, redactEid, translateEsimError, validateDownload } from './esimPresentation.ts'

type Command = Awaited<ReturnType<EsimQuickSwitchApi['enable']>>
export type EsimManagerApi = EsimQuickSwitchApi & {
  status: () => Promise<EsimLpacStatusResponse>
  euicc: (lineId: string, force: boolean) => Promise<EsimEuiccInfo>
  rename: (lineId: string, iccid: string, name: string) => Promise<Command>
  delete: (lineId: string, iccid: string) => Promise<Command>
  download: (lineId: string, form: EsimDownloadRequest) => Promise<Command>
}

export type EsimManagerSnapshot = {
  euicc: EsimEuiccInfo | null
  profiles: EsimProfile[]
  lpac: EsimLpacStatusResponse | null
  loading: boolean
  operation: 'switch' | 'delete' | 'rename' | 'download' | null
  detected: boolean
  error: string | null
  success: string | null
  // An ambiguous switch must be explicitly refreshed before another mutation.
  needsRefresh: boolean
}

/** One store per line. Its lock survives closing/reopening the workbench and
 * reads/writes never migrate to a newly selected line. No unscoped cache reads. */
export function createEsimManager(lineId: string, api: EsimManagerApi) {
  let snapshot: EsimManagerSnapshot = { euicc: null, profiles: [], lpac: null, loading: false, operation: null, detected: false, error: null, success: null, needsRefresh: false }
  const listeners = new Set<() => void>()
  const publish = (patch: Partial<EsimManagerSnapshot>) => {
    snapshot = { ...snapshot, ...patch }
    listeners.forEach((listener) => listener())
  }
  const reportError = (err: unknown) => publish({ error: redactEid(translateEsimError(err instanceof Error ? err.message : String(err)), snapshot.euicc?.eid) })
  let loadingPromise: Promise<void> | null = null
  let warm = false

  const read = async (force: boolean) => {
    const lpac = await api.status()
    publish({ lpac })
    if (!lpac.usable) {
      publish({ euicc: null, profiles: [], detected: false })
      throw new Error(lpac.message || 'lpac 当前不可用')
    }
    // lpac reads share a physical channel; do not run these in parallel.
    const euicc = await api.euicc(lineId, force)
    publish({ euicc, detected: true })
    const profiles = await api.profiles(lineId)
    publish({ profiles, needsRefresh: false })
    warm = true
  }
  const load = (force = false): Promise<void> => {
    if (loadingPromise) return loadingPromise
    if (snapshot.operation || (!force && warm)) return Promise.resolve()
    publish({ loading: true, error: null })
    loadingPromise = read(force).catch((err: unknown) => {
      warm = false
      reportError(err)
      publish({ needsRefresh: true })
    }).finally(() => {
      loadingPromise = null
      publish({ loading: false })
    })
    return loadingPromise
  }

  const mutate = async (operation: NonNullable<EsimManagerSnapshot['operation']>, task: (markSubmitted: () => void) => Promise<void>, message: string) => {
    if (snapshot.loading || snapshot.operation || snapshot.needsRefresh || !snapshot.lpac?.usable || !snapshot.detected) return false
    publish({ operation, error: null, success: null })
    let submitted = false
    try {
      await task(() => { submitted = true })
      publish({ success: message })
      warm = false
      try { await read(true) } catch (err) { reportError(err); publish({ needsRefresh: true }) }
      return true
    } catch (err) {
      warm = false
      reportError(err)
      // A lost write response does not prove that the card was unchanged.
      // Require fresh authoritative state before any retry (not just switching).
      if (submitted || operation === 'switch') publish({ needsRefresh: true })
      return false
    } finally {
      publish({ operation: null })
    }
  }
  const assertCommand = (response: Command, fallback: string) => {
    if (!commandSucceeded(response)) throw new Error(typeof response.data === 'string' && response.data.includes('MatchingID is refused') ? response.data : response.msg || fallback)
  }
  const assertIdle = async () => {
    if ((await api.progress(lineId)).running) throw new Error('当前线路正在切换或恢复，请等待完成')
  }

  return {
    lineId,
    getSnapshot: () => snapshot,
    subscribe: (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener) } },
    load,
    clearError: () => publish({ error: null }),
    clearSuccess: () => publish({ success: null }),
    reportError,
    notify: (success: string) => publish({ success }),
    rename: (iccid: string, input: string) => {
      const name = input.trim()
      if (!name) { reportError('请输入 Profile 名称'); return Promise.resolve(false) }
      return mutate('rename', async (markSubmitted) => {
        await assertIdle()
        if (!snapshot.profiles.some((profile) => profile.iccid === iccid)) throw new Error('未找到此配置，请刷新列表')
        markSubmitted()
        assertCommand(await api.rename(lineId, iccid, name), 'Profile 重命名失败')
        publish({ profiles: snapshot.profiles.map((profile) => profile.iccid === iccid ? { ...profile, name } : profile) })
      }, 'Profile 名称已更新')
    },
    delete: (iccid: string, confirmation: string) => {
      if (confirmation !== CONFIRM_DELETE_PROFILE) return Promise.resolve(false)
      return mutate('delete', async (markSubmitted) => {
        await assertIdle()
        const profiles = await api.profiles(lineId)
        publish({ profiles })
        const target = profiles.find((profile) => profile.iccid === iccid)
        if (!target) throw new Error('未找到此配置，请刷新列表')
        const blocked = profileDeleteBlockReason(target)
        if (blocked) throw new Error(blocked)
        markSubmitted()
        assertCommand(await api.delete(lineId, iccid), 'Profile 删除失败')
        publish({ profiles: profiles.filter((profile) => profile.iccid !== iccid) })
      }, 'Profile 删除完成')
    },
    switch: (iccid: string) => mutate('switch', async () => {
      // This store remains bound to this line even after its UI unmounts. Keep
      // observing recovery, never optimistically enable and never resubmit.
      const profiles = await switchEsimProfile(api, lineId, iccid, () => true)
      publish({ profiles })
    }, '配置切换已确认'),
    download: (form: EsimDownloadRequest) => {
      const validation = validateDownload(form)
      if (validation) { reportError(validation); return Promise.resolve(false) }
      return mutate('download', async (markSubmitted) => {
        await assertIdle()
        markSubmitted()
        assertCommand(await api.download(lineId, { smdp: form.smdp.trim(), matching_id: form.matching_id.trim(), confirmation_code: form.confirmation_code?.trim() || undefined, imei: form.imei?.trim() || undefined }), 'lpac 执行写卡指令失败')
      }, 'Profile 写入成功')
    },
  }
}

export type EsimManager = ReturnType<typeof createEsimManager>
