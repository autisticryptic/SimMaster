import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Alert,
  Box,
  CircularProgress,
  Snackbar,
  Tab,
  Tabs,
  Typography,
} from '@mui/material'
import { api } from '../api/current'
import { useRefreshInterval } from '../contexts/RefreshContext'
import type {
  NotificationChannelInstance,
  NotificationChannelKey,
  NotificationConfig,
  NotificationEventType,
  NotificationLogCleanupConfig,
  NotificationLogEntry,
  NotificationRule,
  SmsChannelResponse,
} from '../api/current'
import ErrorSnackbar from '../components/ErrorSnackbar'
import NotificationChannelsTab from './notifications/NotificationChannelsTab'
import NotificationLogsTab from './notifications/NotificationLogsTab'
import NotificationQueueIndicator, {
  type NotificationQueueItem,
} from './notifications/NotificationQueueIndicator'
import NotificationRulesTab from './notifications/NotificationRulesTab'
import {
  createChannel,
  createDefaultConfig,
  createRule,
  normalizeConfig,
} from './notifications/notificationModel'

const LOG_PAGE_SIZE = 15
const LINE_EVENT_TYPES: Array<{ key: NotificationEventType, label: string }> = [
  { key: 'sms', label: '短信' },
  { key: 'call', label: '通话' },
  { key: 'automation', label: '自动化事件' },
]

type NotificationLogClearFilters = {
  type: string
  status: string
  line_id: string
  start_date: string
  end_date: string
}

type NotificationCenterProps = {
  lineId?: string
  embedded?: boolean
}

