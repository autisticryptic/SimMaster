import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, TextField, Typography } from '@mui/material'
import { api } from '../../api/current'
import { IM_HISTORY_SCOPE, imHistoryRows, type ImHistoryRow } from '../../policies/imHistory'
import { USER_READ_SCOPE, userReadRows, userReadLabel } from '../../policies/imUserRead'
import { userReadBaselineReady, userReadKnownUnknown, userReadUnknownResolved, type UserReadSnapshot, type UserReadUnknown } from '../../policies/imUserReadState'
import { userReadRecipientAllowed } from '../../policies/userEntry'
export default function AppImMessagesPane({ lineId, userRead }: { lineId: string; userRead?: { revision: number; installed: boolean; hosted: boolean; recipientInstalled?: boolean } }) {
  const [loaded, setLoaded] = useState<{ line: string; rows: ImHistoryRow[]; error: boolean } | null>(null)
  const [busy, setBusy] = useState(false)
  const [uncertain, setUncertain] = useState<{ line: string; id: number } | null>(null)
  const [userOutcomes, setUserOutcomes] = useState<UserReadSnapshot | null>(null)
  const [userUncertain, setUserUncertain] = useState<UserReadUnknown | null>(null)
  const userReadContext = JSON.stringify([lineId, userRead?.revision, userRead?.installed, userRead?.hosted, userRead?.recipientInstalled])
  const [recipientInput, setRecipientInput] = useState({ context: userReadContext, value: '' })
  const recipient = recipientInput.context === userReadContext ? recipientInput.value : ''
  const active = useRef({ line: lineId, generation: 0, readSerial: 0, live: false })
  const deleting = useRef(false)
  const marking = useRef(false)
  useEffect(() => {
    const generation = active.current.generation + 1
    active.current = { line: lineId, generation, readSerial: 0, live: true }
    let current = true; let timer: ReturnType<typeof setTimeout> | undefined
    const read = async () => {
      if (deleting.current || marking.current) { if (current) timer = setTimeout(() => { void read() }, 5000); return }
      const serial = ++active.current.readSerial
      try {
        const response = await api.getAppImMessages(lineId)
        if (current && active.current.generation === generation && active.current.readSerial === serial) setLoaded({ line: lineId, rows: imHistoryRows(response.data), error: false })
        if (userRead?.installed) {
          const results = await api.getAppImUserReads(lineId)
          if (current && active.current.generation === generation && active.current.readSerial === serial) {
            const snapshot: UserReadSnapshot = { line: lineId, generation, context: userReadContext, rows: userReadRows(results.data) }; setUserOutcomes(snapshot)
            setUserUncertain((old) => old && userReadUnknownResolved(old, snapshot, lineId, generation) ? null : old)
          }
        }
      } catch { if (current && active.current.generation === generation && active.current.readSerial === serial) setLoaded({ line: lineId, rows: [], error: true }) }
      if (current) timer = setTimeout(() => { void read() }, 5000)
    }
    void read()
    return () => { current = false; if (timer) clearTimeout(timer); if (active.current.generation === generation) active.current.live = false }
  }, [lineId, userRead?.installed, userReadContext])
  const remove = async (row: ImHistoryRow) => {
    if (deleting.current || marking.current || (uncertain?.line === lineId && uncertain.id === row.id) || !active.current.live) return
    const original = { ...active.current }
    if (!window.confirm('删除本地 IM 文本？仅保留私有去重回执，防止服务端重放恢复已删除内容。')) return
    deleting.current = true; active.current.readSerial += 1; setBusy(true)
    try {
      const deleted = await api.deleteAppImMessage(original.line, row.id)
      if (deleted.data !== true && deleted.data !== false) throw new Error('Unconfirmed local IM deletion')
      if (!active.current.live || active.current.generation !== original.generation) return
      const serial = ++active.current.readSerial
      const response = await api.getAppImMessages(original.line)
      if (active.current.live && active.current.generation === original.generation && active.current.readSerial === serial) setLoaded({ line: original.line, rows: imHistoryRows(response.data), error: false })
    } catch {
      if (active.current.live && active.current.generation === original.generation) setUncertain({ line: original.line, id: row.id })
    } finally { deleting.current = false; if (active.current.live) setBusy(false) }
  }
  const markRead = async (row: ImHistoryRow, alias?: string) => {
    if (alias !== undefined && (!userRead?.recipientInstalled || !userReadRecipientAllowed(alias))) return
    if (deleting.current || marking.current || !active.current.live || !userRead?.installed || !userRead.hosted || !Number.isSafeInteger(userRead.revision) || userRead.revision <= 0 || row.kind !== 'p2p' || row.index.startsWith('-') || !userReadBaselineReady(userOutcomes, lineId, active.current.generation) || userOutcomes.context !== userReadContext || userReadKnownUnknown(userOutcomes, lineId, active.current.generation, row.id, row.index) || userUncertain?.line === lineId && userUncertain.id === row.id) return
    if (!window.confirm(alias === undefined ? '确认已阅读这条原 IM 消息，并向原服务端标记已读？此操作与接收位置回执、渠道送达或通话/媒体状态无关。' : '通过原收件人解析标记这条原消息已读？解析 UID 必须匹配本条本地原目标，不清空未知写入、不自动重放或重新登录。')) return
    const original = { ...active.current }; const revision = userRead.revision
    const after = userOutcomes?.line === lineId ? Math.max(0, ...userOutcomes.rows.map((row) => row.id)) : 0
    const nonce = crypto.randomUUID().replaceAll('-', '')
    marking.current = true; active.current.readSerial += 1; setBusy(true)
    try {
      const response = alias === undefined ? await api.markAppImUserRead(original.line, row.id, revision, nonce) : await api.markAppImUserReadViaRecipient(original.line, row.id, revision, nonce, alias)
      const result = userReadRows([response.data])[0]
      if (!result || result.messageId !== row.id || result.index !== row.index) throw new Error('Unconfirmed original user-read result')
      if (!active.current.live || active.current.generation !== original.generation) return
      setUserOutcomes((old) => ({ line: original.line, generation: original.generation, context: userReadContext, rows: [result, ...(old?.line === original.line && old.context === userReadContext ? old.rows.filter((row) => row.id !== result.id) : [])].slice(0, 16) }))
      if (result.state === 'uncertain') setUserUncertain({ line: original.line, id: row.id, index: row.index, after })
    } catch {
      if (active.current.live && active.current.generation === original.generation) setUserUncertain({ line: original.line, id: row.id, index: row.index, after })
    } finally { marking.current = false; if (active.current.live) setBusy(false) }
  }
  const currentUserOutcomes = userOutcomes?.line === lineId && userOutcomes.context === userReadContext ? userOutcomes : null
  const rows = loaded?.line === lineId ? loaded.rows : []
  if (loaded?.line === lineId && loaded.error) return <Alert severity="warning">无法读取原 IM 本地记录；不会据此重发、重新登录或确认服务端已读。</Alert>
  if (!rows.length && uncertain?.line !== lineId && userUncertain?.line !== lineId) return null
  return <Stack spacing={1} sx={{ overflowWrap: 'anywhere', mt: 1 }}>
    <Typography variant="subtitle2">无忧行原 IM 记录</Typography>
    <Typography variant="caption" color="text.secondary">{IM_HISTORY_SCOPE}</Typography>
    {uncertain?.line === lineId && <Alert severity="warning">删除结果尚未核实；仅继续读取原账号，不自动重放删除。</Alert>}
    {userUncertain?.line === lineId && <Alert severity="warning">用户已读写入未核实；仅查询原账号状态，不自动重放。</Alert>}
    {userRead?.installed && <Typography variant="caption" color="text.secondary">{USER_READ_SCOPE}</Typography>}
    {userRead?.recipientInstalled && <TextField size="small" label="已读收件人原 URI（可选）" value={recipient} autoComplete="off" slotProps={{ htmlInput: { maxLength: 512 } }} disabled={busy || !userRead.hosted} onChange={(event) => setRecipientInput({ context: userReadContext, value: event.target.value })} />}
    {rows.map((row) => <Stack key={row.id} spacing={0.5}>
      <Typography variant="caption">{row.kind === 'p2p' ? '点对点' : '群组'} · 消息索引 {row.index} · {row.receivedAt}</Typography>
      <Typography variant="body2" sx={{ whiteSpace: 'pre-wrap' }}>{row.text}</Typography>
      {!!row.attachments && <Typography variant="caption">私有附件 {row.attachments} 项；不会按服务端文件名写入路径或自动打开。</Typography>}
      {userRead?.installed && row.kind === 'p2p' && !row.index.startsWith('-') && <Button size="small" disabled={busy || !userRead.hosted || !currentUserOutcomes || userReadKnownUnknown(currentUserOutcomes, lineId, currentUserOutcomes.generation, row.id, row.index) || userUncertain?.line === lineId && userUncertain.id === row.id} onClick={() => { void markRead(row) }}>明确标记原消息已读</Button>}
      {userRead?.installed && userRead.recipientInstalled && row.kind === 'p2p' && !row.index.startsWith('-') && <Button size="small" disabled={busy || !userRead.hosted || !currentUserOutcomes || !userReadRecipientAllowed(recipient) || userReadKnownUnknown(currentUserOutcomes, lineId, currentUserOutcomes.generation, row.id, row.index) || userUncertain?.line === lineId && userUncertain.id === row.id} onClick={() => { void markRead(row, recipient) }}>通过原收件人解析标记已读</Button>}
      {currentUserOutcomes && currentUserOutcomes.rows.filter((result) => result.messageId === row.id).slice(0, 1).map((result) => <Typography key={result.id} variant="caption">{userReadLabel(result.state)}</Typography>)}
      <Button size="small" color="warning" disabled={busy || (uncertain?.line === lineId && uncertain.id === row.id)} onClick={() => { void remove(row) }}>删除本地 IM 文本</Button>
    </Stack>)}
  </Stack>
}
