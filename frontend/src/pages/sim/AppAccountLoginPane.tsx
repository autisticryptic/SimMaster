import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, CircularProgress, FormControlLabel, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api, type AppLoginAttempt, type AppLoginSetup } from '../../api/current'
import { appBusinessLabel, appLoginError, jegotripLoginInput, appLoginNeedsPolling, appLoginPhase, canSubmitJegotripLogin, emptyAppLoginDraft } from '../../policies/appLogin'

const emptyDraft = emptyAppLoginDraft

export default function AppAccountLoginPane({ open, onCreated }: { open: boolean; onCreated?: () => void }) {
  const [draft, setDraft] = useState<AppLoginDraft>(emptyDraft)
  const [setup, setSetup] = useState<AppLoginSetup | null>(null)
  const [attempt, setAttempt] = useState<AppLoginAttempt | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef({ active: false })
  const accepting = !!setup?.accepting

  useEffect(() => {
    const scope = { active: open }
    generation.current = scope
    setDraft(emptyDraft()); setAttempt(null); setSubmitting(false); setError(null); setSetup(null)
    if (open) void api.getJegotripLoginSetup().then((response) => {
      if (scope.active) setSetup(response.data ?? null)
    }).catch(() => { if (scope.active) setError('无法读取数据库账号登录配置。') })
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
  return <Stack spacing={2}>
    <Alert severity="info">登录已绑定号码的无忧行账号，后台自动取得电话 token/SIP 密码、确认托管业务并启动无忧行接收器。不是仅保存 App 认证摘要；业务开启、SDK 在线、电话/短信和媒体就绪分别核验，无需手填个人 SDK 凭据。</Alert>
    {setup && !setup.accepting_authenticate && <Alert severity="warning">App HTTP 安装配置或运行锁尚未准备好，完整登录启动流程不可用。配置使用专用数据库；不再要求单独的 jegotrip-login.json 文件。</Alert>}
    {setup?.accepting_authenticate && !setup.native_configured && <Alert severity="warning">完整原生启动配置尚未就绪，暂不能登录并启动无忧行。程序不会降级为仅认证、使用占位值启动或显示接收就绪。</Alert>}
    {setup && <Typography variant="caption" color="text.secondary">配置来源：{setup.configuration_source} · 数据库版本：{setup.configuration_revision}</Typography>}
    <TextField select size="small" label="登录方式" value={draft.mode} disabled={pending}
      onChange={(event) => setDraft({ ...draft, mode: event.target.value === 'phone' ? 'phone' : 'account' })}>
      <MenuItem value="phone">手机号和密码</MenuItem><MenuItem value="account">无忧行账号和密码</MenuItem>
    </TextField>
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
  </Stack>
}
