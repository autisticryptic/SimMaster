import {
  Box,
  Button,
  Checkbox,
  FormControlLabel,
  Paper,
  Typography,
} from '@mui/material'
import { defaultAutomationEventCodes } from './notificationModel'

const AUTOMATION_EVENT_GROUPS = [
  {
    key: 'restart_baseband',
    label: '重启基带',
    events: [
      { code: 'restart_baseband:success', label: '执行成功' },
      { code: 'restart_baseband:failed', label: '执行失败' },
    ],
  },
  {
    key: 'reboot_device',
    label: '重启设备',
    events: [
      { code: 'reboot_device:success', label: '执行成功' },
      { code: 'reboot_device:failed', label: '执行失败' },
    ],
  },
  {
    key: 'send_sms',
    label: '发送短信',
    events: [
      { code: 'send_sms:success', label: '发送成功' },
      { code: 'send_sms:failed', label: '发送失败' },
    ],
  },
]

type AutomationRuleEditorProps = {
  eventCodes: string[]
  onChange: (eventCodes: string[]) => void
}

export default function AutomationRuleEditor({ eventCodes, onChange }: AutomationRuleEditorProps) {
  const selected = new Set(eventCodes)

  const toggleEvent = (code: string, checked: boolean) => {
    const next = new Set(selected)
    if (checked) {
      next.add(code)
    } else {
      next.delete(code)
    }
    onChange([...next])
  }

  const toggleGroup = (codes: string[], checked: boolean) => {
    const next = new Set(selected)
    codes.forEach((code) => {
      if (checked) {
        next.add(code)
      } else {
        next.delete(code)
      }
    })
    onChange([...next])
  }

  const allCodes = AUTOMATION_EVENT_GROUPS.flatMap((group) => group.events.map((e) => e.code))

  return (
    <Box mt={2}>
      <Box display="flex" alignItems="center" gap={1} mb={1.5}>
        <Typography variant="subtitle2">自动化事件类型</Typography>
        <Button size="small" onClick={() => onChange(defaultAutomationEventCodes())}>恢复默认</Button>
        <Button size="small" onClick={() => onChange(allCodes)}>全选</Button>
        <Button size="small" onClick={() => onChange([])}>清空</Button>
      </Box>
      <Box
        display="grid"
        gridTemplateColumns={{ xs: '1fr', md: 'repeat(3, minmax(0, 1fr))' }}
        gap={2}
        alignItems="stretch"
      >
        {AUTOMATION_EVENT_GROUPS.map((group) => {
          const codes = group.events.map((event) => event.code)
          const checkedCount = codes.filter((code) => selected.has(code)).length
          return (
            <Paper
              key={group.key}
              variant="outlined"
              sx={{
                p: 1.5,
                borderRadius: 1.5,
                height: '100%',
                display: 'flex',
                flexDirection: 'column',
                bgcolor: (theme) => theme.palette.mode === 'light' ? 'rgba(255,255,255,0.48)' : 'rgba(30,41,59,0.2)',
                borderColor: 'divider',
              }}
            >
              <Box display="flex" alignItems="center" justifyContent="space-between" mb={1} pb={0.5} sx={{ borderBottom: '1px solid', borderColor: 'divider' }}>
                <Typography variant="body2" fontWeight={700}>{group.label}</Typography>
                <FormControlLabel
                  sx={{ m: 0 }}
                  control={
                    <Checkbox
                      size="small"
                      checked={checkedCount === codes.length}
                      indeterminate={checkedCount > 0 && checkedCount < codes.length}
                      onChange={(event) => toggleGroup(codes, event.target.checked)}
                    />
                  }
                  label={<Typography variant="caption" sx={{ fontWeight: 600 }}>{checkedCount}/{codes.length}</Typography>}
                />
              </Box>
              <Box
                display="flex"
                flexDirection="column"
                gap={0.5}
              >
                {group.events.map((event) => (
                  <FormControlLabel
                    key={event.code}
                    sx={{
                      m: 0,
                      minHeight: 32,
                      alignItems: 'center',
                      '& .MuiFormControlLabel-label': { minWidth: 0 },
                    }}
                    control={
                      <Checkbox
                        size="small"
                        checked={selected.has(event.code)}
                        onChange={(changeEvent) => toggleEvent(event.code, changeEvent.target.checked)}
                      />
                    }
                    label={<Typography variant="body2">{event.label}</Typography>}
                  />
                ))}
              </Box>
            </Paper>
          )
        })}
      </Box>
    </Box>
  )
}
