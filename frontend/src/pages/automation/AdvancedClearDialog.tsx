import { useState, useEffect } from 'react'
import {
  Alert,
  Box,
  Button,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  IconButton,
  MenuItem,
  TextField,
  Typography,
} from '@mui/material'
import { DeleteSweep, Delete, Close } from '@mui/icons-material'
import DateRangePicker from '../../components/DateRangePicker'

type AdvancedClearDialogProps = {
  open: boolean
  onClose: () => void
  defaultType: string
  defaultStatus: string
  defaultStartDate: string
  defaultEndDate: string
  onConfirm: (filters: {
    type: string
    status: string
    start_date: string
    end_date: string
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

export default function AdvancedClearDialog({
  open,
  onClose,
  defaultType,
  defaultStatus,
  defaultStartDate,
  defaultEndDate,
  onConfirm,
}: AdvancedClearDialogProps) {
  const [clearType, setClearType] = useState('')
  const [clearStatus, setClearStatus] = useState('')
  const [clearStartDate, setClearStartDate] = useState('')
  const [clearEndDate, setClearEndDate] = useState('')
  const [clearing, setClearing] = useState(false)

  useEffect(() => {
    if (open) {
      setClearType(defaultType)
      setClearStatus(defaultStatus)
      setClearStartDate(defaultStartDate)
      setClearEndDate(defaultEndDate)
    }
  }, [open, defaultType, defaultStatus, defaultStartDate, defaultEndDate])

  const handleConfirm = async () => {
    setClearing(true)
    try {
      await onConfirm({
        type: clearType,
        status: clearStatus,
        start_date: clearStartDate,
        end_date: clearEndDate,
      })
      onClose()
    } finally {
      setClearing(false)
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
        <DeleteSweep color="primary" fontSize="small" />
        <Typography variant="subtitle1" fontWeight={700}>
          高级清理日志
        </Typography>
        <Box flexGrow={1} />
        <IconButton size="small" onClick={onClose} aria-label="关闭">
          <Close fontSize="small" />
        </IconButton>
      </DialogTitle>
      <DialogContent dividers sx={{ pt: 3 }}>
        <Box display="flex" flexDirection="column" gap={2}>
          <Alert severity="warning">
            清理操作不可逆，请谨慎选择过滤条件。默认条件为当前表格的筛选状态。
          </Alert>

          <TextField
            select
            size="small"
            label="任务类型"
            value={clearType}
            onChange={(e) => setClearType(e.target.value)}
            fullWidth
            sx={filterTextFieldSx}
          >
            <MenuItem value="">所有类型</MenuItem>
            <MenuItem value="restart_baseband">基带维护</MenuItem>
            <MenuItem value="reboot_device">系统操作</MenuItem>
            <MenuItem value="send_sms">短信发送</MenuItem>
          </TextField>

          <TextField
            select
            size="small"
            label="执行状态"
            value={clearStatus}
            onChange={(e) => setClearStatus(e.target.value)}
            fullWidth
            sx={filterTextFieldSx}
          >
            <MenuItem value="">所有状态</MenuItem>
            <MenuItem value="success">成功</MenuItem>
            <MenuItem value="failed">失败</MenuItem>
          </TextField>

          <Box>
            <Typography variant="body2" color="text.secondary" mb={1}>
              日期范围
            </Typography>
            <DateRangePicker
              startDate={clearStartDate}
              endDate={clearEndDate}
              onChange={(startDate, endDate) => {
                setClearStartDate(startDate)
                setClearEndDate(endDate)
              }}
              fullWidth
            />
            <Typography variant="caption" color="text.secondary" sx={{ display: 'block', mt: 0.5 }}>
              留空表示不限制开始或结束时间
            </Typography>
          </Box>
        </Box>
      </DialogContent>
      <DialogActions sx={{ px: 3, py: 2 }}>
        <Button variant="outlined" onClick={onClose} disabled={clearing}>
          取消
        </Button>
        <Button color="error" variant="contained" startIcon={<Delete />} onClick={() => void handleConfirm()} disabled={clearing}>
          确认清理
        </Button>
      </DialogActions>
    </Dialog>
  )
}
