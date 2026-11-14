import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, CircularProgress, Divider, FormControlLabel, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api, type AppLoginAttempt, type AppLoginSetup, type JegotripAppVersionView } from '../../api/current'
import { appBusinessLabel, appLoginError, jegotripLoginInput, appLoginNeedsPolling, appLoginPhase, canSubmitJegotripLogin, emptyAppLoginDraft, validJegotripDeviceId, validPhoneAppUuid, deviceTrustQueryLabel, deviceSyncLabel, callForwardingQueryLabel, localReceiverTaskLabel, type AppLoginDraft } from '../../policies/appLogin'
import JegotripDeviceIdentityPane from './JegotripDeviceIdentityPane'
import JegotripPushStatusPane from './JegotripPushStatusPane'
import JegotripPhoneBackupInput from './JegotripPhoneBackupInput'

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
  const [backupNote, setBackupNote] = useState<string | null>(null)
  const generation = useRef({ active: false })
  const checkingTrust = draft.intent === 'check_device_trust'
  const reusingTrust = draft.intent === 'reuse_hosting' || draft.intent === 'enable_trusted_hosting'
  const localReceiver = draft.intent === 'prepare_local_receiver'
  const accepting = !!setup?.accepting
  const acceptingSelected = checkingTrust ? !!setup?.accepting_authenticate : localReceiver ? !!setup?.accepting_local_receiver : accepting

  useEffect(() => {
    const scope = { active: open }
    generation.current = scope
    setDraft(emptyDraft()); setAttempt(null); setSubmitting(false); setError(null); setSetup(null); setBackupNote(null)
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
    if (submitting || !acceptingSelected || !canSubmitJegotripLogin(draft) || (attempt && appLoginNeedsPolling(attempt))) return
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
    <Alert severity="info">本人账号登录、设备授信读回、托管业务、SDK 在线和短信/来电媒体分别核验。复用手机授信时保持原 App UUID，不申请新授信；复用流程的电话 token/SIP 密码仅在服务器明确认可后由后台取得，接收就绪仍以原运行时为准。</Alert>
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
      onChange={(event) => {
        const mode = event.target.value === 'phone_app' ? 'phone_app' : event.target.value === 'manual' ? 'manual' : 'auto'
        setBackupNote(null)
        setDraft({ ...draft, deviceMode: mode, consent: false,
          intent: mode === 'phone_app' ? 'reuse_hosting' : reusingTrust || localReceiver ? 'attach_hosting' : draft.intent,
          startReceiver: mode === 'phone_app' ? true : draft.startReceiver })
      }}>
      <MenuItem value="auto">自动生成 / 复用已保存标识（默认）</MenuItem><MenuItem value="manual">手工指定自己的标识</MenuItem>
      <MenuItem value="phone_app">复用手机 App 已授信 UUID</MenuItem>
    </TextField>
    {(draft.deviceMode === 'manual' || draft.deviceMode === 'phone_app') && <TextField size="small"
      label={draft.deviceMode === 'phone_app' ? '手机 App 授信 UUID' : '首次登录使用的设备标识'} value={draft.deviceId ?? ''} disabled={pending}
      onChange={(event) => { setBackupNote(null); setDraft({ ...draft, deviceId: event.target.value }) }} inputProps={{ maxLength: 64 }}
      error={!!draft.deviceId && !(draft.deviceMode === 'phone_app' ? validPhoneAppUuid(draft.deviceId) : validJegotripDeviceId(draft.deviceId))}
      helperText={draft.deviceMode === 'phone_app'
        ? '复制本人手机 App 的 phone_uuid / device.dat 中原样的 36 位 UUID（含连字符）；不是硬件 IMEI、phone_safe_num 的 MD5、enrollId 或 SDK UUID。已有账号更换请先用下方标识管理。'
        : '8–64 位，仅字母、数字、下划线与连字符；现有账号更换标识请用下方管理，不会自动覆盖旧绑定。'} />}
    <JegotripPhoneBackupInput open={open} disabled={pending}
      contextKey={JSON.stringify([draft.deviceMode, draft.deviceId, draft.intent, draft.mode, draft.username, draft.country])}
      label="从原始备份读取登录 UUID（仅本地）" onUuid={(value) => {
        setDraft((current) => ({ ...current, deviceMode: 'phone_app', deviceId: value, consent: false,
          intent: current.intent === 'check_device_trust' ? 'check_device_trust' : 'reuse_hosting',
          startReceiver: current.intent !== 'check_device_trust' }))
        setBackupNote('已在本地读取 App UUID；没有上传备份、保存标识或查询服务器。授权勾选已清除，账号/号码仍需你核对，正式复用仍以本次服务器结果为准。')
        setError(null)
      }} />
    {backupNote && <Alert severity="info">{backupNote}</Alert>}
    {draft.deviceMode === 'phone_app' && <Alert severity="info">先在本人手机完成授信，再复用同一账号/号码与原 App UUID。复用接管/开启流程不 enroll、不请求 VTD，也不使用旧 enrollId；不会自动改用另一 UUID。按单设备复用边界，不同时申请新设备授信；手机 App 同时收件还可能发生独立的 SDK 会话冲突。</Alert>}
    <Typography variant="caption" color="text.secondary">标识保存在独立数据库中，重试/重启不反复随机。不同号码不共用；标识不是认证 token，也不保证通过服务端一键验证。</Typography>
    <TextField select size="small" label="托管操作" value={draft.intent ?? 'enable_hosting'} disabled={pending}
      onChange={(event) => {
        const intent = event.target.value as AppLoginDraft['intent']
        setDraft({ ...draft, intent, consent: false,
          startReceiver: intent === 'check_device_trust' ? false : checkingTrust ? true : draft.startReceiver })
      }}>
      <MenuItem value="enable_hosting" disabled={draft.deviceMode === 'phone_app'}>登录并开启托管（需要服务端允许本机开启）</MenuItem>
      <MenuItem value="attach_hosting" disabled={draft.deviceMode === 'phone_app'}>接管已开启的托管（先在手机 App 开启）</MenuItem>
      <MenuItem value="reuse_hosting" disabled={draft.deviceMode !== 'phone_app'}>复用已授信 UUID 接管托管（不写业务开关）</MenuItem>
      <MenuItem value="enable_trusted_hosting" disabled={draft.deviceMode !== 'phone_app'}>复用已授信 UUID 并开启托管（明确业务授权）</MenuItem>
      <MenuItem value="prepare_local_receiver" disabled={draft.deviceMode !== 'phone_app' || !setup?.accepting_local_receiver}>准备本地SDK接收器（不改呼转、不报告设备）</MenuItem>
      <MenuItem value="check_device_trust">只查询设备授信（不建接收账号、不改业务、不启动接收）</MenuItem>
    </TextField>
    {localReceiver && <Alert severity="info">本地SDK接收与App呼转业务是不同操作。本次保持原UUID和同账号，查询本次已有设备认可并取得当前电话token和SIP资料。再次核对已有设备后保存本地接收账号，可启动所选SDK会话，但仍需其登录、网关注册回执及正确接收器。getCallStatus的呼转开关只作观察，不自动开通、不申请新授信、不发送设备同步，也不从历史记录恢复授权。</Alert>}
    {checkingTrust && <Alert severity="info">本次仅登录本人账号并查询该 UUID 的设备授信及业务状态。不 enroll、不取电话 token/SIP 密码、不创建原生接收账号、不修改托管开关；查询完成不等于托管或收件成功。</Alert>}
    {reusingTrust && <Alert severity="info">本次以 getCallStatus 对原 UUID 的实时 authentication_code=1 为门禁，不从历史查询恢复授权。取得电话凭据后、写业务或创建接收账号前会再次读回；未认可或状态变化立即停止。仅当安装侧明确配置本人手机的真实报告资料时才发送设备同步，报告被接受不是授信或收件成功。</Alert>}
    {(draft.intent ?? 'enable_hosting') === 'attach_hosting' && <Alert severity="info">本机不会修改服务端托管开关：只有服务端已确认托管处于开启状态时才继续创建本地账号并启动接收器；否则以「托管未开启」结束。</Alert>}
    {draft.mode === 'phone' && <TextField size="small" label="国家/地区区号" value={draft.country} disabled={pending} onChange={(event) => setDraft({ ...draft, country: event.target.value })} />}
    <TextField size="small" label={draft.mode === 'phone' ? '已绑定手机号' : '无忧行账号'} value={draft.username} disabled={pending} autoComplete="username" onChange={(event) => setDraft({ ...draft, username: event.target.value })} />
    <TextField size="small" label="无忧行密码" type="password" value={draft.password} disabled={pending} autoComplete="current-password" onChange={(event) => setDraft({ ...draft, password: event.target.value })} />
    <FormControlLabel control={<Checkbox checked={draft.startReceiver} disabled={pending || checkingTrust} onChange={(event) => setDraft({ ...draft, startReceiver: event.target.checked })} />} label={localReceiver ? '准备后启动本地接收器（仍须SDK注册确认）' : '业务确认后立即启动无忧行接收器（默认开启）'} />
    <FormControlLabel control={<Checkbox checked={draft.consent} disabled={pending} onChange={(event) => setDraft({ ...draft, consent: event.target.checked })} />}
      label={localReceiver ? '我拥有此账号及号码，授权保持原UUID、查询已有设备、取得电话资料并准备或启动所选本地接收器；不修改呼转、不报告设备、不申请新授信'
        : checkingTrust ? '我拥有此账号及号码，仅授权登录和查询设备授信/业务状态'
        : draft.intent === 'reuse_hosting' ? '我拥有此账号及号码，授权保持原 UUID、同步已明确配置的本人资料并接管已开启业务，不申请新授信或开启业务'
          : draft.intent === 'enable_trusted_hosting' ? '我拥有此账号及号码，授权保持原 UUID、同步已明确配置的本人资料、明确开启托管并启动所选接收器，不申请新授信'
            : '我拥有此账号及绑定号码，并授权登录、开启托管业务和启动所选接收器'} />
    <Typography variant="caption" color="text.secondary">请通过 HTTPS 或可信本机输入。密码不保存到数据库或浏览器存储；App 会话只用于本次任务。关闭窗口不撤销已发请求，结果保留在独立账号/任务表中。</Typography>
    <Button variant="contained" disabled={pending || !acceptingSelected || !canSubmitJegotripLogin(draft)} onClick={() => { void submit() }}>
      {pending ? <CircularProgress size={18} color="inherit" /> : checkingTrust ? '查询设备授信' : localReceiver ? '准备本地接收器' : '登录并启动无忧行'}
    </Button>
    {attempt && <Alert severity={attempt.error ? 'warning' : attempt.intent === 'check_device_trust' || attempt.intent === 'prepare_local_receiver' ? 'info' : attempt.completed ? 'success' : 'info'}>
      {attempt.intent === 'prepare_local_receiver' ? localReceiverTaskLabel(attempt) : attempt.intent === 'check_device_trust' && attempt.completed ? '设备授信查询任务已结束' : appLoginPhase(attempt.phase)} · {appBusinessLabel(attempt.business_state)}{attempt.masked_number ? ` · ${attempt.masked_number}` : ''}
      {attempt.intent === 'prepare_local_receiver' && <Typography variant="caption" display="block">本地准备完成不是SDK或收件成功；未修改服务端呼转，也未发送设备同步。</Typography>}
      {attempt.intent === 'check_device_trust' && <Typography variant="caption" display="block">本次不创建接收账号，不代表托管或收件成功。</Typography>}
      {attempt.intent !== 'check_device_trust' && attempt.account_id && !attempt.line_id && <Typography variant="caption" display="block">App 认证已有记录；完整托管/接收任务尚未成功，不能据此视为无忧行已启动。</Typography>}
      {attempt.line_id && <Typography variant="caption" display="block">接收账号已保存，电话/短信是否就绪以原运行时为准。</Typography>}
    </Alert>}
    {attempt?.device_trust && <Alert severity="info">
      <Typography variant="subtitle2">服务器设备授信查询（历史观察）</Typography>
      <Typography variant="body2">设备标识版本：{attempt.device_trust.device_identity_revision} · 查询时间：{new Date(attempt.device_trust.observed_at * 1000).toLocaleString()}</Typography>
      <Typography variant="body2">securityVerify：{deviceTrustQueryLabel(attempt.device_trust.security_verify)}</Typography>
      <Typography variant="body2">已有设备查询：{deviceTrustQueryLabel(attempt.device_trust.existing_device)}</Typography>
      {attempt.device_trust.call_status && <Typography variant="body2">{attempt.intent === 'prepare_local_receiver' ? 'getCallStatus接口观察：' : 'getCallStatus 授信读回：'}{attempt.intent === 'prepare_local_receiver' ? callForwardingQueryLabel(attempt.device_trust.call_status) : deviceTrustQueryLabel(attempt.device_trust.call_status)}</Typography>}
      {attempt.device_trust.local_cs_forward !== undefined && attempt.device_trust.local_cs_forward !== null && <Typography variant="body2">本次 localCsForward：{attempt.device_trust.local_cs_forward}（不是 SDK 或媒体就绪证明）</Typography>}
      {attempt.device_trust.device_sync && <Typography variant="body2">设备同步：{deviceSyncLabel(attempt.device_trust.device_sync)}</Typography>}
      <Typography variant="body2">{attempt.intent === 'prepare_local_receiver' ? '远端呼转业务读回：' : '托管业务读回：'}{attempt.device_trust.hosting_enabled === true ? '该次读回已开启' : attempt.device_trust.hosting_enabled === false ? '该次读回未开启' : '查询失败 / 未确认'}</Typography>
      <Typography variant="caption">只表示上述时间、该标识版本的服务器回答；更换 UUID 或状态变化后需重新核验。不会从记录恢复授信、SDK 或短信/来电 ready。</Typography>
    </Alert>}
    {error && <Alert severity="error">{error}</Alert>}
    <Divider />
    {open && <JegotripPushStatusPane open refreshKey={deviceRefresh} />}
    <Divider />
    <JegotripDeviceIdentityPane open={open} disabled={pending} refreshKey={deviceRefresh} />
  </Stack>
}
