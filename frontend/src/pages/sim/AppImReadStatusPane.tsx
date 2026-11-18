import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, TextField, Typography } from '@mui/material'
import { userReadRecipientAllowed } from '../../policies/userEntry'
import { api } from '../../api/current'
import { READ_STATUS_SCOPE, readStatusLabel, readStatusTargets, readStatusRows, readStatusHistory, readStatusBaselineReady, readStatusUnknownResolved, readStatusRecipientNoWriteError, type ReadStatusTarget, type ReadStatusRow, type ReadStatusUnknown } from '../../policies/imReadStatus'
export default function AppImReadStatusPane({ lineId, revision, installed, hosted, recipientInstalled = false }: { lineId: string; revision: number; installed: boolean; hosted: boolean; recipientInstalled?: boolean }) {
  const context = JSON.stringify([lineId, revision, installed, hosted, recipientInstalled])
  const [recipientInput, setRecipientInput] = useState({ context, value: '' })
  const recipient = recipientInput.context === context ? recipientInput.value : ''
  const [loaded, setLoaded] = useState<{ context: string; generation: number; targets: ReadStatusTarget[]; rows: ReadStatusRow[]; failed: boolean } | null>(null)
  const [pending, setPending] = useState<ReadStatusUnknown | null>(null)
  const [busy, setBusy] = useState(false); const writing = useRef(false); const current = useRef({ context, generation: 0, readSerial: 0, live: false })
  useEffect(() => {
    const generation = current.current.generation + 1; current.current = { context, generation, readSerial: 0, live: true }
    let active = true; let timer: ReturnType<typeof setTimeout> | undefined
    const load = async () => {
      if (!installed || !Number.isSafeInteger(revision) || revision <= 0) return
      if (writing.current) { if (active) timer = setTimeout(() => { void load() }, 5000); return }
      const serial = ++current.current.readSerial
      try {
        const [targets, history] = await Promise.all([hosted ? api.getAppImReadStatusTargets(lineId, revision) : Promise.resolve({ data: [] }), api.getAppImReadStatusHistory(lineId)])
        const rows = readStatusHistory(history.data)
        if (!rows) throw new Error('Original read status history is incomplete')
        if (active && current.current.generation === generation && current.current.readSerial === serial) {
          const snapshot = { context, generation, rows }
          setLoaded({ ...snapshot, targets: readStatusTargets(targets.data), failed: false })
          setPending((old) => old && readStatusUnknownResolved(old, snapshot, context, generation) ? null : old)
        }
      } catch { if (active && current.current.generation === generation && current.current.readSerial === serial) setLoaded({ context, generation, targets: [], rows: [], failed: true }) }
      if (active) timer = setTimeout(() => { void load() }, 5000)
    }
    void load(); return () => { active = false; if (timer) clearTimeout(timer); if (current.current.generation === generation) current.current.live = false }
  }, [context, lineId, revision, installed, hosted])
  const selected = readStatusBaselineReady(loaded, context, current.current.generation) ? loaded : null
  const unknown = pending?.context === context || selected?.rows.some((row) => row.state === 'uncertain') === true
  const setPosition = async (target: ReadStatusTarget, alias?: string) => {
    if (alias !== undefined && (!recipientInstalled || target.kind !== 'p2p' || !userReadRecipientAllowed(alias))) return
    if (writing.current || unknown || !current.current.live || current.current.context !== context || !installed || !hosted || !selected || selected.failed || !selected.targets.some((row) => row.id === target.id && row.index === target.index && row.kind === target.kind) || !Number.isSafeInteger(revision) || revision <= 0) return
    if (!window.confirm(alias === undefined ? '手动设置这个原连续消费位置？这是接收位置，不是用户已读、消息通知回执或授信；不清空未知写入，不自动重放。' : '通过原收件人解析设置当前连续读位置？原 private UID 必须匹配这个 P2P 目标；这是接收位置，不是用户已读或授信，不清空未知写入、不自动重放。')) return
    const original = { ...current.current }; const after = Math.max(0, ...selected.rows.map((row) => row.id)); const nonce = crypto.randomUUID().replaceAll('-', '')
    writing.current = true; current.current.readSerial += 1; setBusy(true)
    try {
      const response = alias === undefined ? await api.setAppImReadStatus(lineId, target.id, revision, target.index, nonce) : await api.setAppImReadStatusViaRecipient(lineId, target.id, revision, target.index, nonce, alias)
      const row = readStatusRows([response.data])[0]
      if (!row || row.targetId !== target.id || row.index !== target.index || row.id <= after) throw new Error('Original read status unconfirmed')
      if (!current.current.live || current.current.generation !== original.generation || current.current.context !== original.context) return
      setLoaded((old) => old?.context === context && old.generation === original.generation ? { ...old, rows: [row, ...old.rows.filter((r) => r.id !== row.id)].slice(0, 128) } : old)
      if (row.state === 'uncertain') setPending({ context, targetId: target.id, index: target.index, after })
    } catch (error) {
      if (current.current.live && current.current.generation === original.generation && current.current.context === original.context) {
        if (alias !== undefined && readStatusRecipientNoWriteError(error)) setLoaded((old) => old?.context === context && old.generation === original.generation ? { ...old, targets: [], failed: true } : old)
        else setPending({ context, targetId: target.id, index: target.index, after })
      }
    }
    finally { writing.current = false; if (current.current.live) setBusy(false) }
  }
  if (!installed) return null
  return <Stack spacing={1} sx={{ mt: 1, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原群组/点对点手动读位置</Typography>
    <Typography variant="caption" color="text.secondary">{READ_STATUS_SCOPE}</Typography>
    {unknown && <Alert severity="warning">原手动读位置写入未核实；仅查询历史，不重放。</Alert>}
    {selected?.failed && <Alert severity="warning">当前原目标或历史未核实；不生成新的写入。</Alert>}
    {recipientInstalled && <TextField size="small" label="手动读位置收件人原 URI（可选）" value={recipient} autoComplete="off" slotProps={{ htmlInput: { maxLength: 512 } }} disabled={busy || !hosted} onChange={(event) => setRecipientInput({ context, value: event.target.value })} />}
    {selected?.targets.map((target) => <Stack key={target.id} spacing={0.5}>
      <Typography variant="caption">{target.kind === 'org' ? '群组' : '点对点'} · 原连续消费位置 {target.index}</Typography>
      <Button size="small" disabled={busy || unknown || !hosted || selected.failed} onClick={() => { void setPosition(target) }}>明确设置原读位置</Button>
      {recipientInstalled && target.kind === 'p2p' && <Button size="small" disabled={busy || unknown || !hosted || selected.failed || !userReadRecipientAllowed(recipient)} onClick={() => { void setPosition(target, recipient) }}>通过原收件人设置读位置</Button>}
    </Stack>)}
    {selected?.rows.slice(0, 16).map((row) => <Typography key={row.id} variant="caption">位置 {row.index} · {readStatusLabel(row.state)}</Typography>)}
  </Stack>
}
