import { useState, useEffect, useRef } from 'react'
import {
  Alert,
  Box,
  Button,
  Chip,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  FormHelperText,
  MenuItem,
  TextField,
  Typography,
} from '@mui/material'
import type { AutomationTask, AutomationAction, AutomationTrigger, AutomationTarget, VolteLineControlResponse } from '../../api/contracts'
import { api } from '../../api/current'

type AutomationTaskDialogProps = {
  open: boolean
  onClose: () => void
  editingTask: AutomationTask | null
  onSave: (task: AutomationTask) => Promise<void>
}


export default function AutomationTaskDialog({
  open,
  onClose,
  editingTask,
  onSave,
}: AutomationTaskDialogProps) {
  const [formName, setFormName] = useState('')
  const [formEnabled, setFormEnabled] = useState(true)
  const [formActionType, setFormActionType] = useState<'restart_baseband' | 'reboot_device' | 'send_sms' | 'consume_data' | 'dial_call'>('restart_baseband')
  const [formRebootDelay, setFormRebootDelay] = useState(5)
  const [formSmsPhone, setFormSmsPhone] = useState('')
  const [formSmsContent, setFormSmsContent] = useState('')
  const [formSmsDelay, setFormSmsDelay] = useState(120)
  const [formSmsRetries, setFormSmsRetries] = useState(3)
  const [formDataBytes, setFormDataBytes] = useState(100)
  const [formDataUnit, setFormDataUnit] = useState<'auto' | 'bytes' | 'kb' | 'mb'>('bytes')
  const [formCountryCode, setFormCountryCode] = useState('+86')
  const [manualCountryCode, setManualCountryCode] = useState(false)
  const [formCallPhone, setFormCallPhone] = useState('')
  const [formCallDuration, setFormCallDuration] = useState(30)
  const [formTarget, setFormTarget] = useState<AutomationTarget | null>(null)
  const [lines, setLines] = useState<VolteLineControlResponse[]>([])

  const [formTriggerType, setFormTriggerType] = useState<'fixed' | 'interval' | 'cron'>('fixed')
  const [formWeekdays, setFormWeekdays] = useState<number[]>([1, 2, 3, 4, 5, 6, 7])
  const [formTriggerTime, setFormTriggerTime] = useState('04:00')
  const [formIntervalVal, setFormIntervalVal] = useState(180)
  const [formIntervalUnit, setFormIntervalUnit] = useState('days')
  const [formCronExpression, setFormCronExpression] = useState('*/15 * * * *')

  const [dialogError, setDialogError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)

  const smsContentRef = useRef<HTMLTextAreaElement | null>(null)

  useEffect(() => {
    if (open) {
      setLines([])
      void api.getVolteLines().then((response) => {
        if (response.data) setLines(response.data)
      }).catch(() => undefined)
      setDialogError(null)
      if (editingTask) {
        setFormName(editingTask.name)
        setFormEnabled(editingTask.enabled)
        setFormActionType(editingTask.action.type)
        setFormTarget(editingTask.target?.kind === 'modem_line' ? editingTask.target : null)
        if (editingTask.action.type === 'reboot_device') {
          setFormRebootDelay(editingTask.action.config.delay_seconds)
        } else if (editingTask.action.type === 'send_sms') {
          setFormSmsPhone(editingTask.action.config.phone_number)
          setFormSmsContent(editingTask.action.config.content)
          setFormSmsDelay(editingTask.action.config.random_delay_seconds ?? 0)
          setFormSmsRetries(editingTask.action.config.retry_limit ?? 0)
        } else if (editingTask.action.type === 'consume_data') {
          setFormDataBytes(editingTask.action.config.bytes)
          setFormDataUnit(editingTask.action.config.unit)
        } else if (editingTask.action.type === 'dial_call') {
          setManualCountryCode(!['+86', '+1', '+44', '+81', '+82'].includes(editingTask.action.config.country_code))
          setFormCountryCode(editingTask.action.config.country_code)
          setFormCallPhone(editingTask.action.config.phone_number)
          setFormCallDuration(editingTask.action.config.duration_seconds)
        }
        setFormTriggerType(editingTask.trigger.type)
        if (editingTask.trigger.type === 'fixed') {
          setFormWeekdays(editingTask.trigger.config.weekdays || [1, 2, 3, 4, 5, 6, 7])
          setFormTriggerTime((editingTask.trigger.config.times || []).join(', '))
        } else if (editingTask.trigger.type === 'interval') {
          setFormIntervalVal(editingTask.trigger.config.interval_value)
          setFormIntervalUnit(editingTask.trigger.config.interval_unit)
        } else {
          setFormCronExpression(editingTask.trigger.config.expression)
        }
      } else {
        setFormName('')
        setFormEnabled(true)
        setFormActionType('restart_baseband')
        setFormRebootDelay(5)
        setFormSmsPhone('')
        setFormSmsContent('')
        setFormSmsDelay(120)
        setFormSmsRetries(3)
        setFormDataBytes(100)
        setFormDataUnit('bytes')
        setFormCountryCode('+86')
        setManualCountryCode(false)
        setFormCallPhone('')
        setFormCallDuration(30)
        setFormTarget(null)
        setFormTriggerType('fixed')
        setFormWeekdays([1, 2, 3, 4, 5, 6, 7])
        setFormTriggerTime('04:00')
        setFormIntervalVal(180)
        setFormIntervalUnit('days')
        setFormCronExpression('*/15 * * * *')
      }
    }
  }, [open, editingTask])

  const insertVariable = (token: string) => {
    const el = smsContentRef.current
    if (!el) {
      setFormSmsContent((prev) => prev + token)
      return
    }
    const start = el.selectionStart ?? formSmsContent.length
    const end = el.selectionEnd ?? formSmsContent.length
    const nextValue = formSmsContent.slice(0, start) + token + formSmsContent.slice(end)
    setFormSmsContent(nextValue)
    setTimeout(() => {
      el.focus()
      const newCursorPos = start + token.length
      el.setSelectionRange(newCursorPos, newCursorPos)
    }, 0)
  }

  const handleToggleWeekday = (day: number) => {
    setFormWeekdays((prev) =>
      prev.includes(day) ? prev.filter((d) => d !== day) : [...prev, day].sort()
    )
  }

  const handleSave = async () => {
    setDialogError(null)

    if (!formName.trim()) {
      setDialogError('请输入任务名称')
      return
    }

    if (formActionType !== 'reboot_device' && formTarget?.kind !== 'modem_line') {
      setDialogError('请选择执行该任务的基带线路')
      return
    }

    let action: AutomationAction
    if (formActionType === 'restart_baseband') {
      action = { type: 'restart_baseband', config: null }
    } else if (formActionType === 'reboot_device') {
      action = { type: 'reboot_device', config: { delay_seconds: Number(formRebootDelay) || 5 } }
    } else if (formActionType === 'send_sms') {
      const phoneClean = formSmsPhone.trim()
      if (!phoneClean) {
        setDialogError('请输入接收短信的手机号码')
        return
      }
      if (!/^[0-9+]+$/.test(phoneClean)) {
        setDialogError('接收号码格式不正确（只能包含数字和“+”号）')
        return
      }
      action = {
        type: 'send_sms',
        config: {
          phone_number: phoneClean,
          content: formSmsContent,
          random_delay_seconds: Number(formSmsDelay) || 0,
          retry_limit: Number(formSmsRetries) || 0,
        },
      }
    } else if (formActionType === 'consume_data') {
      if (!Number.isInteger(Number(formDataBytes)) || Number(formDataBytes) <= 0) {
        setDialogError('请输入大于 0 的流量大小')
        return
      }
      action = { type: 'consume_data', config: { bytes: Number(formDataBytes), unit: formDataUnit } }
    } else {
      const country = formCountryCode.trim()
      const phone = formCallPhone.trim()
      if (!/^\+[0-9]+$/.test(country) || !/^[0-9]+$/.test(phone)) {
        setDialogError('请输入合法的国家区号和号码主体')
        return
      }
      action = { type: 'dial_call', config: { country_code: country, phone_number: phone, duration_seconds: Math.min(7200, Math.max(1, Number(formCallDuration) || 1)) } }
    }

    let trigger: AutomationTrigger
    if (formTriggerType === 'fixed') {
      const rawTimes = formTriggerTime
        .replace(/：/g, ':')
        .replace(/，/g, ',')
        .split(',')
        .map((t) => t.trim())
        .filter((t) => t.length > 0)

      const times: string[] = []
      for (const t of rawTimes) {
        const match = t.match(/^(\d{1,2}):(\d{1,2})$/)
        if (match) {
          const hour = parseInt(match[1], 10)
          const minute = parseInt(match[2], 10)
          if (hour >= 0 && hour <= 23 && minute >= 0 && minute <= 59) {
            const paddedHour = hour.toString().padStart(2, '0')
            const paddedMinute = minute.toString().padStart(2, '0')
            times.push(`${paddedHour}:${paddedMinute}`)
            continue
          }
        }
        setDialogError(`请输入合法的触发时间: "${t}"（格式如 04:00，多个用逗号隔开）`)
        return
      }

      if (times.length === 0) {
        setDialogError('请输入合法的触发时间（格式如 04:00，多个用逗号隔开）')
        return
      }

      if (formWeekdays.length === 0) {
        setDialogError('请至少选择一个触发星期')
        return
      }

      trigger = {
        type: 'fixed',
        config: {
          weekdays: formWeekdays,
          times,
        },
      }
    } else if (formTriggerType === 'interval') {
      trigger = {
        type: 'interval',
        config: {
          interval_value: Number(formIntervalVal) || 1,
          interval_unit: formIntervalUnit as 'mins' | 'hours' | 'days',
        },
      }
    } else {
      const expression = formCronExpression.trim()
      if (expression.split(/\s+/).length !== 5) {
        setDialogError('Cron 必须是 5 段表达式，例如 */15 * * * *')
        return
      }
      trigger = { type: 'cron', config: { expression } }
    }

    const newTask: AutomationTask = {
      id: editingTask?.id || `task-${Date.now()}-${Math.random().toString(36).slice(2, 6)}`,
      name: formName.trim(),
      enabled: formEnabled,
      trigger,
      target: formActionType === 'reboot_device' ? null : formTarget,
      action,
    }

    setSaving(true)
    try {
      await onSave(newTask)
      onClose()
    } catch (err) {
      setDialogError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      maxWidth="sm"
      fullWidth
      slotProps={{
        paper: {
          sx: { borderRadius: 2.5 },
        },
      }}
    >
      <DialogTitle sx={{ fontWeight: 700, pb: 1 }}>
        {editingTask ? '编辑自动化任务' : '添加自动化任务'}
      </DialogTitle>
      <DialogContent>
        <Box display="flex" flexDirection="column" gap={2.5} mt={1}>
          {dialogError && (
            <Alert severity="error" onClose={() => setDialogError(null)}>
              {dialogError}
            </Alert>
          )}
          <TextField
            label="任务名称"
            placeholder="例如：每日凌晨基带自动重启"
            fullWidth
            value={formName}
            onChange={(e) => {
              setFormName(e.target.value)
              setDialogError(null)
            }}
          />

          <TextField
            select
            label="执行动作"
            fullWidth
            value={formActionType}
            onChange={(e) => {
              const actionType = e.target.value as typeof formActionType
              setFormActionType(actionType)
              if (actionType === 'reboot_device') setFormTarget(null)
            }}
          >
            <MenuItem value="restart_baseband">重启基带</MenuItem>
            <MenuItem value="reboot_device">重启设备</MenuItem>
            <MenuItem value="send_sms">发送短信</MenuItem>
            <MenuItem value="consume_data">消耗移动流量</MenuItem>
            <MenuItem value="dial_call">定时拨号</MenuItem>
          </TextField>

          {formActionType !== 'reboot_device' && (
            <TextField
              select
              required
              label="使用的基带 / SIM 卡"
              value={formTarget ? `${formTarget.kind}:${formTarget.kind === 'modem_line' ? formTarget.line_id : formTarget.slot_id}` : ''}
              onChange={(e) => {
                const [kind, ...rest] = e.target.value.split(':')
                setFormTarget(kind === 'modem_line' ? { kind: 'modem_line', line_id: rest.join(':') } : null)
                setDialogError(null)
              }}
              helperText="任务始终绑定到所选线路，不会自动回退到其他基带"
            >
              <MenuItem value="" disabled>请选择基带线路</MenuItem>
              {lines.map((line) => <MenuItem key={line.modem.line_id} value={`modem_line:${line.modem.line_id}`}>基带 {line.modem.display_order || line.modem.modem_id} · 卡槽 {line.modem.uim_slot}</MenuItem>)}
            </TextField>
          )}

          {/* 重启设备特有字段 */}
          {formActionType === 'reboot_device' && (
            <TextField
              label="重启延迟时间 (秒)"
              type="number"
              fullWidth
              value={formRebootDelay}
              onChange={(e) => setFormRebootDelay(Math.max(2, parseInt(e.target.value, 10) || 2))}
              slotProps={{ htmlInput: { min: 2, max: 60 } }}
            />
          )}

          {/* 发送短信特有字段 */}
          {formActionType === 'send_sms' && (
            <Box display="flex" flexDirection="column" gap={2.5}>
              <TextField
                label="接收号码"
                placeholder="如：10010 或其他号码"
                fullWidth
                value={formSmsPhone}
                onChange={(e) => setFormSmsPhone(e.target.value)}
              />

              <Box>
                <Box display="flex" justifyContent="space-between" alignItems="center" mb={1}>
                  <Typography variant="body2" fontWeight={600} color="text.secondary">
                    短信内容
                  </Typography>
                  <Box display="flex" gap={0.5}>
                    <Chip
                      size="small"
                      label="+ 时间"
                      variant="outlined"
                      onClick={() => insertVariable('{{时间}}')}
                      sx={{ cursor: 'pointer' }}
                    />
                    <Chip
                      size="small"
                      label="+ 随机字符串"
                      variant="outlined"
                      onClick={() => insertVariable('{{随机字符串}}')}
                      sx={{ cursor: 'pointer' }}
                    />
                  </Box>
                </Box>
                <TextField
                  multiline
                  rows={3}
                  placeholder="发送内容，如：开源项目 SimAdmin {{时间}}"
                  fullWidth
                  value={formSmsContent}
                  onChange={(e) => setFormSmsContent(e.target.value)}
                  inputRef={smsContentRef}
                />
                <FormHelperText>
                  可在内容中插入变量，短信发送时会自动替换。
                </FormHelperText>
              </Box>

              <Box display="grid" gridTemplateColumns="1fr 1fr" gap={2}>
                <TextField
                  label="随机延迟范围 (秒)"
                  type="number"
                  value={formSmsDelay}
                  onChange={(e) => setFormSmsDelay(Math.max(0, parseInt(e.target.value, 10) || 0))}
                  helperText="从0到设定值随机延迟后发送"
                />
                <TextField
                  label="失败重试次数"
                  type="number"
                  value={formSmsRetries}
                  onChange={(e) => setFormSmsRetries(Math.max(0, parseInt(e.target.value, 10) || 0))}
                  helperText="发送失败后每5秒自动重试"
                />
              </Box>
            </Box>
          )}

          {formActionType === 'consume_data' && (
            <Box display="grid" gridTemplateColumns="1fr 1fr" gap={2}>
              <TextField label="流量大小" type="number" value={formDataBytes} onChange={(e) => setFormDataBytes(Math.max(1, Number(e.target.value) || 1))} slotProps={{ htmlInput: { min: 1 } }} />
              <TextField select label="单位" value={formDataUnit} onChange={(e) => setFormDataUnit(e.target.value as typeof formDataUnit)}>
                <MenuItem value="auto">自动</MenuItem><MenuItem value="bytes">Byte</MenuItem><MenuItem value="kb">KiB</MenuItem><MenuItem value="mb">MiB</MenuItem>
              </TextField>
            </Box>
          )}

          {formActionType === 'dial_call' && (
            <Box display="grid" gridTemplateColumns="0.8fr 1.2fr" gap={2}>
              {manualCountryCode ? <TextField label="国家区号" value={formCountryCode} onChange={(e) => setFormCountryCode(e.target.value)} placeholder="例如 +61、+31" /> : <TextField select label="国家区号" value={formCountryCode} onChange={(e) => e.target.value === 'manual' ? (setManualCountryCode(true), setFormCountryCode('+61')) : setFormCountryCode(e.target.value)}>
                <MenuItem value="+86">中国 +86</MenuItem><MenuItem value="+1">美国/加拿大 +1</MenuItem><MenuItem value="+44">英国 +44</MenuItem><MenuItem value="+81">日本 +81</MenuItem><MenuItem value="+82">韩国 +82</MenuItem><MenuItem value="manual">手动输入</MenuItem>
              </TextField>}
              <TextField label="号码主体" value={formCallPhone} onChange={(e) => setFormCallPhone(e.target.value)} />
              <TextField label="拨号保持时间（秒）" type="number" value={formCallDuration} onChange={(e) => setFormCallDuration(Math.min(7200, Math.max(1, Number(e.target.value) || 1)))} slotProps={{ htmlInput: { min: 1, max: 7200 } }} sx={{ gridColumn: '1 / -1' }} />
            </Box>
          )}

          <TextField
            select
            label="触发机制"
            fullWidth
            value={formTriggerType}
            onChange={(e) => setFormTriggerType(e.target.value as 'fixed' | 'interval' | 'cron')}
          >
            <MenuItem value="fixed">定点定时</MenuItem>
            <MenuItem value="interval">时间间隔</MenuItem>
            <MenuItem value="cron">Cron 表达式</MenuItem>
          </TextField>

          {/* 定点定时配置 */}
          {formTriggerType === 'fixed' && (
            <Box display="flex" flexDirection="column" gap={2.5}>
              <Box>
                <Typography variant="body2" fontWeight={600} color="text.secondary" mb={1}>
                  重复星期
                </Typography>
                <Box display="flex" gap={0.5} flexWrap="wrap">
                  {[
                    { val: 1, label: '一' },
                    { val: 2, label: '二' },
                    { val: 3, label: '三' },
                    { val: 4, label: '四' },
                    { val: 5, label: '五' },
                    { val: 6, label: '六' },
                    { val: 7, label: '日' },
                  ].map((day) => {
                    const active = formWeekdays.includes(day.val)
                    return (
                      <Button
                        key={day.val}
                        size="small"
                        variant={active ? 'contained' : 'outlined'}
                        sx={{ minWidth: 36, px: 0 }}
                        onClick={() => handleToggleWeekday(day.val)}
                      >
                        {day.label}
                      </Button>
                    )
                  })}
                </Box>
              </Box>

              <TextField
                label="触发时刻 (HH:MM，多个用逗号隔开)"
                placeholder="例如：04:00, 12:00"
                fullWidth
                value={formTriggerTime}
                onChange={(e) => setFormTriggerTime(e.target.value)}
                helperText="输入英文或中文逗号隔开的HH:MM时刻，例如 04:00, 16:30"
              />
            </Box>
          )}

          {/* 时间间隔配置 */}
          {formTriggerType === 'interval' && (
            <Box display="grid" gridTemplateColumns="1fr 1fr" gap={2}>
              <TextField
                label="间隔时长"
                type="number"
                value={formIntervalVal}
                onChange={(e) => setFormIntervalVal(Math.max(1, parseInt(e.target.value, 10) || 1))}
              />
              <TextField
                select
                label="时间单位"
                value={formIntervalUnit}
                onChange={(e) => setFormIntervalUnit(e.target.value)}
              >
                <MenuItem value="mins">分钟</MenuItem>
                <MenuItem value="hours">小时</MenuItem>
                <MenuItem value="days">天</MenuItem>
              </TextField>
            </Box>
          )}
          {formTriggerType === 'cron' && (
            <TextField label="Cron 表达式（5 段）" value={formCronExpression} onChange={(e) => setFormCronExpression(e.target.value)} placeholder="*/15 * * * *" helperText="北京时间：分钟 小时 日 月 星期（星期日为 0）" />
          )}
        </Box>
      </DialogContent>
      <DialogActions sx={{ px: 3, pb: 2.5 }}>
        <Button variant="outlined" onClick={onClose} disabled={saving}>
          取消
        </Button>
        <Button variant="contained" onClick={() => void handleSave()} disabled={saving}>
          保存
        </Button>
      </DialogActions>
    </Dialog>
  )
}
