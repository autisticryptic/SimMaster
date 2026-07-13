import { useState, useEffect } from 'react'
import {
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  IconButton,
  InputAdornment,
  Switch,
  TextField,
  Typography,
} from '@mui/material'
import { SmartToy, Close } from '@mui/icons-material'
import type { NotificationConfig } from '../../api/contracts'

type AutoCleanDialogProps = {
  open: boolean
  onClose: () => void
  notificationConfig: NotificationConfig | null
  onSave: (cleanup: {
    retention_days_enabled: boolean
    retention_days: number
    max_entries_enabled: boolean
    max_entries: number
  }) => Promise<void>
}

const filterTextFieldSx = {
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
}

export default function AutoCleanDialog({ open, onClose, notificationConfig, onSave }: AutoCleanDialogProps) {
  const [autoRetentionEnabled, setAutoRetentionEnabled] = useState(false)
  const [autoRetentionDays, setAutoRetentionDays] = useState('90')
  const [autoMaxEntriesEnabled, setAutoMaxEntriesEnabled] = useState(false)
  const [autoMaxEntries, setAutoMaxEntries] = useState('10000')
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (open) {
      if (notificationConfig) {
        setAutoRetentionEnabled(notificationConfig.log_cleanup.retention_days_enabled)
        setAutoRetentionDays(String(notificationConfig.log_cleanup.retention_days))
        setAutoMaxEntriesEnabled(notificationConfig.log_cleanup.max_entries_enabled)
        setAutoMaxEntries(String(notificationConfig.log_cleanup.max_entries))
      } else {
        setAutoRetentionEnabled(true)
        setAutoRetentionDays('90')
        setAutoMaxEntriesEnabled(true)
        setAutoMaxEntries('10000')
      }
    }
  }, [open, notificationConfig])

  const handleConfirm = async () => {
    setSaving(true)
    try {
      const days = parseInt(autoRetentionDays, 10) || 90
      const count = parseInt(autoMaxEntries, 10) || 10000
      await onSave({
        retention_days_enabled: autoRetentionEnabled,
        retention_days: days,
        max_entries_enabled: autoMaxEntriesEnabled,
        max_entries: count,
      })
      onClose()
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      fullWidth
      maxWidth="xs"
      slotProps={{
        paper: {
          sx: { borderRadius: 2.5 },
        },
      }}
    >
      <DialogTitle sx={{ display: 'flex', alignItems: 'center', gap: 1, pr: 1, fontWeight: 700 }}>
        <SmartToy color="primary" fontSize="small" />
        <Typography variant="subtitle1" fontWeight={700}>
          自动清理设置
        </Typography>
        <Box flexGrow={1} />
        <IconButton size="small" onClick={onClose} aria-label="关闭">
          <Close fontSize="small" />
        </IconButton>
      </DialogTitle>
      <DialogContent dividers sx={{ pt: 3 }}>
        <Box display="flex" flexDirection="column" gap={3}>
          {/* 按保留时长清理 */}
          <Box>
            <Box display="flex" alignItems="center" justifyContent="space-between" mb={0.5}>
              <Typography variant="subtitle2">按保留时长清理</Typography>
              <Switch
                checked={autoRetentionEnabled}
                onChange={(e) => setAutoRetentionEnabled(e.target.checked)}
              />
            </Box>
            <Typography variant="caption" color="text.secondary">
              超过设定天数的记录将被永久删除
            </Typography>
            <TextField
              size="small"
              type="number"
              value={autoRetentionDays}
              onChange={(e) => {
                const next = e.target.value
                if (/^\d{0,5}$/.test(next)) setAutoRetentionDays(next)
              }}
              fullWidth
              disabled={!autoRetentionEnabled}
              sx={{ mt: 1, ...filterTextFieldSx }}
              slotProps={{
                input: { endAdornment: <InputAdornment position="end">天</InputAdornment> },
                htmlInput: { min: 1 },
              }}
            />
          </Box>

          <Box sx={{ borderTop: 1, borderColor: 'divider' }} />

          {/* 按最大条数清理 */}
          <Box>
            <Box display="flex" alignItems="center" justifyContent="space-between" mb={0.5}>
              <Typography variant="subtitle2">按最大条数清理</Typography>
              <Switch
                checked={autoMaxEntriesEnabled}
                onChange={(e) => setAutoMaxEntriesEnabled(e.target.checked)}
              />
            </Box>
            <Typography variant="caption" color="text.secondary">
              总数超过此阈值时，自动删除最旧记录
            </Typography>
            <TextField
              size="small"
              type="number"
              value={autoMaxEntries}
              onChange={(e) => {
                const next = e.target.value
                if (/^\d{0,8}$/.test(next)) setAutoMaxEntries(next)
              }}
              fullWidth
              disabled={!autoMaxEntriesEnabled}
              sx={{ mt: 1, ...filterTextFieldSx }}
              slotProps={{
                input: { endAdornment: <InputAdornment position="end">条</InputAdornment> },
                htmlInput: { min: 1 },
              }}
            />
          </Box>
        </Box>
      </DialogContent>
      <DialogActions sx={{ px: 3, py: 2 }}>
        <Button variant="outlined" onClick={onClose} disabled={saving}>
          取消
        </Button>
        <Button variant="contained" onClick={() => void handleConfirm()} disabled={saving}>
          保存设置
        </Button>
      </DialogActions>
    </Dialog>
  )
}
