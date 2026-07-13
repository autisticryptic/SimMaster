import { useState, useEffect } from 'react'
import {
  Box,
  Button,
  IconButton,
  Popover,
  Typography,
} from '@mui/material'
import {
  CalendarMonth,
  KeyboardArrowLeft,
  KeyboardArrowRight,
  KeyboardDoubleArrowLeft,
  KeyboardDoubleArrowRight,
} from '@mui/icons-material'

const WEEKDAY_LABELS = ['一', '二', '三', '四', '五', '六', '日']

const DATE_RANGE_BUTTON_SX = {
  height: 40,
  minWidth: 260,
  justifyContent: 'space-between',
  px: 1.5,
  borderColor: 'divider',
  color: 'text.primary',
  fontWeight: 400,
  '&:hover': {
    borderColor: 'text.disabled',
    bgcolor: 'transparent',
  },
}

function padDatePart(value: number) {
  return String(value).padStart(2, '0')
}

function formatDateValue(date: Date) {
  return `${date.getFullYear()}-${padDatePart(date.getMonth() + 1)}-${padDatePart(date.getDate())}`
}

function parseDateValue(value: string) {
  const [year, month, day] = value.split('-').map(Number)
  if (!year || !month || !day) return null
  return new Date(year, month - 1, day)
}

function addMonths(date: Date, amount: number) {
  return new Date(date.getFullYear(), date.getMonth() + amount, 1)
}

function monthTitle(date: Date) {
  return `${date.getFullYear()}年 ${date.getMonth() + 1}月`
}

function compareDateValue(left: string, right: string) {
  return left.localeCompare(right)
}

function monthGrid(monthDate: Date) {
  const firstDay = new Date(monthDate.getFullYear(), monthDate.getMonth(), 1)
  const mondayOffset = (firstDay.getDay() + 6) % 7
  const startDate = new Date(firstDay)
  startDate.setDate(firstDay.getDate() - mondayOffset)

  return Array.from({ length: 42 }, (_, index) => {
    const date = new Date(startDate)
    date.setDate(startDate.getDate() + index)
    return date
  })
}

export type DateRangePickerProps = {
  startDate: string
  endDate: string
  onChange: (startDate: string, endDate: string) => void
  minWidth?: number | string
  fullWidth?: boolean
}

