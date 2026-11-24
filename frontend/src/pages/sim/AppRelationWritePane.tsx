import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, FormControlLabel, MenuItem, Stack, TextField, Typography } from '@mui/material'
import { api } from '../../api/current'
import { relationHistoryLabel } from '../../policies/relationHistory'
import { relationWriteBody, relationSnapshotClear, RELATION_WRITE_SCOPE, type RelationDraft } from '../../policies/relationWrite'
import { readRelationOutcome, submitRelationOnce, RELATION_WRITE_UNRESOLVED, type RelationAttemptResult, type RelationWritePorts } from '../../policies/relationWriteFlow'
import { loadRelationMarker, relationStorageKey } from '../../policies/relationWriteStorage'
type RelationWriteProps = { lineId: string; revision: number; hosted: boolean; installed: boolean }
export default function AppRelationWritePane(props: RelationWriteProps) {
  return <RelationEditor key={JSON.stringify([props.lineId, props.revision, props.hosted, props.installed])} {...props} />
}
function RelationEditor({ lineId, revision, hosted, installed }: RelationWriteProps) {
  const context = JSON.stringify([lineId, revision, hosted, installed])
  const empty = { context, target: 'buddy' as const, groupId: '', additions: '[]', updates: '[]', removals: '[]', nonce: '', consent: false, writeConsent: false }
  const [input, setInput] = useState<{ context: string; target: RelationDraft['target']; groupId: string; additions: string; updates: string; removals: string; nonce: string; consent: boolean; writeConsent: boolean }>(empty)
  const form = input.context === context ? input : empty
  const [result, setResult] = useState<{ context: string; value: RelationAttemptResult } | null>(null)
  const [fault, setFault] = useState<{ context: string } | null>(null); const [busy, setBusy] = useState(false)
  const current = useRef({ context, generation: 0, live: false }); const operating = useRef(false)
  useEffect(() => {
    const generation = current.current.generation + 1; current.current = { context, generation, live: true }
    const changed = (event: StorageEvent) => { if (event.key === relationStorageKey(lineId) || event.key === null) { setResult(null); setFault({ context }) } }
    window.addEventListener('storage', changed)
    return () => { window.removeEventListener('storage', changed); if (current.current.generation === generation) current.current.live = false }
  }, [context, lineId])
  const selected = result?.context === context ? result.value : null
  const ready = selected?.status === 'observed_terminal' && !!selected.snapshot && relationSnapshotClear(selected.snapshot) && selected.snapshot.intents.length < 128 && selected.snapshot.writes.length < 128 && fault?.context !== context
  const draft: RelationDraft = { target: form.target, groupId: form.groupId, additions: form.additions, updates: form.updates, removals: form.removals }
  const body = relationWriteBody(revision, form.nonce, draft, form.consent, form.writeConsent)
  const supported = typeof navigator.locks?.request === 'function' && window.isSecureContext
  const run = async (write: boolean) => {
    if (operating.current || !current.current.live || current.current.context !== context || !supported || !installed || write && (!ready || !hosted || !body)) return
    if (write && !window.confirm('明确修改本次 Group/Buddy 关系？这是真实关系 WRITE，可能产生业务影响；不是目录读取、群组创建或 SDK 授信。结果未知只查询，不换 nonce 重放。')) return
    const original = { ...current.current }; const capturedBody = body
    const active = () => current.current.live && current.current.context === original.context && current.current.generation === original.generation
    operating.current = true; setBusy(true); setFault(null); setResult(null)
    // Private draft and raw nonce stay in memory only and are wiped from the
    // editor before the first network await; only opaque marker DATA is stored.
    if (write) setInput(empty)
    try {
      const ports: RelationWritePorts = {
        storage: window.localStorage, current: active,
        read: async () => { const [intents, writes] = await Promise.all([api.getAppRelationIntents(lineId), api.getAppRelationHistory(lineId)]); return { intents: intents.data, writes: writes.data } },
        post: async (request) => api.updateAppRelations(lineId, request),
      }
      const value = await navigator.locks.request(`simadmin-relation-write-v1:${lineId}`, { mode: 'exclusive', ifAvailable: true }, async (lock) => {
        if (!lock || !active()) return { status: 'blocked' as const }
        return write && capturedBody ? submitRelationOnce(ports, lineId, capturedBody) : readRelationOutcome(ports, lineId)
      })
      if (active()) setResult({ context, value })
    } catch { if (active()) setFault({ context }) }
    finally { operating.current = false; if (current.current.live) setBusy(false) }
  }
  const newNonce = () => {
    if (!supported || !ready || busy || !hosted) return
    try { if (loadRelationMarker(window.localStorage, lineId) !== null) throw new Error('Unresolved original relation intent'); setInput({ ...form, nonce: crypto.randomUUID().replaceAll('-', ''), consent: false, writeConsent: false }) }
    catch { setFault({ context }); setResult(null) }
  }
  if (!installed) return null
  const disabled = busy || !hosted || !supported
  return <Stack spacing={1} sx={{ mt: 1, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原 Group/Buddy 关系修改（显式 WRITE）</Typography>
    <Typography variant="caption" color="text.secondary">{RELATION_WRITE_SCOPE}</Typography>
    {!supported && <Alert severity="warning">浏览器安全存储或同源排他锁不可用时不开放关系写入；不会降级为无锁提交。</Alert>}
    {!hosted && <Typography variant="caption">当前原 SDK 未托管；不自动启动、重新登录或另建连接。</Typography>}
    <Button size="small" disabled={busy || !supported} onClick={() => { void run(false) }}>明确核对准备与结果（只读）</Button>
    {!selected && <Typography variant="caption">先核对两套记录；字段存在或空列表不构成关系写权限。</Typography>}
    {(selected?.status === 'unresolved' || selected?.status === 'blocked' || fault?.context === context) && <Alert severity="warning">{RELATION_WRITE_UNRESOLVED}</Alert>}
    {selected?.status === 'observed_terminal' && <Typography variant="caption">本次记录核对完成；仍需新的明确授权、未使用 nonce 和当前原 SDK 检查，不代表关系在线或消息送达。</Typography>}
    <TextField select size="small" label="关系修改目标" slotProps={{ select: { SelectDisplayProps: { 'aria-label': '关系修改目标', 'aria-labelledby': undefined } } }} value={form.target} disabled={disabled} onChange={(event) => setInput({ ...form, target: event.target.value === 'group' ? 'group' : 'buddy', groupId: '', consent: false, writeConsent: false })}>
      <MenuItem value="buddy">Buddy（当前原 SDK 用户）</MenuItem><MenuItem value="group">Group（明确原群组 ID）</MenuItem>
    </TextField>
    {form.target === 'group' && <TextField size="small" label="本次原群组 ID" value={form.groupId} disabled={disabled} autoComplete="off" slotProps={{ htmlInput: { maxLength: 508 } }} helperText="仅原 GroupID；不是 P2P/Org Box、URI 或网络端点，不创建新群组。" onChange={(event) => setInput({ ...form, groupId: event.target.value, consent: false, writeConsent: false })} />}
    <Typography variant="caption" color="text.secondary">输入原关系变更 JSON 数组；类型为原 MTC 枚举，properties 的 key/value 为字节数组，不是权限。删除 flag 默认 false。三个集合不能重复同一 label。</Typography>
    {(['additions', 'updates', 'removals'] as const).map((field) => <TextField key={field} size="small" multiline minRows={2} label={{ additions: '新增关系 JSON', updates: '更新关系 JSON', removals: '删除关系 JSON' }[field]} value={form[field]} disabled={disabled} autoComplete="off" slotProps={{ htmlInput: { maxLength: 262144 } }} onChange={(event) => setInput({ ...form, [field]: event.target.value, consent: false, writeConsent: false })} />)}
    <TextField size="small" label="本次用户 nonce（32位小写十六进制）" value={form.nonce} disabled={disabled || !ready} autoComplete="off" slotProps={{ htmlInput: { maxLength: 32 } }} helperText="仅用于本次不可变请求；未决时不得复用或换 nonce 重放。不会保存原 nonce 或私有关系内容。" onChange={(event) => setInput({ ...form, nonce: event.target.value, consent: false, writeConsent: false })} />
    <Button size="small" disabled={disabled || !ready} onClick={newNonce}>为新意图生成 nonce（不提交）</Button>
    <FormControlLabel control={<Checkbox checked={form.consent} disabled={disabled || !ready} onChange={(event) => setInput({ ...form, consent: event.target.checked })} />} label="确认当前账号、原目标和本次不可变关系内容" />
    <FormControlLabel control={<Checkbox checked={form.writeConsent} disabled={disabled || !ready} onChange={(event) => setInput({ ...form, writeConsent: event.target.checked })} />} label="明确授权本次关系 WRITE，接受业务影响且不重放未知结果" />
    <Button size="small" disabled={disabled || !ready || !body} onClick={() => { void run(true) }}>明确执行本次关系修改</Button>
    {selected?.snapshot?.intents.slice(0, 16).map((row) => <Typography key={`intent-${row.id}`} variant="caption">准备 #{row.id} · {row.state === 'preparing' ? '准备未决（不授 WRITE）' : row.state === 'bound' ? '已关联原写入记录' : '准备未写出'} · 新增 {row.additions} / 更新 {row.updates} / 删除 {row.removals}</Typography>)}
    {selected?.snapshot?.writes.slice(0, 16).map((row) => <Typography key={`write-${row.id}`} variant="caption">结果 #{row.id} · {relationHistoryLabel(row.state)}{row.baseTime !== null ? ` · BaseTime ${row.baseTime} · UpdateTime ${row.updateTime}` : ''}</Typography>)}
    <Typography variant="caption" color="text.secondary">即使其他标签页、配置切换或重载，未决标记仍保留；没有强制清除、自动重试或重新登录按钮。</Typography>
  </Stack>
}
