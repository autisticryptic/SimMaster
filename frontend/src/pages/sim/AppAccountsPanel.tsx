import { useCallback, useEffect, useState } from 'react'
import { Alert, Box, Button, Chip, Paper, Stack, Typography } from '@mui/material'
import { api, type AppAccountView, type AppLoginAttempt, type JegotripAccountView, type JegotripConfigurationView } from '../../api/current'
import AppCallNotificationsPane from './AppCallNotificationsPane'
import AppInviteNotificationsPane from './AppInviteNotificationsPane'
import AppImMessagesPane from './AppImMessagesPane'
import AppImReceiptsPane from './AppImReceiptsPane'
import AppImReadStatusPane from './AppImReadStatusPane'
import AppImSendPane from './AppImSendPane'
import AppImReconciliationPane from './AppImReconciliationPane'
import AppUserEntryPane from './AppUserEntryPane'
import AppNativeDirectoryPane from './AppNativeDirectoryPane'
import AppRelationHistoryPane from './AppRelationHistoryPane'
import AppRelationEventsPane from './AppRelationEventsPane'
import AppOrgRelationEventsPane from './AppOrgRelationEventsPane'
import AppRelationRosterPane from './AppRelationRosterPane'
import AppBuddyRelationCheckPane from './AppBuddyRelationCheckPane'
import AppRelationWritePane from './AppRelationWritePane'
import JegotripConfigurationDialog from './JegotripConfigurationDialog'
import { appBusinessLabel, appLoginError, appLoginPhase } from '../../policies/appLogin'