export default function DateRangePicker({ startDate, endDate, onChange, minWidth = 260, fullWidth = false }: DateRangePickerProps) {
  const [anchorEl, setAnchorEl] = useState<HTMLElement | null>(null)
  const [baseMonth, setBaseMonth] = useState(() => parseDateValue(startDate) ?? new Date())
  const [draftStart, setDraftStart] = useState(startDate)
  const [draftEnd, setDraftEnd] = useState(endDate)
  const [hoverDate, setHoverDate] = useState('')
  const open = Boolean(anchorEl)
  const displayText = startDate || endDate
    ? `${startDate || '不限'}  →  ${endDate || '不限'}`
    : '选择时间范围'

  useEffect(() => {
    setDraftStart(startDate)
  }, [startDate])

  useEffect(() => {
    setDraftEnd(endDate)
  }, [endDate])

  const handleOpen = (event: React.MouseEvent<HTMLElement>) => {
    setDraftStart(startDate)
    setDraftEnd(endDate)
    setHoverDate('')
    setBaseMonth(parseDateValue(startDate) ?? parseDateValue(endDate) ?? new Date())
    setAnchorEl(event.currentTarget)
  }

  const handleClose = () => {
    setHoverDate('')
    setAnchorEl(null)
  }

  const handleClear = () => {
    setDraftStart('')
    setDraftEnd('')
    setHoverDate('')
    onChange('', '')
    handleClose()
  }

  const handleDayClick = (value: string) => {
    if (!draftStart || draftEnd) {
      setDraftStart(value)
      setDraftEnd('')
      setHoverDate('')
      onChange(value, '')
      return
    }

    const nextStart = compareDateValue(value, draftStart) < 0 ? value : draftStart
    const nextEnd = compareDateValue(value, draftStart) < 0 ? draftStart : value
    setDraftStart(nextStart)
    setDraftEnd(nextEnd)
    setHoverDate('')
    onChange(nextStart, nextEnd)
    handleClose()
  }

  const renderMonth = (monthDate: Date) => (
    <Box sx={{ width: 270 }}>
      <Typography variant="subtitle2" textAlign="center" mb={1}>{monthTitle(monthDate)}</Typography>
      <Box display="grid" gridTemplateColumns="repeat(7, 1fr)" gap={0.25} mb={0.5}>
        {WEEKDAY_LABELS.map((label) => (
          <Typography key={label} variant="caption" textAlign="center" color="text.secondary">{label}</Typography>
        ))}
      </Box>
      <Box display="grid" gridTemplateColumns="repeat(7, 1fr)" gap={0}>
        {monthGrid(monthDate).map((date) => {
          const value = formatDateValue(date)
          const inCurrentMonth = date.getMonth() === monthDate.getMonth()
          const previewEnd = draftStart && !draftEnd && hoverDate ? hoverDate : ''
          const rangeStart = previewEnd && compareDateValue(previewEnd, draftStart) < 0 ? previewEnd : draftStart
          const rangeEnd = draftEnd || previewEnd
          const isStart = value === draftStart
          const isEnd = value === draftEnd
          const isPreviewEnd = Boolean(previewEnd && value === previewEnd && value !== draftStart)
          const inRange = Boolean(rangeStart && rangeEnd && compareDateValue(value, rangeStart) > 0 && compareDateValue(value, rangeEnd) < 0)
          const previewActive = inRange || isPreviewEnd
          return (
            <Button
              key={value}
              size="small"
              onClick={() => handleDayClick(value)}
              onMouseEnter={() => setHoverDate(value)}
              sx={{
                minWidth: 0,
                height: 30,
                px: 0,
                border: '1px solid',
                borderColor: isPreviewEnd ? 'primary.main' : 'transparent',
                borderRadius: isStart || isEnd || isPreviewEnd ? 1 : 0,
                color: isStart || isEnd ? 'common.white' : inCurrentMonth ? 'text.primary' : 'text.disabled',
                bgcolor: isStart || isEnd ? 'primary.main' : previewActive ? 'rgba(18, 150, 219, 0.10)' : 'transparent',
                '&:hover': {
                  bgcolor: isStart || isEnd ? 'primary.dark' : previewActive ? 'rgba(18, 150, 219, 0.14)' : 'action.hover',
                },
              }}
            >
              {date.getDate()}
            </Button>
          )
        })}
      </Box>
    </Box>
  )

  return (
    <>
      <Button
        variant="outlined"
        size="small"
        onClick={handleOpen}
        endIcon={<CalendarMonth fontSize="small" />}
        sx={[DATE_RANGE_BUTTON_SX, { minWidth, width: fullWidth ? '100%' : undefined }]}
      >
        <Typography variant="body2" noWrap>{displayText}</Typography>
      </Button>
      <Popover
        open={open}
        anchorEl={anchorEl}
        onClose={handleClose}
        anchorOrigin={{ vertical: 'bottom', horizontal: 'left' }}
        transformOrigin={{ vertical: 'top', horizontal: 'left' }}
        slotProps={{ paper: { sx: { mt: 1, p: 1.5, borderRadius: 1.5 } } }}
      >
        <Box>
          <Box display="flex" alignItems="center" justifyContent="space-between" mb={1}>
            <Box>
              <IconButton size="small" onClick={() => setBaseMonth((current) => addMonths(current, -12))} aria-label="上一年">
                <KeyboardDoubleArrowLeft fontSize="small" />
              </IconButton>
              <IconButton size="small" onClick={() => setBaseMonth((current) => addMonths(current, -1))} aria-label="上一月">
                <KeyboardArrowLeft fontSize="small" />
              </IconButton>
            </Box>
            <Button size="small" onClick={handleClear}>清除</Button>
            <Box>
              <IconButton size="small" onClick={() => setBaseMonth((current) => addMonths(current, 1))} aria-label="下一月">
                <KeyboardArrowRight fontSize="small" />
              </IconButton>
              <IconButton size="small" onClick={() => setBaseMonth((current) => addMonths(current, 12))} aria-label="下一年">
                <KeyboardDoubleArrowRight fontSize="small" />
              </IconButton>
            </Box>
          </Box>
          <Box display="flex" gap={2} onMouseLeave={() => setHoverDate('')}>
            {renderMonth(baseMonth)}
            {renderMonth(addMonths(baseMonth, 1))}
          </Box>
        </Box>
      </Popover>
    </>
  )
}
