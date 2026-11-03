import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, CircularProgress, Divider, FormControlLabel, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api, type AppLoginAttempt, type AppLoginSetup, type JegotripAppVersionView } from '../../api/current'
import { appBusinessLabel, appLoginError, jegotripLoginInput, appLoginNeedsPolling, appLoginPhase, canSubmitJegotripLogin, emptyAppLoginDraft, validJegotripDeviceId, type AppLoginDraft } from '../../policies/appLogin'
import JegotripDeviceIdentityPane from './JegotripDeviceIdentityPane'
import JegotripPushStatusPane from './JegotripPushStatusPane'

const emptyDraft = emptyAppLoginDraft

export default function AppAccountLoginPane({ open, onCreated }: { open: boolean; onCreated?: () => void }) {
  const [draft, setDraft] = useState<AppLoginDraft>(emptyDraft)
  const [setup, setSetup] = useState<AppLoginSetup | null>(null)
  const [appVersion, setAppVersion] = useState<JegotripAppVersionView | null>(null)
  const [versionInput, setVersionInput] = useState('')
  const [versionBusy, setVersionBusy] = useState(false)
  const [versionNote, setVersionNote] = useState<string | null>(null)
  const [attempt, setAttempt] = useState<AppLoginAttempt | null>(null)
  const [deviceRefresh, setDeviceRefresh] = useState(0)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef({ active: false })
  const accepting = !!setup?.accepting

  useEffect(() => {
    const scope = { active: open }
    generation.current = scope
    setDraft(emptyDraft()); setAttempt(null); setSubmitting(false); setError(null); setSetup(null)
    setAppVersion(null); setVersionInput(''); setVersionNote(null); setDeviceRefresh(0)
    if (open) void api.getJegotripLoginSetup().then((response) => {
      if (scope.active) setSetup(response.data ?? null)
    }).catch(() => { if (scope.active) setError('无法读取数据库账号登录配置。') })
    if (open) void api.getJegotripAppVersion().then((response) => {
      if (!scope.active) return
      setAppVersion(response.data ?? null); setVersionInput(response.data?.stored?.version ?? '')
    }).catch(() => { if (scope.active) setVersionNote('无法读取 App 版本状态。') })
    return () => { scope.active = false }
  }, [open])

  const attemptId = attempt && appLoginNeedsPolling(attempt) ? attempt.attempt_id : undefined
  useEffect(() => {
    if (!open || !attemptId) return
    let active = true
    let timer: ReturnType<typeof setTimeout> | undefined
    const deadline = Date.now() + 150000
    const poll = async () => {
      try {
        const response = await api.getJegotripLoginAttempt(attemptId)
        if (!active) return
        const value = response.data
        if (!value) throw new Error('missing attempt')
        setAttempt(value)
        if (value.error) setError(appLoginError(value.error))
        if (!appLoginNeedsPolling(value)) {
          setDeviceRefresh((current) => current + 1)
          if (value.account_id || value.line_id) onCreated?.()
          return
        }
        if (Date.now() >= deadline) { setError('任务尚未确认结束，请查看原任务；不要重复提交。'); return }
        timer = setTimeout(() => { void poll() }, 1200)
      } catch { if (active) setError('暂时无法读取原任务，任务可能仍在执行；请查看任务列表，不要重复提交。') }
    }
    timer = setTimeout(() => { void poll() }, 400)
    return () => { active = false; if (timer) clearTimeout(timer) }
  }, [open, attemptId, onCreated])

  const submit = async () => {
    if (submitting || !accepting || !canSubmitJegotripLogin(draft) || (attempt && appLoginNeedsPolling(attempt))) return
    const scope = generation.current
    if (!scope.active) return
    const bytes = new Uint8Array(16)
    if (!globalThis.crypto?.getRandomValues) { setError('浏览器不支持安全随机标识，请使用 HTTPS 和现代浏览器。'); return }
    globalThis.crypto.getRandomValues(bytes)
    const id = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
    setSubmitting(true); setError(null); setAttempt(null)
    try {
      const response = await api.submitJegotripLogin(jegotripLoginInput(id, draft))
      if (scope.active) {
        if (!response.data) throw new Error('missing attempt')
        setAttempt(response.data)
      }
    } catch (failure) {
      // A lost response is not permission to repeat login or a business write.
      try {
        const existing = await api.getJegotripLoginAttempt(id)
        if (scope.active && existing.data) setAttempt(existing.data)
      } catch { if (scope.active) setError(appLoginError(failure)) }
    } finally {
      if (scope.active) { setDraft((value) => ({ ...value, password: '', consent: false })); setSubmitting(false) }
    }
  }

  const pending = submitting || (attempt !== null && appLoginNeedsPolling(attempt))
  const versionValid = /^\d+(\.\d+)*$/.test(versionInput.trim()) && versionInput.trim().length <= 24
  const storeVersion = async () => {
    if (versionBusy || !versionValid) return
    setVersionBusy(true); setVersionNote(null)
    try {
      const response = await api.saveJegotripAppVersion(versionInput.trim())
      setAppVersion(response.data ?? null); setVersionNote('已写入共享状态表，下次登录立即生效。')
    } catch (failure) { setVersionNote(appLoginError(failure)) } finally { setVersionBusy(false) }
  }
  const refreshVersion = async () => {
    if (versionBusy) return
    setVersionBusy(true); setVersionNote(null)
    try {
      const response = await api.refreshJegotripAppVersion()
      setAppVersion(response.data ?? null); setVersionInput(response.data?.stored?.version ?? versionInput)
      setVersionNote('已按 App Store/官方页面重新拉取并写入。')
    } catch (failure) { setVersionNote(appLoginError(failure)) } finally { setVersionBusy(false) }
  }
  const sourceLabel: Record<string, string> = { manual: '手工设置', ios: 'iOS App Store', page: '官方页面', store: '共享状态表', file: '版本文件', static: '静态配置' }
  return <Stack spacing={2}>
    <Alert severity="info">登录已绑定号码的无忧行账号，后台自动取得电话 token/SIP 密码、确认托管业务并启动无忧行接收器。不是仅保存 App 认证摘要；业务开启、SDK 在线、电话/短信和媒体就绪分别核验，无需手填个人 SDK 凭据。</Alert>
    {setup && !setup.accepting_authenticate && <Alert severity="warning">App HTTP 安装配置或运行锁尚未准备好，完整登录启动流程不可用。配置使用专用数据库；不再要求单独的 jegotrip-login.json 文件。</Alert>}
    {setup?.accepting_authenticate && !setup.native_configured && <Alert severity="warning">完整原生启动配置尚未就绪，暂不能登录并启动无忧行。程序不会降级为仅认证、使用占位值启动或显示接收就绪。</Alert>}
    {setup && <Typography variant="caption" color="text.secondary">配置来源：{setup.configuration_source} · 数据库版本：{setup.configuration_revision}</Typography>}
    <Stack spacing={1}>
      <Typography variant="subtitle2">App 客户端版本</Typography>
      <Typography variant="caption" color="text.secondary">
        {appVersion?.stored
          ? `共享状态表：${appVersion.stored.version}（${sourceLabel[appVersion.stored.source] ?? appVersion.stored.source}）`
          : '共享状态表暂无版本号；首次登录会先按已配置来源拉取并写入。'}
        {appVersion && ` · 来源：${[appVersion.configured_ios_lookup ? 'App Store' : '', appVersion.configured_mobile_page ? '官方页面' : '',
          appVersion.configured_file ? '版本文件' : '', appVersion.configured_static ? '静态值' : ''].filter(Boolean).join('/') || '未配置'}`}
      </Typography>
      <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1}>
        <TextField size="small" label="App 版本（如 8.9.8.0）" value={versionInput} disabled={versionBusy}
          onChange={(event) => setVersionInput(event.target.value)} error={versionInput.length > 0 && !versionValid} />
        <Button variant="outlined" disabled={versionBusy || !versionValid} onClick={() => { void storeVersion() }}>保存</Button>
        <Button variant="outlined" disabled={versionBusy} onClick={() => { void refreshVersion() }}>拉取最新</Button>
      </Stack>
      {versionNote && <Alert severity="info">{versionNote}</Alert>}
    </Stack>
    <TextField select size="small" label="登录方式" value={draft.mode} disabled={pending}
      onChange={(event) => setDraft({ ...draft, mode: event.target.value === 'phone' ? 'phone' : 'account' })}>
      <MenuItem value="phone">手机号和密码</MenuItem><MenuItem value="account">无忧行账号和密码</MenuItem>
    </TextField>
    <TextField select size="small" label="本次登录的设备标识" value={draft.deviceMode ?? 'auto'} disabled={pending}
      onChange={(event) => setDraft({ ...draft, deviceMode: event.target.value === 'manual' ? 'manual' : 'auto' })}>
      <MenuItem value="auto">自动生成 / 复用已保存标识（默认）</MenuItem><MenuItem value="manual">手工指定自己的标识</MenuItem>
    </TextField>
    {(draft.deviceMode ?? 'auto') === 'manual' && <TextField size="small" label="首次登录使用的设备标识" value={draft.deviceId ?? ''} disabled={pending}
      onChange={(event) => setDraft({ ...draft, deviceId: event.target.value })} inputProps={{ maxLength: 64 }}
      error={!!draft.deviceId && !validJegotripDeviceId(draft.deviceId)}
      helperText="8–64 位，仅字母、数字、下划线与连字符；现有账号更换标识请用下方管理，不会自动覆盖旧绑定。" />}
    <Typography variant="caption" color="text.secondary">标识保存在独立数据库中，重试/重启不反复随机。不同号码不共用；标识不是认证 token，也不保证通过服务端一键验证。</Typography>
    <TextField select size="small" label="托管操作" value={draft.intent ?? 'enable_hosting'} disabled={pending}
      onChange={(event) => setDraft({ ...draft, intent: event.target.value as AppLoginDraft['intent'] })}>
      <MenuItem value="enable_hosting">登录并开启托管（需要服务端允许本机开启）</MenuItem>
      <MenuItem value="attach_hosting">接管已开启的托管（先在手机 App 开启）</MenuItem>
    </TextField>
    {(draft.intent ?? 'enable_hosting') === 'attach_hosting' && <Alert severity="info">本机不会修改服务端托管开关：只有服务端已确认托管处于开启状态时才继续创建本地账号并启动接收器；否则以「托管未开启」结束。</Alert>}
    {draft.mode === 'phone' && <TextField size="small" label="国家/地区区号" value={draft.country} disabled={pending} onChange={(event) => setDraft({ ...draft, country: event.target.value })} />}
    <TextField size="small" label={draft.mode === 'phone' ? '已绑定手机号' : '无忧行账号'} value={draft.username} disabled={pending} autoComplete="username" onChange={(event) => setDraft({ ...draft, username: event.target.value })} />
    <TextField size="small" label="无忧行密码" type="password" value={draft.password} disabled={pending} autoComplete="current-password" onChange={(event) => setDraft({ ...draft, password: event.target.value })} />
    <FormControlLabel control={<Checkbox checked={draft.startReceiver} disabled={pending} onChange={(event) => setDraft({ ...draft, startReceiver: event.target.checked })} />} label="业务确认后立即启动无忧行接收器（默认开启）" />
    <FormControlLabel control={<Checkbox checked={draft.consent} disabled={pending} onChange={(event) => setDraft({ ...draft, consent: event.target.checked })} />} label="我拥有此账号及绑定号码，并授权登录、开启托管业务和启动所选接收器" />
    <Typography variant="caption" color="text.secondary">请通过 HTTPS 或可信本机输入。密码不保存到数据库或浏览器存储；App 会话只用于本次任务。关闭窗口不撤销已发请求，结果保留在独立账号/任务表中。</Typography>
    <Button variant="contained" disabled={pending || !accepting || !canSubmitJegotripLogin(draft)} onClick={() => { void submit() }}>
      {pending ? <CircularProgress size={18} color="inherit" /> : '登录并启动无忧行'}
    </Button>
    {attempt && <Alert severity={attempt.error ? 'warning' : attempt.completed ? 'success' : 'info'}>
      {appLoginPhase(attempt.phase)} · {appBusinessLabel(attempt.business_state)}{attempt.masked_number ? ` · ${attempt.masked_number}` : ''}
      {attempt.account_id && !attempt.line_id && <Typography variant="caption" display="block">App 认证已有记录；完整托管/接收任务尚未成功，不能据此视为无忧行已启动。</Typography>}
      {attempt.line_id && <Typography variant="caption" display="block">接收账号已保存，电话/短信是否就绪以原运行时为准。</Typography>}
    </Alert>}
    {error && <Alert severity="error">{error}</Alert>}
    <Divider />
    {open && <JegotripPushStatusPane open refreshKey={deviceRefresh} />}
    <Divider />
    <JegotripDeviceIdentityPane open={open} disabled={pending} refreshKey={deviceRefresh} />
  </Stack>
}
