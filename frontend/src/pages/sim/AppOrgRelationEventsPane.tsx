import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, Typography } from '@mui/material'
import { api } from '../../api/current'
import { orgRelationRows, ORG_RELATION_SCOPE, ORG_RELATION_UNKNOWN, type OrgRelationRow } from '../../policies/orgRelationEvents'
export default function AppOrgRelationEventsPane({ lineId, revision }: { lineId: string; revision: number }) {
  return <ReadOrgRelationEvents key={JSON.stringify([lineId, revision])} lineId={lineId} />
}
function ReadOrgRelationEvents({ lineId }: { lineId: string }) {
  const [rows, setRows] = useState<OrgRelationRow[] | null>(null); const [error, setError] = useState(false); const [busy, setBusy] = useState(false)
  const current = useRef({ generation: 0, live: false }); const reading = useRef(false)
  useEffect(() => { const generation = current.current.generation + 1; current.current = { generation, live: true }; return () => { if (current.current.generation === generation) current.current.live = false } }, [])
  const read = async () => {
    if (!current.current.live || reading.current) return
    const generation = current.current.generation; const active = () => current.current.live && current.current.generation === generation
    reading.current = true; setBusy(true); setError(false); setRows(null)
    try { const response = await api.getAppOrgRelationEvents(lineId); const rows = orgRelationRows(response.data); if (!rows) throw new Error('Invalid original system control projection'); if (active()) setRows(rows) }
    catch { if (active()) setError(true) }
    finally { if (active()) { reading.current = false; setBusy(false) } }
  }
  return <Stack spacing={0.5} sx={{ mt: 1, minWidth: 0, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原 System.OrgRelSync 关系控制记录（只读）</Typography>
    <Typography variant="caption" color="text.secondary">{ORG_RELATION_SCOPE}</Typography>
    <Button size="small" disabled={busy} onClick={() => { void read() }}>明确读取系统关系控制记录（只读）</Button>
    {error && <Alert severity="warning">系统关系控制记录未核实；不显示私有错误内容，不重试、不清除未知 WRITE。</Alert>}
    {rows?.length === 0 && <Typography variant="caption">本次没有返回控制记录；不证明没有事件、完整名册或先前写入未发生。</Typography>}
    {rows?.map((row) => <Stack key={row.id} spacing={0.25}>
      <Typography variant="caption">控制记录 #{row.id} · IM 索引 {row.messageIndex} · {row.continuity}</Typography>
      <Typography variant="caption">关系 BaseTime {row.baseTime ?? '未提供'} · UpdateTime {row.updateTime ?? '未提供'}（signed64，不推断在线状态）</Typography>
      <Typography variant="caption">{row.removal ? '原 type20 删除记录' : '局部关系更新'} · {row.relationSupplied ? '携带单条 UpdateRel' : '使用原缺省 Relation'} · OrgDel {String(row.orgDeleted)}</Typography>
      <Typography variant="caption">完整名册未核实（false）；缺口不自动清除，unsequenced 不推进关系时间。</Typography>
    </Stack>)}
    <Typography variant="caption" color="text.secondary">{ORG_RELATION_UNKNOWN}</Typography>
  </Stack>
}
