import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Checkbox, FormControlLabel, Stack, TextField, Typography } from '@mui/material'
import { api } from '../../api/current'
import { BUDDY_CHECK_SCOPE, BUDDY_CHECK_UNKNOWN, buddyCheckBody, buddyCheckResult, type BuddyCheckResult } from '../../policies/buddyRelationCheck'
interface Props { lineId: string; revision: number; hosted: boolean; installed: boolean }
export default function AppBuddyRelationCheckPane(props: Props) { return <CheckPane key={JSON.stringify([props.lineId, props.revision, props.hosted, props.installed])} {...props} /> }
function CheckPane({ lineId, revision, hosted, installed }: Props) {
  const [uid, setUid] = useState(''); const [consent, setConsent] = useState(false); const [result, setResult] = useState<BuddyCheckResult | null>(null)
  const [error, setError] = useState(false); const [busy, setBusy] = useState(false); const reading = useRef(false); const current = useRef({ generation: 0, live: false })
  useEffect(() => { const generation = current.current.generation + 1; current.current = { generation, live: true }; return () => { if (current.current.generation === generation) current.current.live = false } }, [])
  const body = buddyCheckBody(revision, uid, consent)
  const read = async () => {
    if (reading.current || !current.current.live || !installed || !hosted || !body) return
    const captured = { ...body }
    if (!window.confirm('向当前原 SDK 检查此 UID 的关系类型？只读，不修改关系或解决 Unknown-WRITE，不会重新登录。')) return
    const generation = current.current.generation; const active = () => current.current.live && current.current.generation === generation
    reading.current = true; setBusy(true); setError(false); setResult(null)
    try { const response = await api.checkAppBuddyRelation(lineId, captured); const value = buddyCheckResult(response.data); if (!value) throw new Error('Invalid check projection'); if (active()) setResult(value) }
    catch { if (active()) setError(true) }
    finally { if (active()) { reading.current = false; setBusy(false) } }
  }
  if (!installed) return null
  return <Stack spacing={1} sx={{ mt: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原 Buddy 关系类型检查（只读）</Typography>
    <Typography variant="caption" color="text.secondary">{BUDDY_CHECK_SCOPE}</Typography>
    <TextField size="small" label="关系检查目标原 UID" value={uid} disabled={busy || !hosted} autoComplete="off" slotProps={{ htmlInput: { maxLength: 512 } }} onChange={(e) => { setUid(e.target.value); setConsent(false); setResult(null); setError(false) }} />
    <FormControlLabel control={<Checkbox checked={consent} disabled={busy || !hosted} onChange={(e) => setConsent(e.target.checked)} />} label="我明确授权本次关系检查 READ" />
    <Button size="small" disabled={busy || !hosted || !body} onClick={() => { void read() }}>明确检查 Buddy 关系（只读）</Button>
    {!hosted && <Typography variant="caption">当前原 SDK 未托管；不会自动启动或重新登录。</Typography>}
    {error && <Alert severity="warning">关系检查未核实；不显示私有错误，不自动重试。</Alert>}
    {result && <Alert severity={result.accepted ? 'info' : 'warning'}>本次原检查：{result.accepted ? '返回成功' : '返回拒绝'} · 原 RelationType：{result.relationType}；仅原始类型 DATA，不推断权限或在线状态。</Alert>}
    <Typography variant="caption" color="text.secondary">{BUDDY_CHECK_UNKNOWN}</Typography>
  </Stack>
}
