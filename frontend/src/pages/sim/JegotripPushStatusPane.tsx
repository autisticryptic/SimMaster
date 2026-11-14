import { useEffect, useState } from 'react'
import { Alert, Button, Stack, Typography } from '@mui/material'
import { api, type JegotripPushStatus } from '../../api/current'
import { appLoginError, startupFailureLabel, accountAliveLabel } from '../../policies/appLogin'

const warningLabels: Record<string, string> = {
  login_manager_uninitialized: '登录管理器未初始化',
  runtime_template_missing: '原生安装模板未配置',
  sms_receiver_missing_or_unsupported: '未配置受支持的 Sip.Request 短信接收绑定',
  call_receiver_missing: '来电接收绑定未配置',
  call_control_missing: '通话控制未配置',
  call_media_missing: '电话媒体未配置',
  native_accounts_absent: '尚无原生接收账号',
  no_armed_sms_receiver: '当前没有已武装的在线短信接收器',
  no_armed_call_receiver: '当前没有已武装的在线来电接收器',
  account_alive_unimplemented: '服务端返回了尚未实现的 AccountAlive 通道',
  account_alive_transport_not_approved: '独立Alive需要明确批准旧式非TLS传输；当前未连接',
  account_alive_endpoint_unapproved: '独立Alive返回的端点不在已批准范围；当前未连接',
  account_alive_retrying: '独立Alive正在按有限退避等待注册，不代表收件就绪',
  platform_offline_wakeup_unimplemented: 'Android 厂商离线唤醒尚未实现',
  call_notifications_unimplemented: '来电的第三方通知转发尚未接线',
}
const count = (value: number | null) => value === null ? '存储未初始化' : String(value)

export default function JegotripPushStatusPane({ open, refreshKey }: { open: boolean; refreshKey: number }) {
  const [loaded, setLoaded] = useState<{ key: string; data: JegotripPushStatus | null; error: string | null } | null>(null)
  const [refresh, setRefresh] = useState(0)
  const requestKey = `${refresh}:${refreshKey}`
  const current = open && loaded?.key === requestKey ? loaded : null
  const view = current?.data ?? null
  const error = current?.error ?? null
  const busy = open && current === null
  useEffect(() => {
    const scope = { active: open }
    if (open) void api.getJegotripPushStatus().then((response) => {
      if (!response.data) throw new Error('missing push status')
      if (scope.active) setLoaded({ key: requestKey, data: response.data, error: null })
    }).catch((failure: unknown) => {
      if (scope.active) setLoaded({ key: requestKey, data: null, error: appLoginError(failure) })
    })
    return () => { scope.active = false }
  }, [open, requestKey])
  return <Stack spacing={1}>
    <Stack direction="row" justifyContent="space-between" alignItems="center">
      <Typography variant="subtitle2">短信 / 来电推送诊断</Typography>
      <Button size="small" disabled={busy} onClick={() => setRefresh((value) => value + 1)}>刷新推送状态</Button>
    </Stack>
    <Alert severity="info">本诊断只读取当前运行和历史记录，不登录无忧行、不修改授信或托管开关。接收器已武装、历史落库、本次真实收到短信 / 来电是不同证据；仍须本人号码的外部收件测试。</Alert>
    {view && <>
      <Typography variant="body2">原生接收账号：{view.native_accounts.length} · 在线短信接收器：{view.native_accounts.filter((a) => a.incoming_message_ready).length} · 在线来电接收器：{view.native_accounts.filter((a) => a.incoming_call_ready).length}</Typography>
      <Typography variant="caption" color="text.secondary">历史无忧行短信 {count(view.stored_observations.app_sms_records)} · 历史无忧行来电 {count(view.stored_observations.app_call_records)} · 待处理短信通知 {count(view.stored_observations.pending_sms_notifications)}。历史记录不会自动证明当前链路可用。</Typography>
      {view.native_accounts.map((account) => <Typography key={account.line_id} variant="caption" sx={{ overflowWrap: 'anywhere' }}>
        {account.line_id} · {account.phase} · 短信{account.incoming_message_ready ? '已武装，需收件测试' : '未武装'} · 来电{account.incoming_call_ready ? '已武装，需来电测试' : '未武装'}
        {account.account_alive_state && ` · AccountAlive：${accountAliveLabel(account.account_alive_state)}`}
        {account.startup_failure && <span> · 启动连接诊断：{startupFailureLabel(account.startup_failure)}</span>}
      </Typography>)}
      {view.warnings.length > 0 && <Alert severity="warning">{view.warnings.map((code) => <Typography key={code} variant="body2">• {warningLabels[code] ?? '有未支持或未验证的推送条件'}</Typography>)}</Alert>}
      <Typography variant="caption" color="text.secondary">服务器在线推送使用原生长连接；SSE / 页面事件是本地显示，Webhook / Telegram 等是接收后的通知转发。停掉进程后没有 Android 厂商离线唤醒，勿将其视为在线收件的替代。</Typography>
    </>}
    {error && <Alert severity="warning">无法读取推送诊断：{error}</Alert>}
  </Stack>
}
