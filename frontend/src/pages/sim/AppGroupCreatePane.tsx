import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, FormControlLabel, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api } from '../../api/current'
import { groupCreateBody, GROUP_CREATE_SCOPE } from '../../policies/groupCreate'
import { submitGroupCreateOnce, submitOwnerOnlyOnce, type CreateAttemptResult } from '../../policies/groupCreateFlow'
import { ownerOnlyBody } from '../../policies/groupOwnerOnly'
import { relationSnapshotClear } from '../../policies/relationWrite'
import { readRelationOutcome, RELATION_WRITE_UNRESOLVED } from '../../policies/relationWriteFlow'
import { loadRelationMarker, relationStorageKey } from '../../policies/relationWriteStorage'
interface Props { lineId: string; revision: number; hosted: boolean; installed: boolean; readInstalled: boolean; ownerOnlyInstalled?: boolean }
export default function AppGroupCreatePane(props: Props) { return <Mode key={JSON.stringify([props.lineId, props.revision, props.hosted, props.installed, props.readInstalled, props.ownerOnlyInstalled])} {...props} /> }
function Mode(props: Props) {
  const [ownerOnly, setMode] = useState(false)
  if (!props.installed) return null
  return <Stack>{props.ownerOnlyInstalled === true && <FormControlLabel control={<Checkbox checked={ownerOnly} disabled={!props.hosted} onChange={(event) => setMode(event.target.checked)} />} label="仅以当前会话 owner 建群（无其他初始成员）" />}<Editor key={ownerOnly ? 'session' : 'explicit'} {...props} ownerOnly={ownerOnly} /></Stack>
}
function Editor({ lineId, revision, hosted, installed, readInstalled, ownerOnly }: Props & { ownerOnly: boolean }) {
  const empty = { name: '', groupType: 1, owner: '', members: '[]', properties: '[]', nonce: '', consent: false, writeConsent: false, ownerConsent: false, readConsent: false }
  const [form, setForm] = useState(empty); const [result, setResult] = useState<CreateAttemptResult | null>(null); const [fault, setFault] = useState(false); const [busy, setBusy] = useState(false)
  const current = useRef({ generation: 0, live: false }); const operating = useRef(false)
  useEffect(() => { const generation = current.current.generation + 1; current.current = { generation, live: true }; const changed = (e: StorageEvent) => { if (e.key === relationStorageKey(lineId) || e.key === null) { setFault(true); setResult(null) } }; window.addEventListener('storage', changed); return () => { window.removeEventListener('storage', changed); if (current.current.generation === generation) current.current.live = false } }, [lineId])
  const supported = window.isSecureContext && typeof navigator.locks?.request === 'function'
  const ready = result?.status === 'observed_terminal' && !!result.snapshot && relationSnapshotClear(result.snapshot) && result.snapshot.intents.length < 128 && result.snapshot.writes.length < 128 && !fault
  const body = ownerOnly ? ownerOnlyBody(revision, form.nonce, form.name, form.groupType, form.properties, form.consent, form.writeConsent, form.ownerConsent, readInstalled && form.readConsent) : groupCreateBody(revision, form.nonce, form.name, form.groupType, form.owner, form.members, form.properties, form.consent, form.writeConsent, readInstalled && form.readConsent)
  const run = async (write: boolean) => {
    if (operating.current || !current.current.live || !installed || !supported || write && (!hosted || !ready || !body)) return
    if (write && !window.confirm(ownerOnly ? '只用当前已认证会话的 owner 建群？这会提交一次真实创建 WRITE，不邀请其他成员。' : '明确创建本次原群组？这是真实创建 WRITE。owner UID 会核对当前 SDK；属性后读不能确认未知创建，也不会换 nonce 重放。')) return
    const generation = current.current.generation; const active = () => current.current.live && current.current.generation === generation; const captured = body
    operating.current = true; setBusy(true); setResult(null); setFault(false); if (write) setForm(empty)
    try {
      const read = async () => { const [a, b] = await Promise.all([api.getAppRelationIntents(lineId), api.getAppRelationHistory(lineId)]); return { intents: a.data, writes: b.data } }
      const value = await navigator.locks.request(`simadmin-relation-write-v1:${lineId}`, { mode: 'exclusive', ifAvailable: true }, async (lock) => {
        if (!lock || !active()) return { status: 'blocked' as const }
        if (write && captured && 'session_owner_consent' in captured) return submitOwnerOnlyOnce({ storage: window.localStorage, current: active, read, post: async (request) => (await api.createOwnerOnlyGroup(lineId, request)).data }, lineId, captured)
        if (write && captured) return submitGroupCreateOnce({ storage: window.localStorage, current: active, read, post: async (request) => (await api.createAppGroup(lineId, request)).data }, lineId, captured)
        return readRelationOutcome({ storage: window.localStorage, current: active, read, post: () => Promise.reject(new Error('Read only')) }, lineId)
      })
      if (active()) setResult(value)
    } catch { if (active()) setFault(true) } finally { operating.current = false; if (active()) setBusy(false) }
  }
  const newNonce = () => { if (!ready || busy || !hosted || !supported) return; try { if (loadRelationMarker(window.localStorage, lineId) !== null) throw new Error('Unresolved relation operation'); setForm({ ...form, nonce: crypto.randomUUID().replaceAll('-', ''), consent: false, writeConsent: false, ownerConsent: false, readConsent: false }) } catch { setFault(true); setResult(null) } }
  if (!installed) return null
  const disabled = busy || !hosted || !supported
  const change = (patch: Partial<typeof form>) => setForm({ ...form, ...patch, consent: false, writeConsent: false, ownerConsent: false, readConsent: false })
  return <Stack spacing={1} sx={{ mt: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">{ownerOnly ? '原群组创建（仅当前会话 owner）' : '原群组创建（显式 owner 成员 WRITE）'}</Typography><Typography variant="caption">{ownerOnly ? '不填写 UID 或用户名。服务端核对原 UserEntry 引用，并从同一已认证登录取唯一成员。不会查询电话、邀请成员或重放未知 WRITE。' : GROUP_CREATE_SCOPE}</Typography>
    {!supported && <Alert severity="warning">安全存储或同源排他锁不可用时不创建群组，不降级提交。</Alert>}
    <Button disabled={busy || !supported} onClick={() => { void run(false) }}>核对创建与所有关系未决记录（只读）</Button>
    {(fault || result?.status === 'blocked' || result?.status === 'unresolved') && <Alert severity="warning">{RELATION_WRITE_UNRESOLVED}</Alert>}
    {ready && <Typography variant="caption">记录核对完成；新创建仍需当前 SDK、未使用 nonce 和明确授权。</Typography>}
    <TextField size="small" label="创建群组名称" value={form.name} disabled={disabled} autoComplete="off" onChange={(e) => change({ name: e.target.value })} />
    <TextField select size="small" label="创建群组原类型" slotProps={{ select: { SelectDisplayProps: { 'aria-label': '创建群组原类型', 'aria-labelledby': undefined } } }} value={form.groupType} disabled={disabled} onChange={(e) => change({ groupType: Number(e.target.value) })}>{['DiscussionGroup', 'Group', 'LargeGroup', 'Room', 'LargeRoom'].map((name, i) => <MenuItem key={name} value={i}>{i} {name}</MenuItem>)}</TextField>
    {!ownerOnly && <TextField size="small" label="创建预期 owner 原 UID" value={form.owner} disabled={disabled} autoComplete="off" helperText="只能匹配当前原 SDK 的 UID；不是用户名、电话或创建权限。服务端会在首次写出前核对。" onChange={(e) => change({ owner: e.target.value })} />}
    {!ownerOnly && <TextField size="small" multiline minRows={2} label="创建初始成员 JSON" value={form.members} disabled={disabled} helperText={'使用完整原成员记录，如 [{"label":"1_2","relation_type":257,"first":"owner名称","second":""}]；必须含预期 owner（类型 257），其他 label 仅 canonical UID。'} onChange={(e) => change({ members: e.target.value })} />}
    <TextField size="small" multiline minRows={2} label="创建额外属性 JSON" value={form.properties} disabled={disabled} helperText="Name/Type 由上方字段确定，不接受重复覆盖；其余按原属性映射。" onChange={(e) => change({ properties: e.target.value })} />
    <TextField size="small" label="创建本次 nonce" value={form.nonce} disabled={disabled || !ready} autoComplete="off" onChange={(e) => change({ nonce: e.target.value })} /><Button disabled={disabled || !ready} onClick={newNonce}>为群组创建新意图生成 nonce（不提交）</Button>
    <FormControlLabel control={<Checkbox checked={form.consent} disabled={disabled || !ready} onChange={(e) => setForm({ ...form, consent: e.target.checked })} />} label="确认创建名称、owner 和完整初始成员" />
    <FormControlLabel control={<Checkbox checked={form.writeConsent} disabled={disabled || !ready} onChange={(e) => setForm({ ...form, writeConsent: e.target.checked })} />} label="授权本次创建 WRITE，不重放未知结果" />
    {ownerOnly && <FormControlLabel control={<Checkbox checked={form.ownerConsent} disabled={disabled || !ready} onChange={(e) => setForm({ ...form, ownerConsent: e.target.checked })} />} label="授权使用当前登录身份生成唯一 owner" />}
    <FormControlLabel control={<Checkbox checked={form.readConsent} disabled={disabled || !ready || !readInstalled} onChange={(e) => setForm({ ...form, readConsent: e.target.checked })} />} label="独立授权本次已确认创建后的属性 READ" />
    <Button disabled={disabled || !ready || !body} onClick={() => { void run(true) }}>{ownerOnly ? '执行当前会话 owner 单成员建群' : '执行本次显式成员群组创建'}</Button>
    {result?.create && <Alert severity="info">创建记录 #{result.create.write.id} · {result.create.write.state}；独立属性 READ：{result.create.read.state}{result.create.read.properties === null ? ' · 无可核实属性数量' : ` · 属性 ${result.create.read.properties}`}。后读失败不撤销已确认创建。</Alert>}
    {result?.snapshot?.writes.filter((r) => r.management?.operation === (ownerOnly ? 'create_owner_only' : 'create')).slice(0, 16).map((r) => <Typography key={r.id} variant="caption">创建历史 #{r.id} · {r.state}（不含后读授权）</Typography>)}
    <Typography variant="caption">创建的 GroupID 留在当前私有结果链中，不从历史、事件或界面导出。记录中的 additions/updates/removals 是成员修改计数，不代表创建初始成员数量。其他成员的电话或 AID 归一化仍待独立实现。</Typography>
  </Stack>
}
