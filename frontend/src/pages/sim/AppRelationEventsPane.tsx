import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, Typography } from '@mui/material'
import { api } from '../../api/current'
import { relationEventRows, relationEventLabel, RELATION_EVENTS_SCOPE, RELATION_EVENTS_UNKNOWN, type RelationEventRow } from '../../policies/relationEvents'

export default function AppRelationEventsPane({ lineId, revision }: { lineId: string; revision: number }) {
  // A keyed lifetime hides old rows synchronously, including A -> B -> A account/revision changes.
  return <RelationEventsRead key={JSON.stringify([lineId, revision])} lineId={lineId} />
}
function RelationEventsRead({ lineId }: { lineId: string }) {
  const [rows, setRows] = useState<RelationEventRow[] | null>(null)
  const [error, setError] = useState(false); const [busy, setBusy] = useState(false)
  const current = useRef({ generation: 0, live: false }); const reading = useRef(false)
  useEffect(() => {
    const generation = current.current.generation + 1
    current.current = { generation, live: true }
    return () => { if (current.current.generation === generation) current.current.live = false }
  }, [])
  const read = async () => {
    if (reading.current || !current.current.live) return
    const generation = current.current.generation
    const active = () => current.current.live && current.current.generation === generation
    reading.current = true; setBusy(true); setError(false); setRows(null)
    try {
      const response = await api.getAppRelationEvents(lineId)
      const result = relationEventRows(response.data)
      if (!result) throw new Error('Invalid private relation event projection')
      if (active()) setRows(result)
    } catch {
      if (active()) setError(true)
    } finally {
      if (active()) { reading.current = false; setBusy(false) }
    }
  }
  return <Stack spacing={0.5} sx={{ mt: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原 Group.Update 入站关系事件（只读）</Typography>
    <Typography variant="caption" color="text.secondary">{RELATION_EVENTS_SCOPE}</Typography>
    <Button size="small" disabled={busy} onClick={() => { void read() }}>明确读取入站关系事件（只读）</Button>
    {error && <Alert severity="warning">入站关系事件未核实；不展示错误响应内容，不自动重试，不解决 Unknown-WRITE。</Alert>}
    {rows?.length === 0 && <Typography variant="caption">本次未返回入站关系事件；不是没有关系、没有事件或先前写入未发生的证明。</Typography>}
    {rows?.map((row) => <Stack key={row.id} spacing={0.25}>
      <Typography variant="caption">事件 #{row.id} · {row.targetKind === 'buddy' ? 'Buddy 关系' : 'Group 关系'} · {relationEventLabel(row.continuity)}</Typography>
      <Typography variant="caption">观测更新 {row.updates} 项 · 观测删除 {row.removals} 项 · 完整名册未核实（false）</Typography>
      <Typography variant="caption">原 BaseTime {row.baseTime} · UpdateTime {row.updateTime}（signed64 原值，不转换为墙上时钟或在线状态）</Typography>
      <Typography variant="caption">本地接收时间 {row.receivedAt}（不代表关系当前有效或 WRITE 已确认）</Typography>
    </Stack>)}
    <Typography variant="caption" color="text.secondary">unseeded 表示尚无基线；partial_chain 仅为局部连续；gap 不自动推进基线或清除缺口；stale 是旧事件。任何状态均不证明完整关系名册。</Typography>
    <Typography variant="caption" color="text.secondary">{RELATION_EVENTS_UNKNOWN}</Typography>
  </Stack>
}
