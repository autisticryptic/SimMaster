import { useEffect, useState } from 'react'
import { Alert, Stack, Typography } from '@mui/material'
import { api, type AppCallNotificationStatus } from '../../api/current'
import { callNotificationLabel } from '../../policies/callNotifications'
export default function AppCallNotificationsPane({ lineId }: { lineId: string }) {
  const [loaded, setLoaded] = useState<{ line: string; rows: AppCallNotificationStatus[]; error: boolean } | null>(null)
  const rows = loaded?.line === lineId ? loaded.rows : []
  const error = loaded?.line === lineId && loaded.error
  useEffect(() => {
    let active = true; let timer: ReturnType<typeof setTimeout> | undefined
    const read = async () => {
      try {
        const response = await api.getAppCallNotifications(lineId)
        if (!active) return
        setLoaded({ line: lineId, rows: Array.isArray(response.data) ? response.data.slice(0, 16) : [], error: false })
      } catch { if (active) setLoaded({ line: lineId, rows: [], error: true }) }
      if (active) timer = setTimeout(() => { void read() }, 5000)
    }
    void read()
    return () => { active = false; if (timer) clearTimeout(timer) }
  }, [lineId])
  if (error) return <Alert severity="warning">无法读取原来电通知状态；不会据此重发、重新登录或恢复媒体。</Alert>
  if (!rows.length) return null
  return <Stack spacing={0.5}>
    <Typography variant="subtitle2">无忧行来电渠道通知（与原邀请ACK分开）</Typography>
    {rows.filter((row) => Number.isSafeInteger(row.id) && row.id > 0 && Number.isSafeInteger(row.call_history_id) && row.call_history_id > 0).map((row) => <Typography key={row.id} variant="caption">
      原通话记录 {row.call_history_id} · {callNotificationLabel(row.state)}
    </Typography>)}
    <Typography variant="caption" color="text.secondary">需要安装侧显式启用call_notifications及本人配置的通话渠道规则；仅原ringing窗口发送，接听/结束/撤销后停止，未知发送结果不重放。</Typography>
  </Stack>
}
