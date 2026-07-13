import { useState, useEffect, useRef, useCallback, useMemo, type ChangeEvent, type KeyboardEvent, type MouseEvent, type ReactNode } from 'react'
import {
  Box,
  Card,
  CardContent,
  Typography,
  Button,
  TextField,
  List,
  ListItemText,
  ListItemButton,
  Alert,
  CircularProgress,
  Chip,
  IconButton,
  Dialog,
  DialogTitle,
  DialogContent,
  DialogActions,
  Divider,
  Paper,
  Badge,
  Avatar,
  Snackbar,
  useMediaQuery,
  InputAdornment,
  Checkbox,
  Tooltip,
} from '@mui/material'
import type { Theme } from '@mui/material/styles'
import {
  Sms as SmsIcon,
  Send,
  Refresh,
  Person,
  ArrowBack,
  Add,
  Checklist,
  Delete,
  DeleteOutline,
  SelectAll,
  Close,
  Search,
} from '@mui/icons-material'
import { api, type SmsMessage, type SmsStats } from '../api/current'

interface ConversationGroup {
  phoneNumber: string
  messages: SmsMessage[]
  lastMessage: SmsMessage
  unreadCount: number
}

type ConversationSearchResult = ConversationGroup & {
  matchedMessage: SmsMessage | null
}

type DeleteTarget =
  | { type: 'batch' }
  | { type: 'conversation'; phoneNumber: string; messageCount: number }
  | { type: 'message'; message: SmsMessage }

function parseSmsTimestamp(timestamp: string): Date | null {
  const normalized = timestamp.includes(' ') ? timestamp.replace(' ', 'T') : timestamp
  const date = new Date(normalized)
  return Number.isNaN(date.getTime()) ? null : date
}

function smsTimestampMillis(timestamp: string): number {
  return parseSmsTimestamp(timestamp)?.getTime() ?? 0
}

function compareSmsChronological(a: SmsMessage, b: SmsMessage): number {
  return smsTimestampMillis(a.timestamp) - smsTimestampMillis(b.timestamp) || a.id - b.id
}

function compareSmsNewestFirst(a: SmsMessage, b: SmsMessage): number {
  return smsTimestampMillis(b.timestamp) - smsTimestampMillis(a.timestamp) || b.id - a.id
}

function buildConversations(msgs: SmsMessage[]): ConversationGroup[] {
  const groups = new Map<string, SmsMessage[]>()

  msgs.forEach((msg) => {
    const key = msg.phone_number
    if (!groups.has(key)) {
      groups.set(key, [])
    }
    groups.get(key)?.push(msg)
  })

  const conversationList: ConversationGroup[] = []
  groups.forEach((groupMessages, phoneNumber) => {
    groupMessages.sort(compareSmsNewestFirst)
    conversationList.push({
      phoneNumber,
      messages: groupMessages,
      lastMessage: groupMessages[0],
      unreadCount: groupMessages.filter((m) => m.direction === 'incoming' && m.status === 'received').length,
    })
  })

  conversationList.sort((a, b) => compareSmsNewestFirst(a.lastMessage, b.lastMessage))

  return conversationList
}

function includesSearchText(value: string, query: string) {
  return value.toLocaleLowerCase().includes(query.toLocaleLowerCase())
}

function renderHighlightedText(text: string, query: string): ReactNode {
  const trimmedQuery = query.trim()
  if (!trimmedQuery) {
    return text
  }

  const lowerText = text.toLocaleLowerCase()
  const lowerQuery = trimmedQuery.toLocaleLowerCase()
  const nodes: ReactNode[] = []
  let cursor = 0
  let matchIndex = lowerText.indexOf(lowerQuery)

  while (matchIndex !== -1) {
    if (matchIndex > cursor) {
      nodes.push(text.slice(cursor, matchIndex))
    }
    const end = matchIndex + trimmedQuery.length
    nodes.push(
      <Box
        key={`${matchIndex}-${end}`}
        component="mark"
        sx={{
          px: 0.25,
          borderRadius: 0.5,
          bgcolor: '#1296DB',
          color: 'common.white',
        }}
      >
        {text.slice(matchIndex, end)}
      </Box>,
    )
    cursor = end
    matchIndex = lowerText.indexOf(lowerQuery, cursor)
  }

  if (cursor < text.length) {
    nodes.push(text.slice(cursor))
  }

  return nodes
}

