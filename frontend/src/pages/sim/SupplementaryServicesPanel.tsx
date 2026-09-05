import { useEffect, useRef, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Card,
  CardContent,
  Chip,
  CircularProgress,
  Divider,
  Stack,
  TextField,
  Typography,
} from '@mui/material'
import { Dialpad, SimCard as SimCardIcon } from '@mui/icons-material'
import { api, type UssdResponse, type VolteLineControlResponse } from '../../api/current'

type SupplementaryServicesPanelProps = {
  line: VolteLineControlResponse | null
}

const statusLabels: Record<string, string> = {
  final: '已完成',
  awaiting_input: '等待输入',
  terminated: '已终止',
  failed: '失败',
}

function statusColor(status: string): 'success' | 'warning' | 'error' | 'default' {
  if (status === 'final') return 'success'
  if (status === 'awaiting_input') return 'warning'
  if (status === 'failed') return 'error'
  return 'default'
}

export default function SupplementaryServicesPanel({ line }: SupplementaryServicesPanelProps) {
  const lineId = line?.modem.line_id ?? ''
  const isReader = line?.modem.line_kind === 'reader'
  const hasModem = Boolean(line && !isReader && line.modem.modem_path)
  const [code, setCode] = useState('')
  const [input, setInput] = useState('')
  const [sessionId, setSessionId] = useState<string | null>(null)
  const activeSessionRef = useRef<string | null>(null)
  const [response, setResponse] = useState<UssdResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    return () => {
      const activeSessionId = activeSessionRef.current
      activeSessionRef.current = null
      if (lineId && activeSessionId) {
        void api.cancelUssd(lineId, activeSessionId).catch(() => undefined)
      }
    }
  }, [lineId])

  useEffect(() => {
    activeSessionRef.current = null
    setCode('')
    setInput('')
    setSessionId(null)
    setResponse(null)
    setError(null)
  }, [lineId])

  const run = async (operation: () => Promise<{ data?: UssdResponse }>) => {
    setLoading(true)
    setError(null)
    try {
      const result = await operation()
      if (!result.data) throw new Error('未收到有效的 USSD/USSI 响应')
      setResponse(result.data)
      const nextSessionId = result.data.continueable ? result.data.session_id ?? null : null
      activeSessionRef.current = nextSessionId
      setSessionId(nextSessionId)
      if (!nextSessionId) setInput('')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }

  const handleStart = () => {
    if (!lineId || !hasModem || !code.trim() || loading) return
    void run(() => api.startUssd(lineId, code.trim()))
  }

  const handleContinue = () => {
    if (!lineId || !sessionId || !input.trim() || loading) return
    void run(() => api.continueUssd(lineId, sessionId, input.trim()))
  }

  const handleCancel = () => {
    if (!lineId || !sessionId || loading) return
    void run(() => api.cancelUssd(lineId, sessionId))
  }

  return (
    <Card variant="outlined">
      <CardContent>
        <Stack spacing={2}>
          <Box display="flex" alignItems="center" gap={1}>
            <Dialpad color="primary" />
            <Box minWidth={0}>
              <Typography variant="subtitle1" fontWeight={700}>补充业务</Typography>
              <Typography variant="caption" color="text.secondary">通过 USSD / USSI 查询和操作运营商补充业务</Typography>
            </Box>
            {line && <Chip size="small" variant="outlined" label={line.modem.line_id} sx={{ ml: 'auto', maxWidth: 180 }} />}
          </Box>

          {!line && <Alert severity="info">请先在左侧选择一条线路</Alert>}
          {line && isReader && (
            <Alert severity="info" icon={<SimCardIcon />}>
              独立读卡器只有 PC/SC 通道，没有蜂窝 modem / AT 通道，因此不能直接执行 USSD / USSI；请选择绑定了 modem 的线路。
            </Alert>
          )}
          {line && !isReader && !line.modem.present && (
            <Alert severity="warning">当前 modem 不在线，暂时无法发送 USSD / USSI。</Alert>
          )}
          {line && !isReader && !line.modem.modem_path && (
            <Alert severity="warning">当前线路没有可用的 modem，无法发送 USSD / USSI。</Alert>
          )}
          {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}

          <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems={{ sm: 'flex-start' }}>
            <TextField
              fullWidth
              size="small"
              label="服务码"
              placeholder="例如 *#06# 或 *123#"
              value={code}
              onChange={(event) => setCode(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') handleStart()
              }}
              disabled={!hasModem || loading}
              inputProps={{ maxLength: 40, inputMode: 'text' }}
            />
            <Button
              variant="contained"
              onClick={handleStart}
              disabled={!hasModem || !code.trim() || loading}
              sx={{ minWidth: { sm: 104 }, height: 40, flexShrink: 0 }}
            >
              {loading && !sessionId ? <CircularProgress size={18} color="inherit" /> : '发送'}
            </Button>
          </Stack>

          {response && (
            <Box sx={{ p: 1.5, border: 1, borderColor: 'divider', borderRadius: 1, bgcolor: 'action.hover' }}>
              <Box display="flex" alignItems="center" gap={1} mb={1}>
                <Typography variant="body2" fontWeight={700}>返回结果</Typography>
                <Chip size="small" color={statusColor(response.status)} variant="outlined" label={statusLabels[response.status] ?? response.status} />
              </Box>
              <Typography variant="body2" sx={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
                {response.text || '未收到文本结果'}
              </Typography>
              <Typography variant="caption" color="text.secondary" display="block" mt={1} sx={{ wordBreak: 'break-all' }}>
                {response.raw}
              </Typography>
            </Box>
          )}

          {sessionId && (
            <>
              <Divider />
              <Typography variant="body2" color="text.secondary">
                这是一个交互式 USSD / USSI 会话。会话期间会独占当前 modem，最多保留 5 分钟。
              </Typography>
              <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1} alignItems={{ sm: 'flex-start' }}>
                <TextField
                  fullWidth
                  size="small"
                  label="继续输入"
                  placeholder="例如 1 或 2"
                  value={input}
                  onChange={(event) => setInput(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') handleContinue()
                  }}
                  disabled={loading}
                  inputProps={{ maxLength: 182 }}
                />
                <Stack direction="row" spacing={1}>
                  <Button variant="contained" onClick={handleContinue} disabled={!input.trim() || loading} sx={{ minWidth: 88, height: 40 }}>
                    {loading ? <CircularProgress size={18} color="inherit" /> : '发送'}
                  </Button>
                  <Button color="error" variant="outlined" onClick={handleCancel} disabled={loading} sx={{ minWidth: 88, height: 40 }}>
                    取消
                  </Button>
                </Stack>
              </Stack>
            </>
          )}

          <Typography variant="caption" color="text.secondary">
            USSD / USSI 使用 modem 的 AT+CUSD 通道；发送命令后会等待异步 +CUSD 响应，不会把前置 OK 当作业务结果。它与 IMS bearer 相互独立。
          </Typography>
        </Stack>
      </CardContent>
    </Card>
  )
}
