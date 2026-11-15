import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, Typography } from '@mui/material'
import { api } from '../../api/current'
import { IM_HISTORY_SCOPE, imHistoryRows, type ImHistoryRow } from '../../policies/imHistory'
export default function AppImMessagesPane({ lineId }: { lineId: string }) {
  const [loaded, setLoaded] = useState<{ line: string; rows: ImHistoryRow[]; error: boolean } | null>(null)
  const [busy, setBusy] = useState(false)
  const [uncertain, setUncertain] = useState<{ line: string; id: number } | null>(null)
  const active = useRef({ line: lineId, generation: 0, readSerial: 0, live: false })
  const deleting = useRef(false)
  useEffect(() => {
    const generation = active.current.generation + 1
    active.current = { line: lineId, generation, readSerial: 0, live: true }
    let current = true; let timer: ReturnType<typeof setTimeout> | undefined
    const read = async () => {
      if (deleting.current) { if (current) timer = setTimeout(() => { void read() }, 5000); return }
      const serial = ++active.current.readSerial
      try {
        const response = await api.getAppImMessages(lineId)
        if (current && active.current.generation === generation && active.current.readSerial === serial) setLoaded({ line: lineId, rows: imHistoryRows(response.data), error: false })
      } catch { if (current && active.current.generation === generation && active.current.readSerial === serial) setLoaded({ line: lineId, rows: [], error: true }) }
      if (current) timer = setTimeout(() => { void read() }, 5000)
    }
    void read()
    return () => { current = false; if (timer) clearTimeout(timer); if (active.current.generation === generation) active.current.live = false }
  }, [lineId])
  const remove = async (row: ImHistoryRow) => {
    if (deleting.current || (uncertain?.line === lineId && uncertain.id === row.id) || !active.current.live) return
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
  const rows = loaded?.line === lineId ? loaded.rows : []
  if (loaded?.line === lineId && loaded.error) return <Alert severity="warning">无法读取原 IM 本地记录；不会据此重发、重新登录或确认服务端已读。</Alert>
  if (!rows.length && uncertain?.line !== lineId) return null
  return <Stack spacing={1} sx={{ overflowWrap: 'anywhere', mt: 1 }}>
    <Typography variant="subtitle2">无忧行原 IM 记录</Typography>
    <Typography variant="caption" color="text.secondary">{IM_HISTORY_SCOPE}</Typography>
    {uncertain?.line === lineId && <Alert severity="warning">删除结果尚未核实；仅继续读取原账号，不自动重放删除。</Alert>}
    {rows.map((row) => <Stack key={row.id} spacing={0.5}>
      <Typography variant="caption">{row.kind === 'p2p' ? '点对点' : '群组'} · 消息索引 {row.index} · {row.receivedAt}</Typography>
      <Typography variant="body2" sx={{ whiteSpace: 'pre-wrap' }}>{row.text}</Typography>
      {!!row.attachments && <Typography variant="caption">私有附件 {row.attachments} 项；不会按服务端文件名写入路径或自动打开。</Typography>}
      <Button size="small" color="warning" disabled={busy || (uncertain?.line === lineId && uncertain.id === row.id)} onClick={() => { void remove(row) }}>删除本地 IM 文本</Button>
    </Stack>)}
  </Stack>
}
