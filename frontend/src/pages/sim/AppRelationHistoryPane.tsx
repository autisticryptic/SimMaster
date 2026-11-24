import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, Typography } from '@mui/material'
import { api } from '../../api/current'
import { relationHistoryRows, relationHistoryLabel, RELATION_HISTORY_SCOPE, type RelationHistoryRow } from '../../policies/relationHistory'
export default function AppRelationHistoryPane({ lineId, revision }: { lineId: string; revision: number }) {
  const context = JSON.stringify([lineId, revision])
  const [result, setResult] = useState<{ context: string; rows: RelationHistoryRow[] } | null>(null)
  const [error, setError] = useState<{ context: string } | null>(null)
  const [busy, setBusy] = useState(false)
  const current = useRef({ context, generation: 0, live: false }); const reading = useRef(false)
  useEffect(() => {
    const generation = current.current.generation + 1
    current.current = { context, generation, live: true }
    return () => { if (current.current.generation === generation) current.current.live = false }
  }, [context])
  const read = async () => {
    if (reading.current || !current.current.live || current.current.context !== context) return
    const original = { ...current.current }; reading.current = true; setBusy(true); setError(null); setResult(null)
    try {
      const response = await api.getAppRelationHistory(lineId); const rows = relationHistoryRows(response.data)
      if (!rows) throw new Error('Invalid private relation outcome projection')
      if (!current.current.live || current.current.context !== original.context || current.current.generation !== original.generation) return
      setResult({ context: original.context, rows })
    } catch {
      if (current.current.live && current.current.context === original.context && current.current.generation === original.generation) setError({ context: original.context })
    } finally { reading.current = false; if (current.current.live) setBusy(false) }
  }
  const rows = result?.context === context ? result.rows : null
  return <Stack spacing={0.5} sx={{ mt: 1, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原 Group/Buddy 关系修改记录（只读）</Typography>
    <Typography variant="caption" color="text.secondary">{RELATION_HISTORY_SCOPE}</Typography>
    <Button size="small" disabled={busy} onClick={() => { void read() }}>明确读取关系修改记录（只读）</Button>
    {error?.context === context && <Alert severity="warning">关系修改记录未核实；不会自动查询、重新登录、换 nonce 或重放未知 WRITE。</Alert>}
    {rows?.length === 0 && <Typography variant="caption">本次未返回既有修改记录；不是先前写入未发生或当前 WRITE 可用的证明。</Typography>}
    {rows?.map((row) => <Stack key={row.id} spacing={0.25}>
      <Typography variant="caption">记录 #{row.id} · {row.targetKind === 'buddy' ? 'Buddy 关系' : 'Group 关系'} · {relationHistoryLabel(row.state)}</Typography>
      <Typography variant="caption">新增 {row.additions} 项 · 更新 {row.updates} 项 · 删除 {row.removals} 项</Typography>
      {row.baseTime !== null && <Typography variant="caption">原 BaseTime {row.baseTime} · UpdateTime {row.updateTime}（signed64 原值，不推断墙上时钟或在线状态）</Typography>}
    </Stack>)}
    <Typography variant="caption" color="text.secondary">此视图没有关系写入按钮；完整修改交互与原服务端关系事件尚待独立验收。</Typography>
  </Stack>
}
