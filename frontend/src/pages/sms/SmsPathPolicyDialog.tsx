import { useEffect, useState } from 'react'
import {
  Alert, Box, Button, CircularProgress, Dialog, DialogActions, DialogContent, DialogTitle,
  FormControl, FormControlLabel, IconButton, InputLabel, List, ListItem, ListItemText,
  MenuItem, Select, Switch, TextField, Tooltip, Typography,
} from '@mui/material'
import { ArrowDownward, ArrowUpward } from '@mui/icons-material'
import { api, type AccessPathKind, type SmsPathPolicy } from '../../api/current'

const pathLabels: Record<AccessPathKind, string> = {
  vowifi: 'VoWiFi（IMS over Wi-Fi）',
  volte: 'VoLTE（IMS over LTE）',
  cs: 'CS/基带短信',
}

type Props = { open: boolean; onClose: () => void }

export default function SmsPathPolicyDialog({ open, onClose }: Props) {
  const [policy, setPolicy] = useState<SmsPathPolicy | null>(null)
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    setLoading(true)
    setError(null)
    void api.getSmsPathPolicy()
      .then((response) => setPolicy(response.data ?? null))
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoading(false))
  }, [open])

  const move = (index: number, delta: -1 | 1) => {
    setPolicy((current) => {
      if (!current) return current
      const nextIndex = index + delta
      if (nextIndex < 0 || nextIndex >= current.priority.length) return current
      const priority = [...current.priority]
      ;[priority[index], priority[nextIndex]] = [priority[nextIndex], priority[index]]
      return { ...current, priority }
    })
  }

  const save = async () => {
    if (!policy) return
    setSaving(true)
    setError(null)
    try {
      const response = await api.setSmsPathPolicy(policy)
      if (response.data) setPolicy(response.data)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="sm">
      <DialogTitle>短信路径策略</DialogTitle>
      <DialogContent dividers>
        <Alert severity="info" sx={{ mb: 2 }}>
          发送时按从上到下的顺序尝试。保存设置不会发送测试短信。
        </Alert>
        {error && <Alert severity="error" sx={{ mb: 2 }}>{error}</Alert>}
        {loading || !policy ? (
          <Box display="flex" justifyContent="center" py={4}><CircularProgress /></Box>
        ) : (
          <>
            <Typography variant="subtitle2">优先级与线路开关</Typography>
            <List disablePadding sx={{ mb: 2 }}>
              {policy.priority.map((layer, index) => (
                <ListItem key={layer.kind} divider secondaryAction={(
                  <Box display="flex" alignItems="center">
                    <Tooltip title="上移"><span><IconButton size="small" disabled={index === 0} onClick={() => move(index, -1)}><ArrowUpward fontSize="small" /></IconButton></span></Tooltip>
                    <Tooltip title="下移"><span><IconButton size="small" disabled={index === policy.priority.length - 1} onClick={() => move(index, 1)}><ArrowDownward fontSize="small" /></IconButton></span></Tooltip>
                    <Switch checked={layer.enabled} onChange={(_, enabled) => setPolicy({
                      ...policy,
                      priority: policy.priority.map((item, itemIndex) => itemIndex === index ? { ...item, enabled } : item),
                    })} />
                  </Box>
                )}>
                  <ListItemText primary={`${index + 1}. ${pathLabels[layer.kind]}`} secondary={layer.enabled ? '启用' : '关闭'} />
                </ListItem>
              ))}
            </List>
            <FormControl fullWidth sx={{ mb: 2 }}>
              <InputLabel>发送中线路被关闭时</InputLabel>
              <Select label="发送中线路被关闭时" value={policy.mid_flight_disable} onChange={(event) => setPolicy({
                ...policy,
                mid_flight_disable: event.target.value,
              })}>
                <MenuItem value="auto_switch">自动切换到下一条线路</MenuItem>
                <MenuItem value="fail">直接反馈失败</MenuItem>
              </Select>
            </FormControl>
            <FormControlLabel control={<Switch checked={policy.dedupe_enabled} onChange={(_, value) => setPolicy({ ...policy, dedupe_enabled: value })} />} label="跨线路接收指纹去重" />
            <FormControlLabel control={<Switch checked={policy.cs_fallback_receiver} onChange={(_, value) => setPolicy({ ...policy, cs_fallback_receiver: value })} />} label="IMS 接收时仍保留 CS 监听（依赖去重）" />
            <TextField
              fullWidth type="number" label="去重指纹保留天数" value={policy.dedup_retention_days}
              onChange={(event) => setPolicy({ ...policy, dedup_retention_days: Math.max(1, Math.min(3650, Number(event.target.value) || 1)) })}
              inputProps={{ min: 1, max: 3650 }} sx={{ mt: 2 }}
            />
            <TextField
              fullWidth type="number" label="短信数据库最多保留条数" value={policy.message_retention_limit}
              helperText="超过上限后自动删除最旧记录；基带短信在成功入库后独立删除。"
              onChange={(event) => setPolicy({ ...policy, message_retention_limit: Math.max(100, Math.min(100000, Number(event.target.value) || 100)) })}
              inputProps={{ min: 100, max: 100000, step: 100 }} sx={{ mt: 2 }}
            />
          </>
        )}
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button variant="contained" onClick={() => void save()} disabled={!policy || loading || saving}>{saving ? '保存中…' : '保存'}</Button>
      </DialogActions>
    </Dialog>
  )
}
