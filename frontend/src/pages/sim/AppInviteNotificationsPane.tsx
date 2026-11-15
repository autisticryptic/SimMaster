import { useEffect, useState } from 'react'
import { Alert, Stack, Typography } from '@mui/material'
import { api } from '../../api/current'
import { sdkInviteRows, type SdkInviteRow } from '../../policies/sdkInviteNotifications'
export default function AppInviteNotificationsPane({ lineId, enabled }: { lineId: string; enabled: boolean }) {
  const [loaded, setLoaded] = useState<{ line: string; rows: SdkInviteRow[]; error: boolean } | null>(null)
  const rows = enabled && loaded?.line === lineId ? loaded.rows : []
  const error = enabled && loaded?.line === lineId && loaded.error
  useEffect(() => {
    if (!enabled) return
    let active = true; let timer: ReturnType<typeof setTimeout> | undefined
    const read = async () => {
      try {
        const response = await api.getAppCalls(lineId)
        if (!active) return
        setLoaded({ line: lineId, rows: sdkInviteRows(response.data), error: false })
      } catch { if (active) setLoaded({ line: lineId, rows: [], error: true }) }
      if (active) timer = setTimeout(() => { void read() }, 5000)
    }
    void read()
    return () => { active = false; if (timer) clearTimeout(timer) }
  }, [lineId, enabled])
  if (!enabled) return null
  if (error) return <Alert severity="warning">无法读取原 SDK Invite 消费状态；不会重发通知、重新登录或恢复媒体。</Alert>
  if (!rows.length) return null
  return <Stack spacing={0.5} sx={{ overflowWrap: 'anywhere' }}>
    <Typography variant="subtitle2">原 SDK Invite 业务关联（不是渠道送达状态）</Typography>
    {rows.map((row) => <Typography key={row.historyId} variant="caption">原通话记录 {row.historyId} · {row.label}</Typography>)}
    <Typography variant="caption" color="text.secondary">需要显式配置 native_receivers.invite_notifications 及独立来电运行时。此处只显示原 Invite 消费结果；Message IM 消费器尚未装配。</Typography>
  </Stack>
}