export default function AppAccountsPanel({ onLogin, refreshKey = 0 }: { onLogin: () => void; refreshKey?: number }) {
  const [accounts, setAccounts] = useState<AppAccountView[]>([])
  const [attempts, setAttempts] = useState<AppLoginAttempt[]>([])
  const [summaries, setSummaries] = useState<JegotripAccountView[]>([])
  const [configuration, setConfiguration] = useState<JegotripConfigurationView | null>(null)
  const [configurationOpen, setConfigurationOpen] = useState(false)
  const [settingsRefresh, setSettingsRefresh] = useState(0)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const load = useCallback(async () => {
    const [a, b, c, d] = await Promise.all([api.getAppAccounts(), api.getJegotripLoginAttempts(), api.getJegotripAccounts(), api.getJegotripConfiguration().catch(() => ({ data: null }))])
    return { accounts: a.data ?? [], attempts: b.data ?? [], summaries: c.data ?? [], configuration: d.data ?? null }
  }, [])

  useEffect(() => {
    let active = true
    let timer: ReturnType<typeof setTimeout> | undefined
    const poll = async () => {
      try {
        const data = await load()
        if (!active) return
        setAccounts(data.accounts)
        setAttempts(data.attempts)
        setSummaries(data.summaries)
        setConfiguration(data.configuration)
        setError(null)
      } catch {
        if (active) setError('无法读取应用账号状态，暂不提供运行操作。')
      }
      if (active) timer = setTimeout(() => { void poll() }, 5000)
    }
    void poll()
    return () => { active = false; if (timer) clearTimeout(timer) }
  }, [load, refreshKey, settingsRefresh])

  const operate = async (account: AppAccountView, running: boolean) => {
    if (busy || error) return
    setBusy(account.line_id)
    try {
      await api.setAppAccountRunning(account.line_id, account.revision, running)
      const data = await load()
      setAccounts(data.accounts)
      setAttempts(data.attempts)
      setSummaries(data.summaries)
      setConfiguration(data.configuration)
      setError(null)
    } catch (failure) {
      setError(appLoginError(failure))
    } finally { setBusy(null) }
  }

  return (
    <Paper variant="outlined" sx={{ p: 2 }}>
      <Stack spacing={1.5}>
        <Stack direction="row" justifyContent="space-between" alignItems="center" spacing={1}>
          <Box>
            <Typography variant="subtitle1">无忧行应用账号</Typography>
            <Typography variant="caption" color="text.secondary">独立于 SIM/五码线路；业务开通、SDK 在线和接收就绪分别显示。</Typography>
          </Box>
          <Stack direction="row" spacing={1}>
            <Button size="small" onClick={() => setConfigurationOpen(true)}>安装配置</Button>
            <Button size="small" variant="outlined" onClick={onLogin}>登录并启动无忧行</Button>
          </Stack>
        </Stack>
        {configuration && <Typography variant="caption" color="text.secondary">专用数据库配置：{configuration.configuration_source} · 版本 {configuration.revision} · App HTTP {configuration.http_configured ? '已录入' : '未配置'} · 电话模板 {configuration.native_configured ? '已录入，运行时另核验' : '尚未配置'}</Typography>}
        {summaries.map((summary) => <Box key={summary.account_id} sx={{ borderTop: 1, borderColor: 'divider', pt: 1 }}>
          <Typography variant="body2">{summary.masked_login_name} · {summary.masked_number} · 账号 {summary.account_id.slice(-8)}</Typography>
          <Typography variant="caption" color="text.secondary">最近 App 认证：{new Date(summary.last_authenticated_at * 1000).toLocaleString()} · {summary.voice_line_id ? '已关联接收配置，状态见下方' : '尚无已关联接收器，不代表托管业务或接收已就绪'}</Typography>
        </Box>)}
        {error && <Alert severity="warning">{error}</Alert>}
        {!accounts.length && !summaries.length && !error && <Typography variant="body2" color="text.secondary">暂无无忧行账号，可登录已绑定号码的账号并自动启动无忧行接收器。</Typography>}
        {!!accounts.length && <Typography variant="caption" color="text.secondary">接收器运行（与最近 App 认证分开）</Typography>}
        {accounts.map((account) => {
          const attempt = attempts.find((a) => a.line_id === account.line_id)
          return (
            <Stack key={account.line_id} direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems={{ sm: 'center' }} sx={{ borderTop: 1, borderColor: 'divider', pt: 1 }}>
              <Box sx={{ flex: 1, minWidth: 0 }}>
                <Typography variant="body2">{attempt?.masked_number ?? '应用账号'} · {account.line_id.slice(-8)}</Typography>
                <Typography variant="caption" color="text.secondary">SDK：{account.phase}{attempt ? ` · ${appBusinessLabel(attempt.business_state)}` : ''}</Typography>
                {account.last_error && <Typography variant="caption" display="block" color="warning.main">{appLoginError(account.last_error)}</Typography>}
                <AppCallNotificationsPane lineId={account.line_id} />
                <AppInviteNotificationsPane lineId={account.line_id} enabled={account.incoming_call_ready} />
                <AppImMessagesPane lineId={account.line_id} userRead={{ revision: account.revision, installed: account.im_user_read_supported === true, hosted: account.phase === 'hosted', recipientInstalled: account.im_user_read_recipient_supported === true }} />
                <AppNativeDirectoryPane lineId={account.line_id} revision={account.revision} hosted={account.phase === 'hosted'} installed={account.directory_queries_supported === true} />
                <AppRelationHistoryPane lineId={account.line_id} revision={account.revision} />
                <AppRelationEventsPane lineId={account.line_id} revision={account.revision} />
                <AppOrgRelationEventsPane lineId={account.line_id} revision={account.revision} />
                <AppBuddyRelationCheckPane lineId={account.line_id} revision={account.revision} hosted={account.phase === 'hosted'} installed={account.buddy_relation_check_supported === true} />
                <AppRelationRosterPane lineId={account.line_id} revision={account.revision} hosted={account.phase === 'hosted'} installed={account.relation_roster_supported === true} />
                <AppRelationWritePane lineId={account.line_id} revision={account.revision} hosted={account.phase === 'hosted'} installed={account.relation_updates_supported === true} />
                <AppUserEntryPane lineId={account.line_id} revision={account.revision} hosted={account.phase === 'hosted'} installed={account.user_entry_supported === true || account.user_entry_uid_local_supported === true} localOnly={account.user_entry_supported !== true} />
                <AppImReceiptsPane lineId={account.line_id} />
                <AppImReadStatusPane lineId={account.line_id} revision={account.revision} hosted={account.phase === 'hosted'} installed={account.im_read_status_supported === true} recipientInstalled={account.im_read_status_recipient_supported === true} />
                <AppImSendPane lineId={account.line_id} revision={account.revision} hosted={account.phase === 'hosted'} installed={account.im_send_supported === true} />
                <AppImReconciliationPane lineId={account.line_id} revision={account.revision} hosted={account.phase === 'hosted'} installed={account.im_reconciliation_supported === true} />
              </Box>
              <Chip size="small" color={account.incoming_message_ready ? 'success' : 'default'} label={account.incoming_message_ready ? '短信就绪' : '短信未就绪'} />
              <Chip size="small" color={account.incoming_call_ready ? 'success' : 'default'} label={account.incoming_call_ready ? '来电就绪' : '来电未就绪'} />
              <Button size="small" disabled={!!busy || !!error || account.io_cleanup_required || account.phase === 'hosted'} onClick={() => { void operate(account, true) }}>启动接收</Button>
              <Button size="small" disabled={!!busy || !!error || !account.io_cleanup_required} onClick={() => { void operate(account, false) }}>停止接收</Button>
            </Stack>
          )
        })}
        {attempts.slice(0, 3).map((attempt) => (
          <Alert key={attempt.attempt_id} severity={attempt.error ? 'warning' : 'info'}>
            {appLoginPhase(attempt.phase)} · {appBusinessLabel(attempt.business_state)} · 任务 {attempt.attempt_id.slice(-8)}
            {attempt.error && <Typography variant="caption" display="block">{appLoginError(attempt.error)}</Typography>}
          </Alert>
        ))}
        {!!accounts.length && <Typography variant="caption" color="text.secondary">停止接收只注销本客户端，不取消服务端托管/呼转。短信可在原短信记录中查看；外呼和发短信仍未开放。</Typography>}
      </Stack>
      <JegotripConfigurationDialog open={configurationOpen} onClose={() => setConfigurationOpen(false)} onSaved={() => setSettingsRefresh((value) => value + 1)} />
    </Paper>
  )
}