export default function SMSPage() {
  const isMobile = useMediaQuery<Theme>((theme: Theme) => theme.breakpoints.down('md'))

  const [messages, setMessages] = useState<SmsMessage[]>([])
  const [stats, setStats] = useState<SmsStats | null>(null)
  const [loading, setLoading] = useState(false)
  const [sendLoading, setSendLoading] = useState(false)
  const [deleteLoading, setDeleteLoading] = useState(false)
  const [phoneNumber, setPhoneNumber] = useState('')
  const [content, setContent] = useState('')
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [newChatDialogOpen, setNewChatDialogOpen] = useState(false)
  const [newChatNumber, setNewChatNumber] = useState('')

  // 对话状态
  const [conversations, setConversations] = useState<ConversationGroup[]>([])
  const [selectedConversation, setSelectedConversation] = useState<string | null>(null)
  const [conversationMessages, setConversationMessages] = useState<SmsMessage[]>([])
  const [conversationLoading, setConversationLoading] = useState(false)
  const [searchQuery, setSearchQuery] = useState('')

  // 批量管理状态
  const [batchMode, setBatchMode] = useState(false)
  const [selectedConversationPhones, setSelectedConversationPhones] = useState<Set<string>>(() => new Set())
  const [selectedMessageIds, setSelectedMessageIds] = useState<Set<number>>(() => new Set())
  const [deleteTarget, setDeleteTarget] = useState<DeleteTarget | null>(null)

  // 聊天区域滚动引用
  const chatEndRef = useRef<HTMLDivElement>(null)
  // 输入框焦点状态 - 有焦点时暂停刷新避免失焦
  const inputFocusedRef = useRef(false)

  const scrollToBottom = useCallback(() => {
    chatEndRef.current?.scrollIntoView({ behavior: 'smooth' })
  }, [])

  const scrollToMessage = useCallback((messageId: number) => {
    const target = document.getElementById(`sms-message-${messageId}`)
    if (target) {
      target.scrollIntoView({ behavior: 'smooth', block: 'center' })
      return
    }
    scrollToBottom()
  }, [scrollToBottom])

  const fetchMessages = useCallback(async (isBackground = false) => {
    if (!isBackground) {
      setLoading(true)
      setError(null)
    }
    try {
      const response = await api.getSmsList({ limit: 1000, offset: 0 })
      if (response.status === 'ok' && response.data) {
        setMessages(response.data.messages)
        setConversations(buildConversations(response.data.messages))
      } else {
        if (!isBackground) setError(response.message)
      }
    } catch (err) {
      if (!isBackground) {
        setError(err instanceof Error ? err.message : String(err))
      } else {
        console.warn('Background SMS fetch warning:', err)
      }
    } finally {
      if (!isBackground) setLoading(false)
    }
  }, [])

  const fetchConversation = useCallback(async (phone: string, scrollTargetId?: number) => {
    setConversationLoading(true)
    try {
      const response = await api.getSmsConversation({ phone_number: phone, limit: 1000 })
      if (response.status === 'ok' && response.data) {
        const sorted = [...response.data.messages].sort(compareSmsChronological)
        setConversationMessages(sorted)
        setTimeout(() => {
          if (scrollTargetId !== undefined) {
            scrollToMessage(scrollTargetId)
          } else {
            scrollToBottom()
          }
        }, 100)
      }
    } catch {
      const localMsgs = messages.filter((m) => m.phone_number === phone)
      const sorted = [...localMsgs].sort(compareSmsChronological)
      setConversationMessages(sorted)
      setTimeout(() => {
        if (scrollTargetId !== undefined) {
          scrollToMessage(scrollTargetId)
        } else {
          scrollToBottom()
        }
      }, 100)
    } finally {
      setConversationLoading(false)
    }
  }, [messages, scrollToBottom, scrollToMessage])

  const fetchStats = useCallback(async () => {
    try {
      const response = await api.getSmsStats()
      if (response.status === 'ok' && response.data) {
        setStats(response.data)
      }
    } catch (err) {
      console.error('获取短信统计失败:', err)
    }
  }, [])

  useEffect(() => {
    void fetchMessages(false)
    void fetchStats()
    const interval = setInterval(() => {
      if (inputFocusedRef.current) {
        return
      }
      void fetchMessages(true)
      void fetchStats()
    }, 10000)
    return () => clearInterval(interval)
  }, [fetchMessages, fetchStats])

  const messageById = useMemo(() => {
    const map = new Map<number, SmsMessage>()
    messages.forEach((msg) => map.set(msg.id, msg))
    conversationMessages.forEach((msg) => map.set(msg.id, msg))
    return map
  }, [messages, conversationMessages])

  const searchTerm = searchQuery.trim()

  const visibleConversations = useMemo<ConversationSearchResult[]>(() => {
    if (!searchTerm) {
      return conversations.map((conv) => ({ ...conv, matchedMessage: null }))
    }

    return conversations
      .map((conv) => {
        const phoneMatched = includesSearchText(conv.phoneNumber, searchTerm)
        const matchedMessage = conv.messages.find((msg) => includesSearchText(msg.content, searchTerm)) ?? null

        if (!phoneMatched && !matchedMessage) {
          return null
        }

        return {
          ...conv,
          matchedMessage,
        }
      })
      .filter((conv): conv is ConversationSearchResult => conv !== null)
  }, [conversations, searchTerm])

  const batchSelection = useMemo(() => {
    const visiblePhones = new Set(visibleConversations.map((conv) => conv.phoneNumber))
    const phoneNumbers = Array.from(selectedConversationPhones).filter((phone) => visiblePhones.has(phone))
    const phoneNumberSet = new Set(phoneNumbers)
    const selectedConversationNumbers = new Set(phoneNumbers)
    let messageCount = 0

    visibleConversations.forEach((conv) => {
      if (phoneNumberSet.has(conv.phoneNumber)) {
        messageCount += conv.messages.length
      }
    })

    const ids = Array.from(selectedMessageIds).filter((id) => {
      const msg = messageById.get(id)
      if (!msg || phoneNumberSet.has(msg.phone_number)) {
        return false
      }
      selectedConversationNumbers.add(msg.phone_number)
      messageCount += 1
      return true
    })

    return {
      ids,
      phoneNumbers,
      conversationCount: selectedConversationNumbers.size,
      messageCount,
    }
  }, [visibleConversations, messageById, selectedConversationPhones, selectedMessageIds])

  const hasBatchSelection = batchSelection.messageCount > 0
  const batchSelectionText = `已选 ${batchSelection.conversationCount} 个对话共 ${batchSelection.messageCount}条短信`
  const smsStats = stats ?? { total: 0, incoming: 0, outgoing: 0, pushed: 0, push_attempted: 0 }
  const pushCount = smsStats.pushed ?? 0
  const pushAttemptedCount = smsStats.push_attempted ?? 0
  const allConversationsSelected = visibleConversations.length > 0
    && visibleConversations.every((conv) => selectedConversationPhones.has(conv.phoneNumber))
  const currentMessagesSomeSelected = conversationMessages.some(
    (msg) => selectedConversationPhones.has(msg.phone_number) || selectedMessageIds.has(msg.id),
  )
  const currentMessagesAllSelected = conversationMessages.length > 0
    && conversationMessages.every((msg) => selectedConversationPhones.has(msg.phone_number) || selectedMessageIds.has(msg.id))

  const resetBatchSelection = () => {
    setSelectedConversationPhones(new Set())
    setSelectedMessageIds(new Set())
  }

  const handleEnterBatchMode = () => {
    setBatchMode(true)
  }

  const handleExitBatchMode = () => {
    setBatchMode(false)
    resetBatchSelection()
  }

  const handleSelectConversation = (phone: string, scrollTargetId?: number) => {
    setSelectedConversation(phone)
    setPhoneNumber(phone)
    void fetchConversation(phone, scrollTargetId)
  }

  const handleBackToList = () => {
    setSelectedConversation(null)
    setConversationMessages([])
  }

  const handleStartNewChat = () => {
    if (!newChatNumber.trim()) {
      setError('请输入电话号码')
      return
    }
    setNewChatDialogOpen(false)
    setSelectedConversation(newChatNumber)
    setPhoneNumber(newChatNumber)
    setConversationMessages([])
    setNewChatNumber('')
  }

  const handleSend = async () => {
    if (!phoneNumber.trim()) {
      setError('请输入电话号码')
      return
    }
    if (!content.trim()) {
      setError('请输入短信内容')
      return
    }

    setSendLoading(true)
    setError(null)
    setSuccess(null)

    try {
      const response = await api.sendSms(phoneNumber, content)
      if (response.status === 'ok') {
        setSuccess(`短信已发送到 ${phoneNumber}`)
        setContent('')
        setTimeout(() => {
          void fetchMessages()
          void fetchStats()
          if (selectedConversation) {
            void fetchConversation(selectedConversation)
          }
        }, 1000)
      } else {
        setError(response.message)
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSendLoading(false)
    }
  }

  const toggleConversationSelection = (phone: string) => {
    const selected = selectedConversationPhones.has(phone)
    setSelectedConversationPhones((prev) => {
      const next = new Set(prev)
      if (selected) {
        next.delete(phone)
      } else {
        next.add(phone)
      }
      return next
    })
    setSelectedMessageIds((prev) => {
      const next = new Set(prev)
      messageById.forEach((msg) => {
        if (msg.phone_number === phone) {
          next.delete(msg.id)
        }
      })
      return next
    })
  }

  const toggleAllConversations = () => {
    if (allConversationsSelected) {
      resetBatchSelection()
      return
    }
    setSelectedConversationPhones(new Set(visibleConversations.map((conv) => conv.phoneNumber)))
    setSelectedMessageIds(new Set())
  }

  const toggleMessageSelection = (msg: SmsMessage) => {
    if (selectedConversationPhones.has(msg.phone_number)) {
      const relatedMessages = Array.from(messageById.values()).filter(
        (item) => item.phone_number === msg.phone_number,
      )
      setSelectedConversationPhones((prev) => {
        const next = new Set(prev)
        next.delete(msg.phone_number)
        return next
      })
      setSelectedMessageIds((prev) => {
        const next = new Set(prev)
        relatedMessages.forEach((item) => {
          if (item.id !== msg.id) {
            next.add(item.id)
          }
        })
        next.delete(msg.id)
        return next
      })
      return
    }

    setSelectedMessageIds((prev) => {
      const next = new Set(prev)
      if (next.has(msg.id)) {
        next.delete(msg.id)
      } else {
        next.add(msg.id)
      }
      return next
    })
  }

  const toggleAllCurrentMessages = () => {
    if (!selectedConversation) {
      return
    }

    if (currentMessagesAllSelected) {
      setSelectedConversationPhones((prev) => {
        const next = new Set(prev)
        next.delete(selectedConversation)
        return next
      })
      setSelectedMessageIds((prev) => {
        const next = new Set(prev)
        conversationMessages.forEach((msg) => next.delete(msg.id))
        return next
      })
      return
    }

    setSelectedConversationPhones((prev) => {
      const next = new Set(prev)
      next.delete(selectedConversation)
      return next
    })
    setSelectedMessageIds((prev) => {
      const next = new Set(prev)
      conversationMessages.forEach((msg) => next.add(msg.id))
      return next
    })
  }

  const isMessageSelected = (msg: SmsMessage) => (
    selectedConversationPhones.has(msg.phone_number) || selectedMessageIds.has(msg.id)
  )

  const getConversationMessageSelectionState = (conv: ConversationGroup) => {
    if (selectedConversationPhones.has(conv.phoneNumber)) {
      return { checked: true, indeterminate: false }
    }

    const selectedCount = conv.messages.filter((msg) => selectedMessageIds.has(msg.id)).length
    return {
      checked: selectedCount > 0 && selectedCount === conv.messages.length,
      indeterminate: selectedCount > 0 && selectedCount < conv.messages.length,
    }
  }

  const requestConversationDelete = (
    event: MouseEvent<HTMLButtonElement>,
    conv: ConversationGroup,
  ) => {
    event.stopPropagation()
    setDeleteTarget({
      type: 'conversation',
      phoneNumber: conv.phoneNumber,
      messageCount: conv.messages.length,
    })
  }

  const requestMessageDelete = (
    event: MouseEvent<HTMLButtonElement>,
    message: SmsMessage,
  ) => {
    event.stopPropagation()
    setDeleteTarget({ type: 'message', message })
  }

  const refreshAfterDelete = (clearConversation: boolean) => {
    void fetchMessages()
    void fetchStats()
    if (clearConversation) {
      setSelectedConversation(null)
      setConversationMessages([])
      return
    }
    if (selectedConversation) {
      void fetchConversation(selectedConversation)
    }
  }

  const handleConfirmDelete = async () => {
    if (!deleteTarget) {
      return
    }

    setDeleteLoading(true)
    setError(null)
    setSuccess(null)

    try {
      let deleted = 0
      let clearCurrentConversation = false

      if (deleteTarget.type === 'batch') {
        const response = await api.deleteSmsBatch({
          ids: batchSelection.ids,
          phone_numbers: batchSelection.phoneNumbers,
        })
        deleted = response.data?.deleted ?? batchSelection.messageCount
        clearCurrentConversation = Boolean(
          selectedConversation
          && (
            batchSelection.phoneNumbers.includes(selectedConversation)
            || (
              conversationMessages.length > 0
              && conversationMessages.every((msg) => batchSelection.ids.includes(msg.id))
            )
          ),
        )
        setSuccess(`已删除 ${deleted} 条短信`)
        handleExitBatchMode()
      } else if (deleteTarget.type === 'conversation') {
        const response = await api.deleteSmsConversation(deleteTarget.phoneNumber)
        deleted = response.data?.deleted ?? deleteTarget.messageCount
        clearCurrentConversation = selectedConversation === deleteTarget.phoneNumber
        setSuccess(`已删除对话 ${deleteTarget.phoneNumber}（${deleted} 条短信）`)
      } else {
        const response = await api.deleteSmsMessage(deleteTarget.message.id)
        deleted = response.data?.deleted ?? 1
        clearCurrentConversation = selectedConversation === deleteTarget.message.phone_number
          && conversationMessages.length <= 1
        setSuccess(deleted > 0 ? '短信已删除' : '短信不存在或已被删除')
      }

      setDeleteTarget(null)
      refreshAfterDelete(clearCurrentConversation)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setDeleteLoading(false)
    }
  }

  const formatTime = (timestamp: string) => {
    try {
      const date = parseSmsTimestamp(timestamp)
      if (!date) return timestamp
      const now = new Date()
      const isToday = date.toDateString() === now.toDateString()
      if (isToday) {
        return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
      }
      return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
    } catch {
      return timestamp
    }
  }

  const formatShortTime = (timestamp: string) => {
    try {
      const date = parseSmsTimestamp(timestamp)
      if (!date) return timestamp
      const now = new Date()
      const isToday = date.toDateString() === now.toDateString()
      if (isToday) {
        return date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
      }
      return date.toLocaleDateString('zh-CN', { month: '2-digit', day: '2-digit' })
    } catch {
      return timestamp
    }
  }

  const deleteDialogTitle = deleteTarget?.type === 'batch'
    ? '确认批量删除'
    : deleteTarget?.type === 'conversation'
      ? '确认删除对话'
      : '确认删除短信'

  const deleteDialogContent = (() => {
    if (!deleteTarget) {
      return ''
    }
    if (deleteTarget.type === 'batch') {
      return `${batchSelectionText}，确定要删除吗？此操作不可撤销。`
    }
    if (deleteTarget.type === 'conversation') {
      return `确定要删除与 ${deleteTarget.phoneNumber} 的对话及全部 ${deleteTarget.messageCount} 条短信吗？此操作不可撤销。`
    }
    return '确定要删除当前短信内容吗？此操作不可撤销。'
  })()

  const renderBatchSelectionBar = () => (
    batchMode && hasBatchSelection ? (
      <Box
        sx={{
          mx: 2,
          mb: 1,
          p: 1,
          borderRadius: 1,
          bgcolor: 'action.hover',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          gap: 1,
        }}
      >
        <Typography variant="body2" fontWeight={600}>
          {batchSelectionText}
        </Typography>
        <Button
          size="small"
          color="error"
          variant="contained"
          startIcon={<Delete />}
          onClick={() => setDeleteTarget({ type: 'batch' })}
          disabled={deleteLoading}
        >
          删除
        </Button>
      </Box>
    ) : null
  )

  const conversationListContent = (
    <Box sx={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
      <Box display="flex" gap={1} p={2} flexWrap="wrap">
        {/* <Paper sx={{ p: 1, flex: 1, minWidth: 60, textAlign: 'center' }}>
          <Typography variant="h6" color="primary" fontWeight={600}>{smsStats.total}</Typography>
          <Typography variant="caption" color="text.secondary">总计</Typography>
        </Paper> */}
        <Paper sx={{ p: 1, flex: 1, minWidth: 60, textAlign: 'center' }}>
          <Typography variant="h6" color="success.main" fontWeight={600}>{smsStats.incoming}</Typography>
          <Typography variant="caption" color="text.secondary">接收</Typography>
        </Paper>
        <Paper sx={{ p: 1, flex: 1, minWidth: 60, textAlign: 'center' }}>
          <Typography variant="h6" color="info.main" fontWeight={600}>{smsStats.outgoing}</Typography>
          <Typography variant="caption" color="text.secondary">发送</Typography>
        </Paper>
        <Paper sx={{ p: 1, flex: 1, minWidth: 60, textAlign: 'center' }}>
          <Tooltip title={`推送成功 ${pushCount} 条，尝试推送 ${pushAttemptedCount} 条`}>
            <Typography variant="h6" component="div">
              <Box component="span" sx={{ color: 'warning.main', fontWeight: 600 }}>
                {pushCount}
              </Box>
              <Box component="span" sx={{ mx: 0.5, color: 'text.secondary', fontWeight: 400 }}>
                /
              </Box>
              <Box component="span" sx={{ color: 'success.main', fontWeight: 600 }}>
                {pushAttemptedCount}
              </Box>
            </Typography>
          </Tooltip>
          <Typography variant="caption" color="text.secondary">推送</Typography>
        </Paper>
      </Box>

      <Box display="flex" justifyContent="space-between" alignItems="center" px={2} pb={1} gap={1}>
        <Typography variant="subtitle1" fontWeight={600}>
          对话 ({visibleConversations.length})
        </Typography>
        <Box display="flex" gap={0.5} alignItems="center">
          {batchMode ? (
            <>
              <Button
                size="small"
                startIcon={<SelectAll />}
                onClick={toggleAllConversations}
                disabled={visibleConversations.length === 0}
              >
                {allConversationsSelected ? '取消全选' : '全选对话'}
              </Button>
              <Tooltip title="退出批量管理">
                <IconButton size="small" onClick={handleExitBatchMode}>
                  <Close />
                </IconButton>
              </Tooltip>
            </>
          ) : (
            <>
              <Tooltip title="新建对话">
                <IconButton size="small" color="primary" onClick={() => setNewChatDialogOpen(true)}>
                  <Add />
                </IconButton>
              </Tooltip>
              <Tooltip title="刷新">
                <IconButton size="small" color="primary" onClick={() => void fetchMessages()} disabled={loading}>
                  <Refresh />
                </IconButton>
              </Tooltip>
              <Tooltip title="批量管理">
                <IconButton size="small" color="primary" onClick={handleEnterBatchMode}>
                  <Checklist />
                </IconButton>
              </Tooltip>
            </>
          )}
        </Box>
      </Box>

      <Box px={2} pb={1}>
        <TextField
          fullWidth
          size="small"
          value={searchQuery}
          onChange={(event: ChangeEvent<HTMLInputElement>) => setSearchQuery(event.target.value)}
          onFocus={() => { inputFocusedRef.current = true }}
          onBlur={() => { inputFocusedRef.current = false }}
          placeholder="搜索联系人或内容..."
          slotProps={{
            input: {
              startAdornment: (
                <InputAdornment position="start">
                  <Search fontSize="small" />
                </InputAdornment>
              ),
              endAdornment: searchQuery ? (
                <InputAdornment position="end">
                  <IconButton
                    size="small"
                    aria-label="清空搜索"
                    onClick={() => setSearchQuery('')}
                    edge="end"
                  >
                    <Close fontSize="small" />
                  </IconButton>
                </InputAdornment>
              ) : null,
            },
          }}
          sx={{
            '& .MuiOutlinedInput-root': {
              bgcolor: 'transparent',
              borderRadius: 1.5,
              '& .MuiOutlinedInput-notchedOutline': {
                borderColor: 'divider',
              },
              '&:hover .MuiOutlinedInput-notchedOutline': {
                borderColor: 'text.disabled',
              },
              '&.Mui-focused .MuiOutlinedInput-notchedOutline': {
                borderColor: '#1296DB',
              },
            },
          }}
        />
      </Box>

      {renderBatchSelectionBar()}

      <Divider />

      {loading && conversations.length === 0 ? (
        <Box display="flex" justifyContent="center" py={4}><CircularProgress /></Box>
      ) : conversations.length === 0 ? (
        <Box p={2}><Alert severity="info">暂无对话，点击 + 开始新对话</Alert></Box>
      ) : visibleConversations.length === 0 ? (
        <Box p={2}><Alert severity="info">未找到匹配的对话</Alert></Box>
      ) : (
        <List sx={{ flex: 1, overflow: 'auto' }}>
          {visibleConversations.map((conv, idx) => {
            const selectionState = getConversationMessageSelectionState(conv)
            const displayMessage = conv.matchedMessage ?? conv.lastMessage
            return (
              <Box
                key={conv.phoneNumber}
                sx={{
                  '&:hover .conversation-delete, &:focus-within .conversation-delete': {
                    opacity: 1,
                  },
                }}
              >
                <ListItemButton
                  onClick={() => handleSelectConversation(conv.phoneNumber, conv.matchedMessage?.id)}
                  selected={selectedConversation === conv.phoneNumber}
                  sx={{ gap: 1 }}
                >
                  {batchMode && (
                    <Checkbox
                      edge="start"
                      size="small"
                      checked={selectionState.checked}
                      indeterminate={selectionState.indeterminate}
                      onClick={(event) => event.stopPropagation()}
                      onChange={() => toggleConversationSelection(conv.phoneNumber)}
                      inputProps={{ 'aria-label': `选择对话 ${conv.phoneNumber}` }}
                    />
                  )}
                  <Avatar sx={{ bgcolor: 'primary.light' }}><Person /></Avatar>
                  <ListItemText
                    primary={
                      <Box display="flex" alignItems="center" gap={1}>
                        <Typography fontWeight={600}>
                          {renderHighlightedText(conv.phoneNumber, searchTerm)}
                        </Typography>
                        <Badge badgeContent={conv.messages.length} color="primary" max={99} />
                      </Box>
                    }
                    secondary={
                      <Typography variant="body2" color="text.secondary" noWrap sx={{ maxWidth: 180 }}>
                        {displayMessage.direction === 'outgoing' ? '你: ' : ''}
                        {renderHighlightedText(displayMessage.content, searchTerm)}
                      </Typography>
                    }
                  />
                  <Typography variant="caption" color="text.secondary" sx={{ minWidth: 44, textAlign: 'right' }}>
                    {formatShortTime(displayMessage.timestamp)}
                  </Typography>
                  {!batchMode && (
                    <Tooltip title="删除对话">
                      <IconButton
                        className="conversation-delete"
                        size="small"
                        onClick={(event) => requestConversationDelete(event, conv)}
                        sx={{
                          opacity: 0,
                          color: 'text.secondary',
                          transition: (theme: Theme) => theme.transitions.create(['opacity', 'color'], {
                            duration: theme.transitions.duration.shortest,
                          }),
                          '&:hover': {
                            color: 'error.main',
                            bgcolor: 'rgba(211, 47, 47, 0.08)',
                          },
                        }}
                      >
                        <DeleteOutline fontSize="small" />
                      </IconButton>
                    </Tooltip>
                  )}
                </ListItemButton>
                {idx < visibleConversations.length - 1 && <Divider />}
              </Box>
            )
          })}
        </List>
      )}
    </Box>
  )

  const chatAreaContent = (
    <Box sx={{ height: '100%', display: 'flex', flexDirection: 'column' }}>
      <Box
        sx={{
          p: 2,
          borderBottom: 1,
          borderColor: 'divider',
          display: 'flex',
          alignItems: 'center',
          gap: 1,
        }}
      >
        {isMobile && (
          <IconButton onClick={handleBackToList} edge="start">
            <ArrowBack />
          </IconButton>
        )}
        <Avatar sx={{ bgcolor: 'primary.main' }}><Person /></Avatar>
        <Typography variant="h6" fontWeight={600}>{selectedConversation}</Typography>
        {batchMode && conversationMessages.length > 0 && (
          <Box sx={{ ml: 'auto', display: 'flex', alignItems: 'center' }}>
            <Checkbox
              size="small"
              checked={currentMessagesAllSelected}
              indeterminate={currentMessagesSomeSelected && !currentMessagesAllSelected}
              onChange={toggleAllCurrentMessages}
              inputProps={{ 'aria-label': '全选当前对话短信' }}
            />
            <Typography variant="body2" color="text.secondary">全选短信</Typography>
          </Box>
        )}
      </Box>

      {isMobile && renderBatchSelectionBar()}

      <Box
        sx={{
          flex: 1,
          overflow: 'auto',
          p: 2,
          bgcolor: (theme: Theme) => theme.palette.mode === 'dark' ? 'grey.900' : 'grey.50',
        }}
      >
        {conversationLoading ? (
          <Box display="flex" justifyContent="center" py={4}><CircularProgress /></Box>
        ) : conversationMessages.length === 0 ? (
          <Box display="flex" justifyContent="center" alignItems="center" height="100%">
            <Typography color="text.secondary">开始发送第一条消息</Typography>
          </Box>
        ) : (
          <>
            {conversationMessages.map((msg, idx) => (
              <Box
                key={msg.id || idx}
                id={`sms-message-${msg.id}`}
                display="flex"
                justifyContent={msg.direction === 'outgoing' ? 'flex-end' : 'flex-start'}
                alignItems="center"
                gap={0.75}
                mb={1.5}
                onClick={batchMode ? () => toggleMessageSelection(msg) : undefined}
                sx={{
                  cursor: batchMode ? 'pointer' : 'default',
                  '&:hover .message-delete, &:focus-within .message-delete': {
                    opacity: 1,
                  },
                }}
              >
                {batchMode && (
                  <Checkbox
                    size="small"
                    checked={isMessageSelected(msg)}
                    onClick={(event) => event.stopPropagation()}
                    onChange={() => toggleMessageSelection(msg)}
                    inputProps={{ 'aria-label': '选择短信' }}
                  />
                )}
                <Paper
                  elevation={1}
                  sx={{
                    p: 1.5,
                    maxWidth: '75%',
                    bgcolor: msg.direction === 'outgoing'
                      ? 'primary.main'
                      : (theme: Theme) => theme.palette.mode === 'dark' ? 'grey.800' : 'white',
                    color: msg.direction === 'outgoing'
                      ? 'white'
                      : 'text.primary',
                    borderRadius: 2,
                    borderTopRightRadius: msg.direction === 'outgoing' ? 0 : 16,
                    borderTopLeftRadius: msg.direction === 'incoming' ? 0 : 16,
                  }}
                >
                  <Typography variant="body2" sx={{ wordBreak: 'break-word', whiteSpace: 'pre-wrap' }}>
                    {renderHighlightedText(msg.content, searchTerm)}
                  </Typography>
                  <Box
                    display="flex"
                    alignItems="center"
                    justifyContent="flex-end"
                    gap={0.5}
                    mt={0.5}
                  >
                    <Typography
                      variant="caption"
                      sx={{ opacity: 0.7 }}
                    >
                      {formatTime(msg.timestamp)}
                    </Typography>
                    {msg.direction === 'incoming' && msg.transport === 'vowifi_ims' && (
                      <Chip
                        label="WiFi Calling"
                        size="small"
                        sx={{
                          height: 16,
                          fontSize: '0.65rem',
                          bgcolor: '#2aae67',
                          color: 'white',
                          borderRadius: 0.5,
                          px: 0.5,
                          ml: 0.5,
                        }}
                      />
                    )}
                    {msg.direction === 'outgoing' && (
                      msg.status === 'sent' ? (
                        <>
                          <Chip label="已发送" size="small" sx={{ height: 16, fontSize: '0.65rem', bgcolor: 'rgba(255,255,255,0.2)', color: '#ffffff', mr: msg.transport === 'vowifi_ims' ? 0.5 : 0 }} />
                          {msg.transport === 'vowifi_ims' && (
                            <Chip
                              label="WiFi Calling"
                              size="small"
                              sx={{
                                height: 16,
                                fontSize: '0.65rem',
                                bgcolor: '#2aae67',
                                color: 'white',
                                borderRadius: 0.5,
                                px: 0.5,
                              }}
                            />
                          )}
                        </>
                      ) : msg.status === 'failed' ? (
                        <Chip label="失败" size="small" color="error" sx={{ height: 16, fontSize: '0.65rem' }} />
                      ) : null
                    )}
                  </Box>
                </Paper>
                {!batchMode && (
                  <Tooltip title="删除短信">
                    <IconButton
                      className="message-delete"
                      size="small"
                      onClick={(event) => requestMessageDelete(event, msg)}
                      sx={{
                        opacity: 0,
                        color: 'text.secondary',
                        transition: (theme: Theme) => theme.transitions.create(['opacity', 'color'], {
                          duration: theme.transitions.duration.shortest,
                        }),
                        '&:hover': {
                          color: 'error.main',
                          bgcolor: 'rgba(211, 47, 47, 0.08)',
                        },
                      }}
                    >
                      <DeleteOutline fontSize="small" />
                    </IconButton>
                  </Tooltip>
                )}
              </Box>
            ))}
            <div ref={chatEndRef} />
          </>
        )}
      </Box>

      <Box
        sx={{
          p: 2,
          borderTop: 1,
          borderColor: 'divider',
          bgcolor: 'background.paper',
        }}
      >
        <TextField
          fullWidth
          multiline
          maxRows={4}
          value={content}
          onChange={(e: ChangeEvent<HTMLInputElement>) => setContent(e.target.value)}
          placeholder="输入短信内容..."
          disabled={sendLoading}
          onFocus={() => { inputFocusedRef.current = true }}
          onBlur={() => { inputFocusedRef.current = false }}
          onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
            if (e.key === 'Enter' && !e.shiftKey) {
              e.preventDefault()
              void handleSend()
            }
          }}
          slotProps={{
            input: {
              endAdornment: (
                <InputAdornment position="end">
                  <IconButton
                    color="primary"
                    onClick={() => void handleSend()}
                    disabled={sendLoading || !content.trim()}
                  >
                    {sendLoading ? <CircularProgress size={24} /> : <Send />}
                  </IconButton>
                </InputAdornment>
              ),
            },
          }}
        />
        <Typography variant="caption" color="text.secondary" sx={{ mt: 0.5, display: 'block' }}>
          {content.length} 字符 | Enter 发送，Shift+Enter 换行
        </Typography>
      </Box>
    </Box>
  )

  const emptyStateContent = (
    <Box sx={{ height: '100%', display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', p: 4 }}>
      <SmsIcon sx={{ fontSize: 64, color: 'text.secondary', mb: 2 }} />
      <Typography variant="h6" color="text.secondary" gutterBottom>
        选择一个对话开始聊天
      </Typography>
      <Typography variant="body2" color="text.secondary">
        或点击左上角 + 开始新对话
      </Typography>
    </Box>
  )

  return (
    <Box sx={{ height: 'calc(100vh - 140px)', minHeight: 500 }}>
      <Box display="flex" alignItems="center" gap={1} mb={2}>
        <Typography variant="h5" fontWeight={700}>
          短信管理
        </Typography>
      </Box>

      <Snackbar open={!!error} autoHideDuration={4000} resumeHideDuration={3000} onClose={() => setError(null)} anchorOrigin={{ vertical: 'top', horizontal: 'center' }}>
        <Alert severity="error" onClose={() => setError(null)} variant="filled">{error}</Alert>
      </Snackbar>
      <Snackbar open={!!success} autoHideDuration={3000} resumeHideDuration={3000} onClose={() => setSuccess(null)} anchorOrigin={{ vertical: 'top', horizontal: 'center' }}>
        <Alert severity="success" onClose={() => setSuccess(null)} variant="filled">{success}</Alert>
      </Snackbar>

      <Card sx={{ height: 'calc(100% - 48px)' }}>
        <CardContent sx={{ height: '100%', p: 0, '&:last-child': { pb: 0 } }}>
          {isMobile ? (
            selectedConversation ? chatAreaContent : conversationListContent
          ) : (
            <Box display="flex" height="100%">
              <Box
                sx={{
                  width: 340,
                  borderRight: 1,
                  borderColor: 'divider',
                  flexShrink: 0,
                }}
              >
                {conversationListContent}
              </Box>
              <Box sx={{ flex: 1 }}>
                {selectedConversation ? chatAreaContent : emptyStateContent}
              </Box>
            </Box>
          )}
        </CardContent>
      </Card>

      <Dialog open={!!deleteTarget} onClose={() => !deleteLoading && setDeleteTarget(null)}>
        <DialogTitle>{deleteDialogTitle}</DialogTitle>
        <DialogContent>
          <Typography>{deleteDialogContent}</Typography>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setDeleteTarget(null)} disabled={deleteLoading}>取消</Button>
          <Button
            onClick={() => void handleConfirmDelete()}
            color="error"
            variant="contained"
            disabled={deleteLoading || (deleteTarget?.type === 'batch' && !hasBatchSelection)}
          >
            {deleteLoading ? '删除中...' : '确认删除'}
          </Button>
        </DialogActions>
      </Dialog>

      <Dialog open={newChatDialogOpen} onClose={() => setNewChatDialogOpen(false)}>
        <DialogTitle>新建对话</DialogTitle>
        <DialogContent>
          <TextField
            autoFocus
            fullWidth
            label="电话号码"
            value={newChatNumber}
            onChange={(e: ChangeEvent<HTMLInputElement>) => setNewChatNumber(e.target.value)}
            placeholder="输入收件人电话号码"
            sx={{ mt: 1 }}
            onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
              if (e.key === 'Enter') {
                handleStartNewChat()
              }
            }}
          />
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setNewChatDialogOpen(false)}>取消</Button>
          <Button onClick={handleStartNewChat} variant="contained">开始对话</Button>
        </DialogActions>
      </Dialog>
    </Box>
  )
}
