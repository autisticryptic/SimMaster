import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Stack, Typography } from '@mui/material'
import { api } from '../../api/current'
import { userReadRows, userReadLabel, type UserReadRow } from '../../policies/imUserRead'
import { imReconciliationView, imReconciliationLabel, IM_RECONCILIATION_SCOPE, type ImReconciliationView } from '../../policies/imReconciliation'
export default function AppImReconciliationPane({ lineId, revision, hosted, installed }: { lineId: string; revision: number; hosted: boolean; installed: boolean }) {
  const context = JSON.stringify([lineId, revision, hosted, installed])
  const [loaded, setLoaded] = useState<{ context: string; rows: UserReadRow[]; error: boolean } | null>(null)
  const [observed, setObserved] = useState<{ context: string; value: ImReconciliationView } | null>(null)
  const [failed, setFailed] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const current = useRef({ context, generation: 0, live: false })
  const reading = useRef(false)
  useEffect(() => {
    const generation = current.current.generation + 1; current.current = { context, generation, live: true }
    let active = true; let timer: ReturnType<typeof setTimeout> | undefined
    const read = async () => {
      if (!installed) return
      try {
        const value = await api.getAppImUserReads(lineId)
        if (active && current.current.generation === generation) setLoaded({ context, rows: userReadRows(value.data), error: false })
      } catch { if (active && current.current.generation === generation) setLoaded({ context, rows: [], error: true }) }
      if (active) timer = setTimeout(() => { void read() }, 5000)
    }
    void read()
    return () => { active = false; if (timer) clearTimeout(timer); if (current.current.generation === generation) current.current.live = false }
  }, [context, lineId, installed])
  const reconcile = async (row: UserReadRow) => {
    if (reading.current || !current.current.live || current.current.context !== context || !installed || !hosted || !Number.isSafeInteger(revision) || revision <= 0 || loaded?.context !== context || loaded.error) return
    if (!window.confirm('只读向原服务端核对这条已读记录的当前报告？不会更改已读、清空未知写入、重新发送、登录或恢复授信。')) return
    const original = { ...current.current }; reading.current = true; setBusy(true)
    try {
      const result = await api.reconcileAppImUserRead(lineId, row.id, revision)
      const value = imReconciliationView(result.data)
      if (!value || value.receiptId !== row.id || value.messageId !== row.messageId || value.targetIndex !== row.index) throw new Error('Original read observation unconfirmed')
      if (!current.current.live || current.current.generation !== original.generation || current.current.context !== original.context) return
      setObserved({ context, value }); setFailed(null)
    } catch { if (current.current.live && current.current.generation === original.generation && current.current.context === original.context) setFailed(context) }
    finally { reading.current = false; if (current.current.live) setBusy(false) }
  }
  if (!installed) return null
  if (loaded?.context === context && loaded.error) return <Alert severity="warning">无法读取原用户已读记录；不自动查询服务端或重放未知写入。</Alert>
  const rows = loaded?.context === context ? loaded.rows : []
  if (!rows.length) return null
  return <Stack spacing={1} sx={{ mt: 1, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原已读位置只读核对</Typography>
    <Typography variant="caption" color="text.secondary">{IM_RECONCILIATION_SCOPE}</Typography>
    {failed === context && <Alert severity="warning">本次只读核对未完成；原未知状态保持，不重发、不重新登录。</Alert>}
    {rows.map((row) => <Stack key={row.id} spacing={0.5}>
      <Typography variant="caption">本地原记录 {row.id} · 目标消息 {row.messageId} · 索引 {row.index}</Typography>
      <Typography variant="caption">{userReadLabel(row.state)}</Typography>
      <Button size="small" disabled={busy || !hosted || !Number.isSafeInteger(revision) || revision <= 0} onClick={() => { void reconcile(row) }}>只读核对原已读位置</Button>
      {observed?.context === context && observed.value.receiptId === row.id && <Stack spacing={0.5}>
        <Typography variant="body2">{imReconciliationLabel(observed.value)}</Typography>
        <Typography variant="caption">当前报告用户已读 {observed.value.lastRead ?? '未提供'} · 接收位置 {observed.value.lastReceived ?? '未提供'} · 最新消息 {observed.value.latest ?? '未提供'}</Typography>
        <Typography variant="caption">服务端状态时间 {observed.value.serverTime ?? '未提供'} · 本次本地观察 {observed.value.observedAt}</Typography>
      </Stack>}
    </Stack>)}
  </Stack>
}