export default function NotificationCenterPage({ lineId, embedded = false }: NotificationCenterProps) {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [tab, setTab] = useState(0)
  const [config, setConfig] = useState<NotificationConfig>(() => createDefaultConfig())
  const [selectedChannelId, setSelectedChannelId] = useState<string>('')
  const [selectedEventType, setSelectedEventType] = useState<NotificationEventType>('sms')
  const [logs, setLogs] = useState<NotificationLogEntry[]>([])
  const [logTotal, setLogTotal] = useState(0)
  const [logType, setLogType] = useState('')
  const [logStatus, setLogStatus] = useState('')
  const [logLineId, setLogLineId] = useState(lineId ?? '')
  const [logStartDate, setLogStartDate] = useState('')
  const [logEndDate, setLogEndDate] = useState('')
  const [logQuery, setLogQuery] = useState('')
  const [logPage, setLogPage] = useState(0)
  const [loading, setLoading] = useState(true)
  const [logsLoading, setLogsLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [cleanupSaving, setCleanupSaving] = useState(false)
  const [testing, setTesting] = useState(false)
  const [queueOpen, setQueueOpen] = useState(false)
  const [queueItems, setQueueItems] = useState<NotificationQueueItem[]>([])
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [smsChannels, setSmsChannels] = useState<SmsChannelResponse[]>([])
  const logsLoadingRef = useRef(false)
  const lastRefreshKeyRef = useRef(refreshKey)

  const selectedChannel = useMemo(
    () => config.channels.find((channel) => channel.id === selectedChannelId) ?? config.channels[0],
    [config.channels, selectedChannelId],
  )
  const notificationLineOptions = useMemo(() => {
    const seen = new Set<string>(['device'])
    return [
      { id: 'device', label: '设备级事件' },
      ...smsChannels.flatMap((channel) => {
        if (seen.has(channel.id)) return []
        seen.add(channel.id)
        return [{ id: channel.id, label: channel.label }]
      }),
    ]
  }, [smsChannels])
  const notificationQueueItems = useMemo(
    () => lineId ? queueItems.filter((item) => item.line_id === lineId) : queueItems,
    [lineId, queueItems],
  )
  const visibleRules = useMemo(
    () => lineId
      ? config.rules.filter((rule) => rule.sim_channel_ids.includes(lineId))
      : config.rules,
    [config.rules, lineId],
  )
  const visibleConfig = useMemo(
    () => ({ ...config, rules: visibleRules }),
    [config, visibleRules],
  )

  const loadConfig = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [response, smsChannelResponse] = await Promise.all([
        api.getNotificationConfig(),
        api.getSmsChannels(),
      ])
      const next = normalizeConfig(response.data)
      setConfig(next)
      setSmsChannels(smsChannelResponse.data ?? [])
      setSelectedChannelId((current) => current || next.channels[0]?.id || '')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  const loadLogs = useCallback(async (silent = false) => {
    if (logsLoadingRef.current) return
    logsLoadingRef.current = true
    if (!silent) setLogsLoading(true)
    try {
      const response = await api.getNotificationLogs({
        type: logType,
        status: logStatus,
        line_id: lineId ?? logLineId,
        start_date: logStartDate,
        end_date: logEndDate,
        q: logQuery,
        limit: LOG_PAGE_SIZE,
        offset: logPage * LOG_PAGE_SIZE,
      })
      setLogs(response.data?.logs ?? [])
      setLogTotal(response.data?.total ?? 0)
    } catch (err) {
      if (!silent) setError(err instanceof Error ? err.message : String(err))
    } finally {
      logsLoadingRef.current = false
      if (!silent) setLogsLoading(false)
    }
  }, [lineId, logEndDate, logLineId, logPage, logQuery, logStartDate, logStatus, logType])

  const loadQueue = useCallback(async (silent = true) => {
    try {
      const response = await api.getNotificationQueue({ line_id: lineId, limit: 100 })
      const items = response.data?.items ?? []
      setQueueItems(items.map((item) => ({
          id: item.id,
          line_id: item.line_id,
          status: item.status,
          event_type: item.event_type,
          event_label: item.event_label,
          summary: item.summary,
          reason: item.reason,
          channel_name: item.channel_name,
          next_attempt_at: item.next_attempt_at,
          attempt_count: item.attempt_count,
          max_attempts: item.max_attempts,
      })))
    } catch (err) {
      if (!silent) setError(err instanceof Error ? err.message : String(err))
    }
  }, [lineId])

  useEffect(() => {
    setLogLineId(lineId ?? '')
    setLogPage(0)
  }, [lineId])

  useEffect(() => {
    void loadConfig()
  }, [loadConfig])

  useEffect(() => {
    void loadQueue()
  }, [loadQueue])

  useEffect(() => {
    if (tab === 0) void loadLogs()
  }, [loadLogs, tab])

  useEffect(() => {
    if (lastRefreshKeyRef.current === refreshKey) return
    lastRefreshKeyRef.current = refreshKey
    if (tab === 0) void loadLogs()
    void loadQueue()
  }, [loadLogs, loadQueue, refreshKey, tab])

  useEffect(() => {
    if (refreshInterval <= 0) return undefined

    const timer = window.setInterval(() => {
      if (document.visibilityState !== 'visible') return
      if (tab === 0) void loadLogs(true)
      void loadQueue(true)
    }, refreshInterval)

    return () => window.clearInterval(timer)
  }, [loadLogs, loadQueue, refreshInterval, tab])

  useEffect(() => {
    const maxPage = Math.max(0, Math.ceil(logTotal / LOG_PAGE_SIZE) - 1)
    if (logPage > maxPage) setLogPage(maxPage)
  }, [logTotal, logPage])

  const patchConfig = (updater: (prev: NotificationConfig) => NotificationConfig) => {
    setConfig((prev) => updater(prev))
  }

  const patchChannel = (id: string, patch: Partial<NotificationChannelInstance>) => {
    patchConfig((prev) => ({
      ...prev,
      channels: prev.channels.map((channel) => channel.id === id ? { ...channel, ...patch } : channel),
    }))
  }

  const patchChannelConfig = (id: string, patch: Record<string, unknown>) => {
    patchConfig((prev) => ({
      ...prev,
      channels: prev.channels.map((channel) => channel.id === id
        ? { ...channel, config: { ...channel.config, ...patch } }
        : channel),
    }))
  }

  const patchRule = (id: string, patch: Partial<NotificationRule>) => {
    patchConfig((prev) => ({
      ...prev,
      rules: prev.rules.map((rule) => rule.id === id
        ? { ...rule, ...patch, sim_channel_ids: lineId ? [lineId] : (patch.sim_channel_ids ?? rule.sim_channel_ids) }
        : rule),
    }))
  }

  const handleAddChannel = (type: NotificationChannelKey) => {
    const channel = createChannel(type)
    patchConfig((prev) => ({ ...prev, channels: [...prev.channels, channel] }))
    setSelectedChannelId(channel.id)
  }

  const handleDeleteChannel = (id: string) => {
    patchConfig((prev) => ({
      ...prev,
      channels: prev.channels.filter((channel) => channel.id !== id),
      rules: prev.rules.map((rule) => ({
        ...rule,
        channel_ids: rule.channel_ids.filter((channelId) => channelId !== id),
      })),
    }))
    setSelectedChannelId((current) => current === id ? '' : current)
  }

  const handleAddRule = () => {
    patchConfig((prev) => ({
      ...prev,
      rules: [
        ...prev.rules,
        {
          ...createRule(
          selectedEventType,
          prev.channels.filter((channel) => channel.enabled).map((channel) => channel.id),
          ),
          sim_channel_ids: lineId ? [lineId] : [],
        },
      ],
    }))
  }

  const handleDeleteRule = (id: string) => {
    patchConfig((prev) => ({ ...prev, rules: prev.rules.filter((rule) => rule.id !== id) }))
  }

  const handleSave = async () => {
    setSaving(true)
    setError(null)
    try {
      const response = await api.setNotificationConfig(config)
      if (response.status === 'ok') {
        setSuccess('通知配置已保存')
      } else {
        setError(response.message)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  const handleTest = async () => {
    if (!selectedChannel) return
    setTesting(true)
    setError(null)
    try {
      await api.setNotificationConfig(config)
      const response = await api.testNotificationChannel(selectedChannel.id)
      if (response.status === 'ok' && response.data?.success) {
        setSuccess(response.data.message)
      } else {
        setError(response.data?.message || response.message)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setTesting(false)
    }
  }

  const handleClearLogs = async (filters: NotificationLogClearFilters) => {
    try {
      const response = await api.clearNotificationLogs({ ...filters, line_id: lineId ?? filters.line_id })
      setLogPage(0)
      const deleted = response.data?.deleted ?? 0
      setSuccess(`已清理 ${deleted} 条转发日志`)
      if (logPage === 0) await loadLogs()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleSaveLogCleanup = async (logCleanup: NotificationLogCleanupConfig) => {
    setCleanupSaving(true)
    setError(null)
    const nextConfig = { ...config, log_cleanup: logCleanup }
    try {
      const response = await api.setNotificationConfig(nextConfig)
      if (response.status === 'ok') {
        setConfig(nextConfig)
        setSuccess('自动清理设置已保存')
      } else {
        setError(response.message)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setCleanupSaving(false)
    }
  }

  const handleRetryQueueItem = async (id: NotificationQueueItem['id']) => {
    try {
      await api.retryNotificationQueueItem(id)
      await loadQueue(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleDeleteQueueItem = async (id: NotificationQueueItem['id']) => {
    try {
      await api.deleteNotificationQueueItem(id)
      await loadQueue(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleRetryAllQueue = async () => {
    try {
      if (lineId) {
        await Promise.all(notificationQueueItems.map((item) => api.retryNotificationQueueItem(item.id)))
      } else {
        await api.retryAllNotificationQueue()
      }
      await loadQueue(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleClearQueue = async () => {
    try {
      if (lineId) {
        await Promise.all(notificationQueueItems.map((item) => api.deleteNotificationQueueItem(item.id)))
        setQueueItems((current) => current.filter((item) => item.line_id !== lineId))
      } else {
        await api.clearNotificationQueue()
        setQueueItems([])
      }
      await loadQueue(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleLogTypeChange = (value: string) => {
    setLogType(value)
    setLogPage(0)
  }

  const handleLogStatusChange = (value: string) => {
    setLogStatus(value)
    setLogPage(0)
  }

  const handleLogLineIdChange = (value: string) => {
    setLogLineId(value)
    setLogPage(0)
  }

  const handleLogDateRangeChange = (startDate: string, endDate: string) => {
    setLogStartDate(startDate)
    setLogEndDate(endDate)
    setLogPage(0)
  }

  const handleLogQueryChange = (value: string) => {
    setLogQuery(value)
    setLogPage(0)
  }

  if (loading) {
    return (
      <Box display="flex" justifyContent="center" alignItems="center" minHeight="60vh">
        <CircularProgress />
      </Box>
    )
  }

  return (
    <Box>
      <Box display="flex" alignItems="center" gap={1} mb={2} flexWrap="wrap">
        <Typography variant={embedded ? 'subtitle1' : 'h5'} fontWeight={700}>{embedded ? '线路通知' : '通知中心'}</Typography>
        <NotificationQueueIndicator
          items={notificationQueueItems}
          open={queueOpen}
          onOpen={() => setQueueOpen(true)}
          onClose={() => setQueueOpen(false)}
          onRetry={(id) => void handleRetryQueueItem(id)}
          onDelete={(id) => void handleDeleteQueueItem(id)}
          onRetryAll={() => void handleRetryAllQueue()}
          onDeleteAll={() => void handleClearQueue()}
        />
      </Box>

      {lineId && (
        <Alert severity="info" sx={{ mb: 2 }}>
          转发日志与转发规则仅显示当前线路；转发通道由整台设备共享，配置一次后可供所有线路的规则复用。
        </Alert>
      )}

      <ErrorSnackbar error={error} onClose={() => setError(null)} />
      <Snackbar
        open={!!success}
        autoHideDuration={3000}
        resumeHideDuration={3000}
        onClose={() => setSuccess(null)}
        anchorOrigin={{ vertical: 'top', horizontal: 'center' }}
      >
        <Alert severity="info" variant="filled" onClose={() => setSuccess(null)}>{success}</Alert>
      </Snackbar>

      <Box sx={{ borderBottom: 1, borderColor: 'divider', mb: 2 }}>
        <Tabs value={tab} onChange={(_, value: number) => setTab(value)} variant="scrollable" scrollButtons="auto">
          <Tab label="转发日志" />
          <Tab label="转发规则" />
          <Tab label={lineId ? '全局转发通道' : '转发通道'} />
        </Tabs>
      </Box>

      {tab === 0 && (
        <NotificationLogsTab
          logs={logs}
          logTotal={logTotal}
          logsLoading={logsLoading}
          logType={logType}
          logStatus={logStatus}
          logLineId={logLineId}
          lineOptions={notificationLineOptions}
          logStartDate={logStartDate}
          logEndDate={logEndDate}
          logCleanup={config.log_cleanup}
          cleanupSaving={cleanupSaving}
          logQuery={logQuery}
          logPage={logPage}
          logPageSize={LOG_PAGE_SIZE}
          onLogTypeChange={handleLogTypeChange}
          onLogStatusChange={handleLogStatusChange}
          onLogLineIdChange={handleLogLineIdChange}
          onLogDateRangeChange={handleLogDateRangeChange}
          onLogQueryChange={handleLogQueryChange}
          onLogPageChange={setLogPage}
          onClearLogs={(filters) => void handleClearLogs(filters)}
          onSaveLogCleanup={(logCleanup) => void handleSaveLogCleanup(logCleanup)}
          fixedLineId={lineId}
          embedded={embedded}
        />
      )}
      {tab === 1 && (
        <NotificationRulesTab
          config={visibleConfig}
          smsChannels={smsChannels}
          selectedEventType={selectedEventType}
          saving={saving}
          onSelectedEventTypeChange={setSelectedEventType}
          onAddRule={handleAddRule}
          onDeleteRule={handleDeleteRule}
          onPatchRule={patchRule}
          onSave={() => void handleSave()}
          eventTypes={lineId ? LINE_EVENT_TYPES : undefined}
          fixedLineId={lineId}
          embedded={embedded}
        />
      )}
      {tab === 2 && (
        <NotificationChannelsTab
          config={config}
          selectedChannel={selectedChannel}
          saving={saving}
          testing={testing}
          onSelectChannel={setSelectedChannelId}
          onAddChannel={handleAddChannel}
          onDeleteChannel={handleDeleteChannel}
          onPatchChannel={patchChannel}
          onPatchChannelConfig={patchChannelConfig}
          onSave={() => void handleSave()}
          onTest={() => void handleTest()}
          embedded={embedded}
        />
      )}
    </Box>
  )
}
