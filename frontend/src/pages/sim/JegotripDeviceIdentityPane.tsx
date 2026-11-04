import { useEffect, useRef, useState } from 'react'
import { Alert, Button, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api, type JegotripAccountView, type JegotripDeviceIdentityState } from '../../api/current'
import { appLoginError, validJegotripDeviceId, validPhoneAppUuid } from '../../policies/appLogin'

export default function JegotripDeviceIdentityPane({ open, disabled, refreshKey }: { open: boolean; disabled: boolean; refreshKey: number }) {
  const [view, setView] = useState<JegotripDeviceIdentityState | null>(null)
  const [accounts, setAccounts] = useState<JegotripAccountView[]>([])
  const [selected, setSelected] = useState('')
  const [accountId, setAccountId] = useState('')
  const [mode, setMode] = useState<'auto' | 'manual'>('auto')
  const [phoneImport, setPhoneImport] = useState(false)
  const [value, setValue] = useState('')
  const [busy, setBusy] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [refresh, setRefresh] = useState(0)
  const generation = useRef({ active: false })
  const action = useRef(false)
  const row = view?.identities.find((identity) => identity.id === selected)
  const locked = busy || disabled || !!row?.in_use
  const validManual = phoneImport ? validPhoneAppUuid(value) : validJegotripDeviceId(value)

  useEffect(() => {
    const scope = { active: open }
    generation.current = scope
    setView(null); setAccounts([]); setSelected(''); setAccountId(''); setValue(''); setMode('auto'); setPhoneImport(false); setError(null); setNote(null)
    setBusy(open)
    if (open) void Promise.all([api.getJegotripDeviceIdentities(), api.getJegotripAccounts()]).then(([identities, summaries]) => {
      if (scope.active) { setView(identities.data ?? null); setAccounts(summaries.data ?? []) }
    }).catch((failure: unknown) => { if (scope.active) setError(appLoginError(failure)) })
      .finally(() => { if (scope.active) setBusy(false) })
    return () => { scope.active = false }
  }, [open, refresh, refreshKey])

  const choose = (id: string) => {
    const selectedRow = view?.identities.find((identity) => identity.id === id)
    setSelected(id); setAccountId(selectedRow?.bound_account_id ?? ''); setValue(selectedRow?.device_id ?? '')
    setMode(selectedRow ? 'manual' : 'auto'); setPhoneImport(false); setNote(null); setError(null)
  }
  const save = async () => {
    if (action.current || locked || !view?.initialized || (selected && !row) || (mode === 'manual' && !validManual)) return
    if (row && !window.confirm('更换该账号的设备标识？旧值仍保留给原身份，不会借给其他号码。新值仅用于下次 App 登录，不代表已授信，也不会自动开关托管。')) return
    const scope = generation.current
    action.current = true; setBusy(true); setNote(null); setError(null)
    try {
      const response = await api.saveJegotripDeviceIdentity({ id: row?.id ?? null, expected_revision: row?.revision ?? 0,
        account_id: accountId || null, device_id: mode === 'manual' ? value.trim() : null })
      if (!response.data) throw new Error('missing identity state')
      if (scope.active) {
        setView(response.data); setNote(row ? '设备标识已更新，下次登录使用；服务端授信仍须另行验证。' : '设备标识已保存。已绑定账号下次自动复用；未分配的值可用于首次登录的手工标识。')
        if (!row) { setSelected(''); setAccountId(''); setValue(''); setMode('auto'); setPhoneImport(false) }
        else setValue(response.data.identities.find((identity) => identity.id === row.id)?.device_id ?? '')
      }
    } catch (failure) {
      // A lost response may hide a completed write. Never auto-regenerate/retry;
      // retain the input and ask for a fresh read before another CAS operation.
      if (scope.active) setError(appLoginError(failure))
    } finally { action.current = false; if (scope.active) setBusy(false) }
  }
  const remove = async () => {
    if (action.current || locked || !row || !window.confirm('弃用这个设备标识？历史绑定不会删除，旧值不会分配给其他号码。下次自动登录会生成新值，可能需要重新完成服务端验证。此操作不关闭托管业务。')) return
    const scope = generation.current
    action.current = true; setBusy(true); setNote(null); setError(null)
    try {
      const response = await api.deleteJegotripDeviceIdentity(row.id, row.revision)
      if (!response.data) throw new Error('missing identity state')
      if (scope.active) { setView(response.data); choose(''); setNote('已弃用，历史所有权仍保留。') }
    } catch (failure) { if (scope.active) setError(appLoginError(failure)) }
    finally { action.current = false; if (scope.active) setBusy(false) }
  }
  const source = { generated: '随机生成', manual: '手工指定', legacy: '保留原安装值' }
  return <Stack spacing={1.5}>
    <Stack direction="row" justifyContent="space-between" alignItems="center">
      <Typography variant="subtitle2">设备标识管理（独立数据库）</Typography>
      <Button size="small" disabled={busy || action.current} onClick={() => setRefresh((current) => current + 1)}>刷新</Button>
    </Stack>
    <Alert severity="info">默认首次生成随机 UUID，此后按认证后的账号/号码持久复用；不同号码不共用标识。同一号码用手机号或邮箱登录时会归并绑定。生成或更换标识不代表绕过一键验证。</Alert>
    <Button variant="outlined" disabled={locked} onClick={() => { setMode('manual'); setPhoneImport(true); setValue(''); setNote(null); setError(null) }}>录入手机 App 授信 UUID</Button>
    {phoneImport && <Alert severity="info">先选择已有号码的标识再替换，或为首次登录新增。复制本人 App 的 phone_uuid / device.dat 原样 UUID，不是 IMEI、MD5、enrollId 或 SDK 标识。保存后用同一账号选择「只查询设备授信」核验；不会在数据库写“已授信”或修改业务。</Alert>}
    {view?.configured_mode === 'static' && <Alert severity="warning">当前安装配置是 static 固定值模式。要为新账号默认随机生成，可将 http.device_identity_mode 改为 store；已有号码的标识仍保留，禁止跨号码复用固定值。</Alert>}
    {view && <Typography variant="caption" color="text.secondary">当前 {view.identities.length}/{view.max_identities} 个有效标识；仅显示标识、来源与脱敏绑定，不保存账号密码。</Typography>}
    {view?.identities.map((identity) => <Stack key={identity.id} direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems={{ sm: 'center' }}>
      <Stack sx={{ flex: 1, minWidth: 0 }}>
        <Typography variant="body2" sx={{ fontFamily: 'monospace', overflowWrap: 'anywhere' }}>{identity.device_id}</Typography>
        <Typography variant="caption" color="text.secondary">{source[identity.source]} · 版本 {identity.revision} · {identity.masked_number ?? (identity.reserved ? '登录身份已保留，等待号码确认' : '未分配')} {identity.in_use ? '· 使用/清理中' : ''}</Typography>
      </Stack>
      <Button size="small" disabled={busy || disabled || identity.in_use} onClick={() => choose(identity.id)}>设置</Button>
    </Stack>)}
    <TextField select size="small" label="设置目标" value={selected} disabled={busy || disabled} onChange={(event) => choose(event.target.value)}>
      <MenuItem value="">新增标识</MenuItem>
      {view?.identities.map((identity) => <MenuItem key={identity.id} value={identity.id}>{identity.masked_number ?? '待认证'} · {identity.device_id}</MenuItem>)}
    </TextField>
    <TextField select size="small" label="绑定已有账号（可选）" value={accountId} disabled={locked || !!row?.bound_account_id} onChange={(event) => setAccountId(event.target.value)}>
      <MenuItem value="">保留原绑定 / 首次登录后确认号码</MenuItem>
      {accounts.map((account) => <MenuItem key={account.account_id} value={account.account_id}
        disabled={view?.identities.some((identity) => identity.bound_account_id === account.account_id && identity.id !== selected)}>
        {account.masked_login_name} · {account.masked_number}
      </MenuItem>)}
    </TextField>
    <TextField select size="small" label="标识生成方式" value={mode} disabled={locked} onChange={(event) => { setMode(event.target.value === 'manual' ? 'manual' : 'auto'); setPhoneImport(false) }}>
      <MenuItem value="auto">随机生成 UUID（默认）</MenuItem><MenuItem value="manual">手工指定自己的设备标识</MenuItem>
    </TextField>
    {mode === 'manual' && <TextField size="small" label={phoneImport ? '待保存的手机 App UUID' : '设备标识'} value={value} disabled={locked} onChange={(event) => setValue(event.target.value)}
      error={value.length > 0 && !validManual}
      helperText={phoneImport ? '36 位 UUID，保留连字符和原值。只用本人的值；既有标识替换需确认且保留原所有权。' : '8–64 位，仅字母、数字、下划线与连字符。不要使用他人的标识。'} inputProps={{ maxLength: 64 }} />}
    <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1}>
      <Button variant="outlined" disabled={locked || !view?.initialized || (mode === 'manual' && !validManual)}
        onClick={() => { void save() }}>{row ? '保存新标识' : mode === 'auto' ? '生成并保存随机标识' : '保存手工标识'}</Button>
      {row && <Button color="warning" disabled={locked} onClick={() => { void remove() }}>弃用标识</Button>}
    </Stack>
    {note && <Alert severity="info">{note}</Alert>}{error && <Alert severity="warning">{error} 如上次响应丢失，请先刷新核对，不要反复生成。</Alert>}
  </Stack>
}
