import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, FormControlLabel, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api } from '../../api/current'
import { groupManagementBody, GROUP_MANAGEMENT_SCOPE } from '../../policies/groupManagement'
import { submitGroupManagementOnce } from '../../policies/groupManagementFlow'
import { readRelationOutcome, RELATION_WRITE_UNRESOLVED, type RelationAttemptResult } from '../../policies/relationWriteFlow'
import { relationSnapshotClear } from '../../policies/relationWrite'
import { loadRelationMarker, relationStorageKey } from '../../policies/relationWriteStorage'
interface Props { lineId: string; revision: number; hosted: boolean; installed: boolean }
export default function AppGroupManagementPane(props: Props) { return <Editor key={JSON.stringify([props.lineId, props.revision, props.hosted, props.installed])} {...props} /> }
function Editor({ lineId, revision, hosted, installed }: Props) {
  const empty = { group: '', operation: 'set_properties', properties: '[]', nonce: '', consent: false, writeConsent: false, destructive: false }
  const [form, setForm] = useState(empty); const [result, setResult] = useState<RelationAttemptResult | null>(null); const [fault, setFault] = useState(false); const [busy, setBusy] = useState(false)
  const current = useRef({ live: false, generation: 0 }); const operating = useRef(false)
  useEffect(() => { const generation = current.current.generation + 1; current.current = { live: true, generation }; const changed = (e: StorageEvent) => { if (e.key === relationStorageKey(lineId) || e.key === null) { setResult(null); setFault(true) } }; window.addEventListener('storage', changed); return () => { window.removeEventListener('storage', changed); if (current.current.generation === generation) current.current.live = false } }, [lineId])
  const supported = window.isSecureContext && typeof navigator.locks?.request === 'function'
  const ready = result?.status === 'observed_terminal' && !!result.snapshot && relationSnapshotClear(result.snapshot) && result.snapshot.intents.length < 128 && result.snapshot.writes.length < 128 && !fault
  const body = groupManagementBody(revision, form.group, form.operation, form.nonce, form.properties, form.consent, form.writeConsent, form.destructive)
  const run = async (write: boolean) => {
    if (operating.current || !current.current.live || !supported || !installed || write && (!hosted || !ready || !body)) return
    if (write && !window.confirm(form.operation === 'disband' ? '明确解散此原群组？这是不可自动撤回的业务 WRITE；未知结果不换 nonce 重放。' : '明确修改此原群组属性？这是真实 WRITE；未知结果不换 nonce 重放。')) return
    const generation = current.current.generation; const active = () => current.current.live && current.current.generation === generation; const captured = body
    operating.current = true; setBusy(true); setFault(false); setResult(null); if (write) setForm(empty)
    try {
      const read = async () => { const [a, b] = await Promise.all([api.getAppRelationIntents(lineId), api.getAppRelationHistory(lineId)]); return { intents: a.data, writes: b.data } }
      const outcome = await navigator.locks.request(`simadmin-relation-write-v1:${lineId}`, { mode: 'exclusive', ifAvailable: true }, async (lock) => {
        if (!lock || !active()) return { status: 'blocked' as const }
        if (write && captured) return submitGroupManagementOnce({ storage: window.localStorage, current: active, read, post: async (b) => api.writeAppGroupManagement(lineId, b) }, lineId, captured)
        return readRelationOutcome({ storage: window.localStorage, current: active, read, post: () => Promise.reject(new Error('Read-only history')) }, lineId)
      })
      if (active()) setResult(outcome)
    } catch { if (active()) setFault(true) } finally { operating.current = false; if (active()) setBusy(false) }
  }
  const newNonce = () => { if (!ready || busy || !hosted || !supported) return; try { if (loadRelationMarker(window.localStorage, lineId) !== null) throw new Error('Unresolved'); setForm({ ...form, nonce: crypto.randomUUID().replaceAll('-', ''), consent: false, writeConsent: false, destructive: false }) } catch { setFault(true); setResult(null) } }
  if (!installed) return null
  const disabled = busy || !hosted || !supported
  return <Stack spacing={1} sx={{ mt: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原群组管理（属性 / 解散 WRITE）</Typography><Typography variant="caption">{GROUP_MANAGEMENT_SCOPE}</Typography>
    {!supported && <Alert severity="warning">缺少安全存储或同源排他锁时不开放写入，不降级提交。</Alert>}
    <Button disabled={busy || !supported} onClick={() => { void run(false) }}>核对所有关系准备与结果（只读）</Button>
    {(fault || result?.status === 'blocked' || result?.status === 'unresolved') && <Alert severity="warning">{RELATION_WRITE_UNRESOLVED}</Alert>}
    {ready && <Typography variant="caption">记录核对完成；仍需当前 SDK、未使用 nonce 和本次明确授权。</Typography>}
    <TextField select size="small" label="群组管理操作" slotProps={{ select: { SelectDisplayProps: { 'aria-label': '群组管理操作', 'aria-labelledby': undefined } } }} value={form.operation} disabled={disabled} onChange={(e) => setForm({ ...form, operation: e.target.value, properties: '[]', consent: false, writeConsent: false, destructive: false })}><MenuItem value="set_properties">修改群组属性</MenuItem><MenuItem value="disband">解散群组</MenuItem></TextField>
    <TextField size="small" label="群组管理原 GroupID" value={form.group} disabled={disabled} autoComplete="off" onChange={(e) => setForm({ ...form, group: e.target.value, consent: false, writeConsent: false, destructive: false })} />
    {form.operation === 'set_properties' && <TextField size="small" multiline minRows={2} label="群组属性 JSON 数组" value={form.properties} disabled={disabled} helperText={'如 [{"key":"Name","value":"名称"}]。Type/UpdateMethod/Permission/PropertyControl 用原数字枚举；其余值必须是字符串。'} onChange={(e) => setForm({ ...form, properties: e.target.value, consent: false, writeConsent: false })} />}
    <TextField size="small" label="群组管理本次 nonce" value={form.nonce} disabled={disabled || !ready} autoComplete="off" onChange={(e) => setForm({ ...form, nonce: e.target.value, consent: false, writeConsent: false, destructive: false })} /><Button disabled={disabled || !ready} onClick={newNonce}>为群组管理新意图生成 nonce（不提交）</Button>
    <FormControlLabel control={<Checkbox checked={form.consent} disabled={disabled || !ready} onChange={(e) => setForm({ ...form, consent: e.target.checked })} />} label="确认本次原群组和不可变内容" />
    <FormControlLabel control={<Checkbox checked={form.writeConsent} disabled={disabled || !ready} onChange={(e) => setForm({ ...form, writeConsent: e.target.checked })} />} label="授权本次群组管理 WRITE，不重放未知结果" />
    {form.operation === 'disband' && <FormControlLabel control={<Checkbox checked={form.destructive} disabled={disabled || !ready} onChange={(e) => setForm({ ...form, destructive: e.target.checked })} />} label="再次明确授权解散整个群组" />}
    <Button disabled={disabled || !ready || !body} onClick={() => { void run(true) }}>执行本次群组管理 WRITE</Button>
    {result?.snapshot?.writes.filter((r) => r.management).slice(0, 16).map((r) => <Typography key={r.id} variant="caption">群组操作 #{r.id} · {r.management?.operation} · 属性 {r.management?.properties} · {r.state}{r.baseTime === null ? ' · 原结果无时间字段' : ` · BaseTime ${r.baseTime} · UpdateTime ${r.updateTime}`}</Typography>)}
    <Typography variant="caption">与成员修改共用同源未决标记。HTTP 失败、页面重载或配置切换不能清除 Unknown-WRITE；没有自动重试或强制清除。</Typography>
  </Stack>
}
