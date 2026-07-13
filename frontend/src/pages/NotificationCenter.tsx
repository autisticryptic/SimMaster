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

type NotificationLogClearFilters = {
  type: string
  status: string
  start_date: string
  end_date: string
}

export default function NotificationCenterPage() {
  const { refreshInterval, refreshKey } = useRefreshInterval()
  const [tab, setTab] = useState(0)
  const [config, setConfig] = useState<NotificationConfig>(() => createDefaultConfig())
  const [selectedChannelId, setSelectedChannelId] = useState<string>('')
  const [selectedEventType, setSelectedEventType] = useState<NotificationEventType>('sms')
  const [logs, setLogs] = useState<NotificationLogEntry[]>([])
  const [logTotal, setLogTotal] = useState(0)
  const [logType, setLogType] = useState('')
  const [logStatus, setLogStatus] = useState('')
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
  const logsLoadingRef = useRef(false)
  const lastRefreshKeyRef = useRef(refreshKey)

  const selectedChannel = useMemo(
    () => config.channels.find((channel) => channel.id === selectedChannelId) ?? config.channels[0],
    [config.channels, selectedChannelId],
  )
  const notificationQueueItems = queueItems

  const loadConfig = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const response = await api.getNotificationConfig()
      const next = normalizeConfig(response.data)
      setConfig(next)
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
  }, [logEndDate, logPage, logQuery, logStartDate, logStatus, logType])

  const loadQueue = useCallback(async (silent = true) => {
    try {
      const response = await api.getNotificationQueue({ limit: 100 })
      const items = response.data?.items ?? []
      setQueueItems(items.map((item) => ({
          id: item.id,
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
  }, [])

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
      rules: prev.rules.map((rule) => rule.id === id ? { ...rule, ...patch } : rule),
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
        createRule(
          selectedEventType,
          prev.channels.filter((channel) => channel.enabled).map((channel) => channel.id),
        ),
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
      const response = await api.clearNotificationLogs(filters)
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
      await api.retryAllNotificationQueue()
      await loadQueue(false)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }

  const handleClearQueue = async () => {
    try {
      await api.clearNotificationQueue()
      setQueueItems([])
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
        <Typography variant="h5" fontWeight={700}>通知中心</Typography>
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
          <Tab label="转发通道" />
        </Tabs>
      </Box>

      {tab === 0 && (
        <NotificationLogsTab
          logs={logs}
          logTotal={logTotal}
          logsLoading={logsLoading}
          logType={logType}
          logStatus={logStatus}
          logStartDate={logStartDate}
          logEndDate={logEndDate}
          logCleanup={config.log_cleanup}
          cleanupSaving={cleanupSaving}
          logQuery={logQuery}
          logPage={logPage}
          logPageSize={LOG_PAGE_SIZE}
          onLogTypeChange={handleLogTypeChange}
          onLogStatusChange={handleLogStatusChange}
          onLogDateRangeChange={handleLogDateRangeChange}
          onLogQueryChange={handleLogQueryChange}
          onLogPageChange={setLogPage}
          onClearLogs={(filters) => void handleClearLogs(filters)}
          onSaveLogCleanup={(logCleanup) => void handleSaveLogCleanup(logCleanup)}
        />
      )}
      {tab === 1 && (
        <NotificationRulesTab
          config={config}
          selectedEventType={selectedEventType}
          saving={saving}
          onSelectedEventTypeChange={setSelectedEventType}
          onAddRule={handleAddRule}
          onDeleteRule={handleDeleteRule}
          onPatchRule={patchRule}
          onSave={() => void handleSave()}
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
        />
      )}
    </Box>
  )
}
