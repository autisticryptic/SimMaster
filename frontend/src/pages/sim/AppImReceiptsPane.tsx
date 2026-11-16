import { useEffect, useRef, useState } from 'react'
import { Alert, Stack, Typography } from '@mui/material'
import { api } from '../../api/current'
import { IM_RECEIPT_SCOPE, imReceiptLabel, imReceiptRows, type ImReceiptRow } from '../../policies/imReceipts'
export default function AppImReceiptsPane({ lineId }: { lineId: string }) {
  const [loaded, setLoaded] = useState<{ line: string; rows: ImReceiptRow[]; failed: boolean } | null>(null)
  const generation = useRef(0)
  useEffect(() => {
    const original = ++generation.current
    let current = true; let timer: ReturnType<typeof setTimeout> | undefined
    const read = async () => {
      try {
        const response = await api.getAppImReceipts(lineId)
        if (current && generation.current === original) setLoaded({ line: lineId, rows: imReceiptRows(response.data), failed: false })
      } catch {
        if (current && generation.current === original) setLoaded({ line: lineId, rows: [], failed: true })
      }
      if (current) timer = setTimeout(() => { void read() }, 5000)
    }
    void read()
    return () => { current = false; if (timer) clearTimeout(timer) }
  }, [lineId])
  if (loaded?.line !== lineId) return null
  if (loaded.failed) return <Alert severity="warning">无法读取原接收回执；不会据此重发写入、登录或确认服务端已读。</Alert>
  if (!loaded.rows.length) return null
  return <Stack spacing={0.5} sx={{ mt: 1, overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">无忧行原接收位置回执</Typography>
    <Typography variant="caption" color="text.secondary">{IM_RECEIPT_SCOPE}</Typography>
    {loaded.rows.map((row) => <Stack key={row.id} spacing={0.25}>
      <Typography variant="caption">{row.kind === 'p2p' ? '点对点' : '群组'} · 接收索引 {row.index} · {row.updatedAt}</Typography>
      <Typography variant="body2" color={row.state === 'uncertain' ? 'warning.main' : 'text.secondary'}>{imReceiptLabel(row.state)}</Typography>
    </Stack>)}
  </Stack>
}
