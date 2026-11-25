import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, FormControlLabel, Stack, TextField, Typography } from '@mui/material'
import { api } from '../../api/current'
import { relationRosterRows, rosterReadBody, rosterReadOutcome, ROSTER_SCOPE, ROSTER_UNKNOWN, type RelationRosterRow, type RosterReadOutcome } from '../../policies/relationRoster'
interface Props { lineId: string; revision: number; hosted: boolean; installed: boolean }
export default function AppRelationRosterPane(props: Props) { return <RosterPane key={JSON.stringify([props.lineId, props.revision, props.hosted, props.installed])} {...props} /> }
function RosterPane({ lineId, revision, hosted, installed }: Props) {
  const [group, setGroup] = useState(''); const [since, setSince] = useState('-1'); const [consent, setConsent] = useState(false)
  const [rows, setRows] = useState<RelationRosterRow[] | null>(null); const [result, setResult] = useState<RosterReadOutcome | null>(null); const [error, setError] = useState(false); const [busy, setBusy] = useState(false)
  const current = useRef({ generation: 0, live: false }); const reading = useRef(false)
  useEffect(() => { const generation = current.current.generation + 1; current.current = { generation, live: true }; return () => { if (current.current.generation === generation) current.current.live = false } }, [])
  const body = rosterReadBody(revision, group, since, consent)
  const read = async (remote: boolean) => {
    if (reading.current || !current.current.live || remote && (!hosted || !installed || !body)) return
    const captured = body ? { ...body } : null
    if (remote && !window.confirm('仅向当前原 SDK 发起目录 READ，并保存本地名册；不修改关系，不解决 Unknown-WRITE。继续？')) return
    const generation = current.current.generation; const active = () => current.current.live && current.current.generation === generation
    reading.current = true; setBusy(true); setError(false); setResult(null); setRows(null)
    try {
      if (remote && captured) { const response = await api.readAppRelationRoster(lineId, captured); const value = rosterReadOutcome(response.data); if (!value) throw new Error('Invalid roster read outcome'); if (active()) setResult(value) }
      else { const response = await api.getAppRelationRosters(lineId); const value = relationRosterRows(response.data); if (!value) throw new Error('Invalid roster history'); if (active()) setRows(value) }
    } catch { if (active()) setError(true) }
    finally { if (active()) { reading.current = false; setBusy(false) } }
  }
  return <Stack spacing={0.5} sx={{ mt: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原关系名册基线与缺口恢复</Typography>
    <Typography variant="caption" color="text.secondary">{ROSTER_SCOPE}</Typography>
    <Button size="small" disabled={busy} onClick={() => { void read(false) }}>明确读取本地名册历史（只读）</Button>
    {installed && <>
      <TextField size="small" label="名册原 GroupID（留空仅当前 Buddy）" value={group} disabled={busy} onChange={(e) => setGroup(e.target.value)} />
      <TextField size="small" label="名册原 since（-1 请求完整基线）" value={since} disabled={busy} onChange={(e) => setSince(e.target.value)} />
      <FormControlLabel control={<Checkbox checked={consent} disabled={busy} onChange={(e) => setConsent(e.target.checked)} />} label="我明确授权本次原目录 READ，不是关系 WRITE" />
      <Button size="small" disabled={busy || !hosted || !body} onClick={() => { void read(true) }}>明确执行原名册 READ（不修改关系）</Button>
    </>}
    {error && <Alert severity="warning">名册读取未核实；不显示私有响应，不自动重试或清除缺口/未知 WRITE。</Alert>}
    {rows?.length === 0 && <Typography variant="caption">本次未返回名册历史；不是空名册或先前写入未发生的证明。</Typography>}
    {rows?.map((r) => <Typography variant="caption" key={r.id}>历史名册 #{r.id} · {r.targetKind} · 成员 {r.members} · 基线 {r.baseline ?? '未建立'} · 已观测 {r.observed ?? '未建立'} · 目标 {r.target ?? '未知'} · 历史完整 {String(r.historicalComplete)} · gap {String(r.gap)} · 必需完整读取 {String(r.fullRequired)}</Typography>)}
    {result && <Alert severity={result.completeRoster ? 'info' : 'warning'}>本次原 READ：{result.outcome} · 快照完整 {String(result.completeRoster)} · 成员 {result.members} · 原时间 {result.originalTime ?? '未提供'}；只表示此原读取的本地观测，不代表当前在线或 WRITE 已确认。</Alert>}
    <Typography variant="caption" color="text.secondary">{ROSTER_UNKNOWN}</Typography>
  </Stack>
}
