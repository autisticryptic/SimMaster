import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, CircularProgress, FormControlLabel, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api, type AppLoginAttempt, type AppLoginSetup } from '../../api/current'
import { appBusinessLabel, appLoginError, appLoginInput, appLoginNeedsPolling, appLoginPhase, canSubmitAppLogin, type AppLoginDraft } from '../../policies/appLogin'

const emptyDraft = (): AppLoginDraft => ({ mode: 'phone', username: '', password: '', country: '86', consent: false, startReceiver: true })

export default function AppAccountLoginPane({ open, onCreated }: { open: boolean; onCreated?: () => void }) {
  const [draft, setDraft] = useState<AppLoginDraft>(emptyDraft)
  const [setup, setSetup] = useState<AppLoginSetup | null>(null)
  const [attempt, setAttempt] = useState<AppLoginAttempt | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef(0)

  useEffect(() => {
    const scope = ++generation.current
    setDraft(emptyDraft())
    setAttempt(null)
    setSubmitting(false)
    setError(null)
    setSetup(null)
    if (open) {
      void api.getAppLoginSetup().then((response) => {
        if (generation.current === scope) setSetup(response.data ?? null)
      }).catch(() => {
        if (generation.current === scope) setError('无法读取后台账号登录配置。')
      })
    }
    return () => { generation.current++ }
  }, [open])

  const attemptId = attempt && appLoginNeedsPolling(attempt) ? attempt.attempt_id : undefined
  useEffect(() => {
    if (!open || !attemptId) return
    let active = true
    let timer: ReturnType<typeof setTimeout> | undefined
    const deadline = Date.now() + 150000
    const poll = async () => {
      try {
        const response = await api.getAppLoginAttempt(attemptId)
        if (!active) return
        const value = response.data
        if (!value) throw new Error('missing attempt')
        setAttempt(value)
        if (value.error) setError(appLoginError(value.error))
        if (!appLoginNeedsPolling(value)) {
          if (value.line_id) onCreated?.()
          return
        }
        if (Date.now() >= deadline) {
          setError('任务尚未确认结束，请查看登录任务列表；不要重复提交开通。')
          return
        }
        timer = setTimeout(() => { void poll() }, 1200)
      } catch {
        if (active) setError('暂时无法读取原任务，任务可能仍在执行；请查看任务列表，不要重复开通。')
      }
    }
    timer = setTimeout(() => { void poll() }, 400)
    return () => { active = false; if (timer) clearTimeout(timer) }
  }, [open, attemptId, onCreated])

  const submit = async () => {
    if (submitting || !setup?.accepting || !canSubmitAppLogin(draft) || (attempt && appLoginNeedsPolling(attempt))) return
    const scope = generation.current
    const bytes = new Uint8Array(16)
    if (!globalThis.crypto?.getRandomValues) { setError('浏览器不支持安全随机标识，请使用 HTTPS 和现代浏览器。'); return }
    globalThis.crypto.getRandomValues(bytes)
    const id = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
    setSubmitting(true)
    setError(null)
    setAttempt(null)
    try {
      const response = await api.submitAppLogin(appLoginInput(id, draft))
      if (generation.current === scope) {
        if (!response.data) throw new Error('missing attempt')
        setAttempt(response.data)
      }
    } catch (failure) {
      // A lost POST response is not evidence that no business request happened.
      // Read the original client-generated ID; never resubmit automatically.
      try {
        const existing = await api.getAppLoginAttempt(id)
        if (generation.current === scope && existing.data) setAttempt(existing.data)
      } catch {
        if (generation.current === scope) setError(appLoginError(failure))
      }
    } finally {
      if (generation.current === scope) {
        setDraft((value) => ({ ...value, password: '', consent: false }))
        setSubmitting(false)
      }
    }
  }

  const pending = submitting || (attempt !== null && appLoginNeedsPolling(attempt))
  return (
    <Stack spacing={2}>
      <Alert severity="info">
        登录已绑定手机号的无忧行账号，后台自动获取通话凭据并请求开启托管，不需要手填电话 token 或 SIP 密码。
        服务要求额外验证时会如实停止，不伪造成功。
      </Alert>
      {setup && !setup.accepting && <Alert severity="warning">
        后台尚未准备好登录部署配置或运行锁。请由管理员配置 data.db 旁的 jegotrip-login.json；这不是个人电话凭据导入。
      </Alert>}
      <TextField select size="small" label="登录方式" value={draft.mode} disabled={pending}
        onChange={(event) => setDraft({ ...draft, mode: event.target.value === 'phone' ? 'phone' : 'account' })}>
        <MenuItem value="phone">手机号和密码</MenuItem>
        <MenuItem value="account">无忧行账号和密码</MenuItem>
      </TextField>
      {draft.mode === 'phone' && <TextField size="small" label="国家/地区区号" value={draft.country} disabled={pending}
        onChange={(event) => setDraft({ ...draft, country: event.target.value })} />}
      <TextField size="small" label={draft.mode === 'phone' ? '已绑定手机号' : '无忧行账号'} value={draft.username} disabled={pending}
        autoComplete="username" onChange={(event) => setDraft({ ...draft, username: event.target.value })} />
      <TextField size="small" label="无忧行密码" type="password" value={draft.password} disabled={pending}
        autoComplete="current-password" onChange={(event) => setDraft({ ...draft, password: event.target.value })} />
      <FormControlLabel control={<Checkbox checked={draft.startReceiver} disabled={pending}
        onChange={(event) => setDraft({ ...draft, startReceiver: event.target.checked })} />} label="业务开启后启动接收器" />
      <FormControlLabel control={<Checkbox checked={draft.consent} disabled={pending}
        onChange={(event) => setDraft({ ...draft, consent: event.target.checked })} />} label="我拥有此账号及绑定号码，并授权登录和开启其托管业务" />
      <Typography variant="caption" color="text.secondary">
        请通过 HTTPS 或可信本机连接输入密码。提交后关闭窗口不会撤销已发请求，任务结果保留在应用账号区；口令不会保存在浏览器存储中。
      </Typography>
      <Button variant="contained" disabled={pending || !setup?.accepting || !canSubmitAppLogin(draft)} onClick={() => { void submit() }}>
        {pending ? <CircularProgress size={18} color="inherit" /> : '登录并开启托管'}
      </Button>
      {attempt && <Alert severity={attempt.error ? 'warning' : attempt.completed ? 'success' : 'info'}>
        {appLoginPhase(attempt.phase)} · {appBusinessLabel(attempt.business_state)}
        {attempt.masked_number ? ` · ${attempt.masked_number}` : ''}
        {attempt.line_id && <Typography variant="caption" display="block">账号已保存，接电话/短信是否就绪以原运行时状态为准。</Typography>}
      </Alert>}
      {error && <Alert severity="error">{error}</Alert>}
    </Stack>
  )
}
