import type { ReactNode } from 'react'
import { Alert, Box, Chip, Typography } from '@mui/material'
import Grid from '@mui/material/Grid'
import type { AppEventEntry, CallRecord, SmsMessage, TrunkProfileResponse, VolteLineControlResponse, VowifiLineConfigResponse, VowifiRuntimeEventEntry } from '../../api/current'
import { standardDerivedProfileMessage, volteErrorMessage } from './volteErrorFormat'

function Field({ label, value }: { label: string, value: ReactNode }) {
  return (
    <Box minWidth={0}>
      <Typography variant="caption" color="text.secondary">{label}</Typography>
      <Typography variant="body2" sx={{ mt: 0.25, wordBreak: 'break-word' }}>{value}</Typography>
    </Box>
  )
}

export function LineCsDetails({ line }: { line: VolteLineControlResponse }) {
  return (
    <Grid container spacing={2}>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="基带状态" value={line.modem.state || '未知'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运营商" value={line.modem.operator_id || '未读取'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="当前设备" value={line.modem.present ? '已连接' : '未连接'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="ModemManager 路径" value={line.modem.modem_path || '未发现'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="主控制端口" value={line.modem.primary_port || '未发现'} /></Grid>
    </Grid>
  )
}

export function LineVolteDetails({ line }: { line: VolteLineControlResponse }) {
  const fallbackMessage = standardDerivedProfileMessage(
    line.runtime.profile_source,
    line.runtime.profile_fallback_reason,
  )
  return (
    <Grid container spacing={2}>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="IMS 阶段" value={`${line.runtime.phase} / ${line.runtime.stage}`} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="注册状态" value={line.runtime.registered ? <Chip size="small" color="success" label="已注册" /> : '未注册'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="注册方式" value={line.runtime.registration_mode || '未确定'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="地址族" value={line.runtime.current_ip_family || '尚未选择'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="Bearer 类型" value={line.runtime.bearer_ip_type || '尚未建立'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="Bearer 网卡" value={line.runtime.bearer_interface || '尚未建立'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="数据 QMI 端口" value={line.runtime.qmi_device || line.modem.qmi_device || '未发现'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}>
        <Field
          label="IMS QMI 端口"
          value={line.runtime.secondary_qmi_device
            ? `${line.runtime.secondary_qmi_device}${line.runtime.secondary_qmi_channel ? ` · ${line.runtime.secondary_qmi_channel}` : ''}`
            : '未启用（IMS 与数据共用主端口）'}
        />
      </Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="P-CSCF" value={line.runtime.pcscf || '尚未发现'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="IMS 数据路径" value={line.runtime.data_path_mode || '尚未建立'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="REGISTER 续期" value={`${line.runtime.register_refresh_count ?? 0} 次${line.runtime.last_register_refresh_at ? ` · ${new Date(line.runtime.last_register_refresh_at).toLocaleString()}` : ''}`} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="身份来源" value={line.runtime.identity_source || '尚未读取'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="运营商 profile" value={line.runtime.profile_id || '尚未匹配'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="ISIM" value={line.runtime.isim_aid ? `已发现 · ${line.runtime.isim_aid}` : '未发现，使用 IMSI 回退'} /></Grid>
      {fallbackMessage && <Grid size={12}><Alert severity="warning">{fallbackMessage}</Alert></Grid>}
      {line.runtime.last_error && <Grid size={12}><Alert severity="warning">{volteErrorMessage(line.runtime.last_error)}</Alert></Grid>}
    </Grid>
  )
}

/// 前端只渲染最近若干条事件，完整历史保留在后端诊断日志里，避免面板变成长列表。
const ACTIVITY_LOG_VISIBLE_LIMIT = 20

type ActivityLogEntry = {
  at: string
  source: 'VoLTE' | 'VoWiFi' | 'Trunk' | '短信' | '通话' | '系统'
  stage: string
  outcome: string
  detail?: string
  error?: string
}

const volteActivityStageLabels: Record<string, string> = {
  bearer: '建立 IMS Bearer',
  bearer_dual: '建立双栈 IMS Bearer',
  bearer_ipv4: '回退 IPv4 IMS Bearer',
  bearer_ipv6: '回退 IPv6 IMS Bearer',
  ip_config: '配置 IMS 网络',
  pcscf: '发现 P-CSCF',
  register_initial: '发送初始 REGISTER',
  register_authenticated: '发送鉴权 REGISTER',
  register_refresh: '续期 IMS 注册',
  register_ipsec: '通过 IPsec 注册',
  register_udp: '通过 UDP 注册',
  registered: 'IMS 已注册',
}

function compactEventDetail(detailJson: string) {
  try {
    const parsed: unknown = JSON.parse(detailJson)
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return String(parsed)
    return Object.entries(parsed as Record<string, unknown>)
      .filter(([, value]) => value !== null && value !== undefined && value !== '')
      .slice(0, 8)
      .map(([key, value]) => {
        if (typeof value === 'string') return `${key}=${value}`
        if (typeof value === 'number' || typeof value === 'boolean' || typeof value === 'bigint') return `${key}=${value}`
        if (typeof value === 'symbol') return `${key}=${value.description || 'symbol'}`
        if (typeof value === 'function') return `${key}=[function]`
        return `${key}=${JSON.stringify(value)}`
      })
      .join(' · ')
  } catch {
    return detailJson
  }
}

function compactMessageContent(content: string) {
  const normalized = content.replace(/\s+/g, ' ').trim()
  return normalized.length > 96 ? `${normalized.slice(0, 96)}...` : normalized
}

function activityOutcomeLabel(outcome: string) {
  if (outcome === 'succeeded' || outcome === 'success') return '成功'
  if (outcome === 'failed' || outcome === 'error') return '失败'
  if (outcome === 'warning') return '警告'
  if (outcome === 'info') return '记录'
  return '进行中'
}

function activityOutcomeColor(outcome: string): 'success' | 'error' | 'warning' | 'default' {
  if (outcome === 'succeeded' || outcome === 'success') return 'success'
  if (outcome === 'failed' || outcome === 'error') return 'error'
  if (outcome === 'warning') return 'warning'
  return 'default'
}

export function LineActivityLog({
  line,
  appEvents = [],
  vowifiEvents = [],
  trunk,
  smsMessages = [],
  callRecords = [],
}: {
  line: VolteLineControlResponse
  appEvents?: AppEventEntry[]
  vowifiEvents?: VowifiRuntimeEventEntry[]
  trunk?: TrunkProfileResponse
  smsMessages?: SmsMessage[]
  callRecords?: CallRecord[]
}) {
  const unifiedEntries: ActivityLogEntry[] = appEvents.map((event) => {
    const payload = event.payload
    const payloadEntries = payload && typeof payload === 'object' && !Array.isArray(payload)
      ? Object.entries(payload as Record<string, unknown>)
      : [['value', payload]] as [string, unknown][]
    const detail = payloadEntries
      .filter(([key, value]) => key !== 'diagnostic' && value !== null && value !== undefined && value !== '')
      .slice(0, 6)
      .map(([key, value]) => `${key}=${typeof value === 'string' ? value : JSON.stringify(value)}`)
      .join(' · ')
    const source = event.transport === 'vowifi_ims' || event.event_type.startsWith('vowifi.')
      ? 'VoWiFi'
      : event.transport === 'volte_ims' || event.event_type.startsWith('volte.')
        ? 'VoLTE'
        : event.transport === 'trunk' || event.event_type.startsWith('trunk.')
          ? 'Trunk'
          : event.event_type.startsWith('sms.') ? '短信' : event.event_type.startsWith('call.') ? '通话' : '系统'
    return {
      at: event.created_at,
      source,
      stage: event.event_type,
      outcome: event.event_type.endsWith('.failed') || event.event_type.includes('failure') ? 'failed' : 'info',
      detail,
    }
  })
  const entries: ActivityLogEntry[] = [
    ...unifiedEntries,
    ...(appEvents.length > 0 ? [] : [
    ...(line.runtime.connection_attempts ?? []).map((attempt) => ({
      at: attempt.at,
      source: 'VoLTE' as const,
      stage: `${volteActivityStageLabels[attempt.stage] || attempt.stage}${attempt.ip_family ? ` · ${attempt.ip_family.toUpperCase()}` : ''}`,
      outcome: attempt.outcome,
      detail: [
        attempt.detail,
        attempt.at_cid !== undefined && attempt.at_cid !== null ? `CID ${attempt.at_cid}` : null,
        attempt.qmi_device ? `QMI ${attempt.qmi_device}` : null,
        attempt.interface ? `网卡 ${attempt.interface}` : null,
        attempt.bearer_path ? `Bearer ${attempt.bearer_path}` : null,
        attempt.pcscf ? `P-CSCF ${attempt.pcscf}` : null,
      ].filter(Boolean).join(' · ') || undefined,
      error: attempt.error_code,
    })),
    ...vowifiEvents.map((event) => ({
      at: event.created_at,
      source: 'VoWiFi' as const,
      stage: `${event.phase} · ${event.event_type}`,
      outcome: event.level,
      detail: compactEventDetail(event.detail_json),
    })),
    ...smsMessages.map((message) => ({
      at: message.timestamp,
      source: '短信' as const,
      stage: `${message.direction === 'incoming' ? '收到短信' : '发送短信'} · ${message.transport || 'modem'}`,
      outcome: message.status === 'failed' ? 'failed' : message.status === 'pending' ? 'pending' : 'success',
      detail: [message.phone_number, compactMessageContent(message.content)].filter(Boolean).join(' · '),
    })),
    ...callRecords.map((record) => ({
      at: record.start_time,
      source: '通话' as const,
      stage: `${record.direction === 'incoming' ? '来电' : record.direction === 'outgoing' ? '外呼' : '未接来电'} · ${record.answered ? '已接通' : '未接通'}`,
      outcome: record.answered ? 'success' : record.failure_code ? 'failed' : 'pending',
      detail: [
        record.phone_number,
        record.duration > 0 ? `${record.duration} 秒` : null,
        record.sip_status ? `SIP ${record.sip_status}` : null,
        record.carrier_reason,
      ].filter(Boolean).join(' · '),
      error: record.failure_code,
    })),
    ]),
  ]

  if (appEvents.length === 0 && trunk?.trunk.enabled && (trunk.runtime.last_activity_at || trunk.runtime.registered_at || trunk.runtime.started_at)) {
    const runtime = trunk.runtime
    entries.push({
      at: runtime.last_activity_at || runtime.registered_at || runtime.started_at || '',
      source: 'Trunk',
      stage: `当前状态 · ${runtime.stage || runtime.phase}`,
      outcome: runtime.registered || runtime.phase === 'ready' ? 'succeeded' : runtime.last_error ? 'failed' : 'pending',
      detail: [
        runtime.last_sip_status ? `SIP ${runtime.last_sip_status}` : null,
        `REGISTER ${runtime.register_attempts}`,
        `重连 ${runtime.reconnect_count}`,
        `通话 ${runtime.active_calls}`,
        `媒体 ${runtime.media_negotiations}`,
      ].filter(Boolean).join(' · '),
      error: runtime.last_error,
    })
  }

  entries.sort((a, b) => new Date(b.at).getTime() - new Date(a.at).getTime())
  const visibleEntries = entries.slice(0, ACTIVITY_LOG_VISIBLE_LIMIT)

  return (
    <Box>
      <Typography variant="subtitle2" fontWeight={700} mb={0.25}>线路活动日志</Typography>
      <Typography variant="caption" color="text.secondary" display="block" mb={1.5}>
        最近 {ACTIVITY_LOG_VISIBLE_LIMIT} 条 IMS、短信、通话与 Trunk 关键事件；单栈/双栈及回退过程会记录在这里。
        完整历史与未截断的错误串见后端诊断日志。
      </Typography>
      <Box sx={{ maxHeight: 360, overflowY: 'auto', borderTop: 1, borderColor: 'divider' }}>
        {visibleEntries.length === 0 && <Typography variant="body2" color="text.secondary" py={1}>尚无活动记录</Typography>}
        {visibleEntries.map((entry, index) => (
          <Box key={`${entry.source}-${entry.at}-${index}`} display="grid" gridTemplateColumns={{ xs: '1fr auto', sm: '150px 72px minmax(0, 1fr) auto' }} gap={2} py={1} borderBottom={1} borderColor="divider" alignItems="center">
            <Typography variant="caption" color="text.secondary">{new Date(entry.at).toLocaleString()}</Typography>
            <Chip size="small" variant="outlined" label={entry.source} />
            <Box minWidth={0}>
              <Typography variant="body2" sx={{ mt: 0.25, wordBreak: 'break-word' }}>{entry.stage}</Typography>
              {entry.detail && <Typography variant="caption" color="text.secondary" display="block" sx={{ wordBreak: 'break-word' }}>{entry.detail}</Typography>}
              {entry.error && <Typography variant="caption" color="error" display="block" sx={{ wordBreak: 'break-all' }}>{entry.error}</Typography>}
            </Box>
            <Chip size="small" variant="outlined" color={activityOutcomeColor(entry.outcome)} label={activityOutcomeLabel(entry.outcome)} />
          </Box>
        ))}
      </Box>
    </Box>
  )
}

export function LineVowifiDetails({ vowifi }: { vowifi?: VowifiLineConfigResponse }) {
  if (!vowifi) return <Alert severity="info">尚未加载该线路的 VoWiFi 状态。</Alert>
  const fallbackMessage = standardDerivedProfileMessage(
    vowifi.matched_profile_source,
    vowifi.matched_profile_fallback_reason,
  )
  return (
    <Grid container spacing={2}>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运行阶段" value={`${vowifi.runtime_phase} / ${vowifi.runtime_stage}`} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="IMS 注册" value={vowifi.runtime_registered ? '已注册' : '未注册'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运行范围" value="线路独立运行时" /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运营商 profile" value={vowifi.matched_profile_id || '尚未匹配'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="代理模式" value={vowifi.config.proxy_mode} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="代理端点" value={vowifi.config.proxy_endpoint || '直连'} /></Grid>
      {fallbackMessage && <Grid size={12}><Alert severity="warning">{fallbackMessage}</Alert></Grid>}
      {vowifi.runtime_error && <Grid size={12}><Alert severity="warning">{vowifi.runtime_error}</Alert></Grid>}
    </Grid>
  )
}

export function LineTrunkDetails({ trunk }: { trunk?: TrunkProfileResponse }) {
  return (
    <Grid container spacing={2}>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运行阶段" value={trunk ? `${trunk.runtime.phase} / ${trunk.runtime.stage}` : '未加载'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="注册状态" value={trunk?.runtime.registered ? '已注册' : '未注册'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="本地 SIP" value={trunk?.runtime.local_endpoint || '未监听'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="Asterisk Peer" value={trunk?.runtime.peer || '未解析'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="REGISTER / 重连" value={trunk ? `${trunk.runtime.register_attempts} / ${trunk.runtime.reconnect_count}` : '0 / 0'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="通话 / 对话" value={trunk ? `${trunk.runtime.active_calls} / ${trunk.runtime.active_dialogs}` : '0 / 0'} /></Grid>
      {trunk?.runtime.last_error && <Grid size={12}><Alert severity="error">{trunk.runtime.last_error}</Alert></Grid>}
    </Grid>
  )
}
