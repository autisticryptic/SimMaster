import { type ChangeEvent, useRef } from 'react'
import {
  Accordion,
  AccordionDetails,
  AccordionSummary,
  Avatar,
  Box,
  Button,
  Card,
  CardContent,
  Checkbox,
  Chip,
  CircularProgress,
  FormControlLabel,
  IconButton,
  List,
  ListItemButton,
  ListItemText,
  MenuItem,
  Paper,
  Switch,
  TextField,
  Typography,
  useMediaQuery,
} from '@mui/material'
import { alpha, type Theme } from '@mui/material/styles'
import { Add, DeleteOutline, Dns, ExpandMore, NotificationsActive, QueryStats, Save, Sms, SystemUpdateAlt, AutoMode } from '@mui/icons-material'
import type {
  MatcherOperator,
  NotificationConfig,
  NotificationEventType,
  NotificationRule,
} from '../../api/current'
import {
  DEFAULT_TEMPLATES,
  EVENT_TYPES,
  MATCHER_OPERATORS,
  MATCH_FIELDS,
  TEMPLATE_VARIABLES,
  WEEKDAYS,
  createQuietSchedule,
  eventLabel,
} from './notificationModel'
import SystemEventRuleEditor from './SystemEventRuleEditor'
import DeviceStatusRuleEditor from './DeviceStatusRuleEditor'
import AutomationRuleEditor from './AutomationRuleEditor'

const EVENT_ICONS: Record<NotificationEventType, typeof Sms> = {
  sms: Sms,
  ddns: Dns,
  version_update: SystemUpdateAlt,
  system_event: NotificationsActive,
  device_status: QueryStats,
  automation: AutoMode,
}

type NotificationRulesTabProps = {
  config: NotificationConfig
  selectedEventType: NotificationEventType
  saving: boolean
  onSelectedEventTypeChange: (eventType: NotificationEventType) => void
  onAddRule: () => void
  onDeleteRule: (id: string) => void
  onPatchRule: (id: string, patch: Partial<NotificationRule>) => void
  onSave: () => void
}

export default function NotificationRulesTab({
  config,
  selectedEventType,
  saving,
  onSelectedEventTypeChange,
  onAddRule,
  onDeleteRule,
  onPatchRule,
  onSave,
}: NotificationRulesTabProps) {
  const textareaRefs = useRef<Record<string, HTMLTextAreaElement | HTMLInputElement | null>>({})

  const insertToken = (ruleId: string, template: string, token: string) => {
    const currentTemplate = template || ''
    const el = textareaRefs.current[ruleId]
    if (!el) {
      return `${currentTemplate}${currentTemplate ? '\n' : ''}${token}`
    }

    const start = el.selectionStart ?? currentTemplate.length
    const end = el.selectionEnd ?? currentTemplate.length

    const nextValue = currentTemplate.slice(0, start) + token + currentTemplate.slice(end)

    setTimeout(() => {
      el.focus()
      const newCursorPos = start + token.length
      el.setSelectionRange(newCursorPos, newCursorPos)
    }, 0)

    return nextValue
  }

  const isCompact = useMediaQuery<Theme>((theme: Theme) => theme.breakpoints.down('md'))
  const rulesForType = config.rules.filter((rule) => rule.type === selectedEventType)
  const ruleCountForType = (eventType: NotificationEventType) => {
    const rules = config.rules.filter((rule) => rule.type === eventType)
    return {
      enabled: rules.filter((rule) => rule.enabled).length,
      total: rules.length,
    }
  }

  const renderQuietHours = (rule: NotificationRule) => (
    <Box mt={2}>
      <Box display="flex" justifyContent="space-between" alignItems="center" mb={1}>
        <Typography variant="subtitle2">免打扰时间段</Typography>
        <Button size="small" startIcon={<Add />} onClick={() => onPatchRule(rule.id, { quiet_hours: [...rule.quiet_hours, createQuietSchedule()] })}>
          添加时间段
        </Button>
      </Box>
      {rule.quiet_hours.length === 0 && (
        <Typography variant="body2" color="text.secondary">未配置免打扰</Typography>
      )}
      {rule.quiet_hours.map((schedule, index) => (
        <Paper key={`${rule.id}-${index}`} variant="outlined" sx={{ p: 1.5, mb: 1.25, borderRadius: 1 }}>
          <Box display="flex" alignItems="center" gap={1.5} flexWrap="wrap">
            <Box display="flex" gap={0.5} flexWrap="wrap">
              {WEEKDAYS.map((day) => {
                const active = schedule.weekdays.includes(day.value)
                return (
                  <Button
                    key={day.value}
                    size="small"
                    variant={active ? 'contained' : 'outlined'}
                    sx={{ minWidth: 34, px: 0.5 }}
                    onClick={() => {
                      const weekdays = active
                        ? schedule.weekdays.filter((value) => value !== day.value)
                        : [...schedule.weekdays, day.value].sort((a, b) => a - b)
                      const next = [...rule.quiet_hours]
                      next[index] = { ...schedule, weekdays }
                      onPatchRule(rule.id, { quiet_hours: next })
                    }}
                  >
                    {day.label}
                  </Button>
                )
              })}
            </Box>
            <TextField
              size="small"
              type="time"
              label="开始"
              value={schedule.start}
              onChange={(event: ChangeEvent<HTMLInputElement>) => {
                const next = [...rule.quiet_hours]
                next[index] = { ...schedule, start: event.target.value }
                onPatchRule(rule.id, { quiet_hours: next })
              }}
            />
            <TextField
              size="small"
              type="time"
              label="结束"
              value={schedule.end}
              onChange={(event: ChangeEvent<HTMLInputElement>) => {
                const next = [...rule.quiet_hours]
                next[index] = { ...schedule, end: event.target.value }
                onPatchRule(rule.id, { quiet_hours: next })
              }}
            />
            <Box flexGrow={1} />
            <IconButton size="small" color="error" onClick={() => onPatchRule(rule.id, { quiet_hours: rule.quiet_hours.filter((_, itemIndex) => itemIndex !== index) })}>
              <DeleteOutline fontSize="small" />
            </IconButton>
            <Switch
              size="small"
              checked={schedule.enabled}
              onChange={(event: ChangeEvent<HTMLInputElement>) => {
                const next = [...rule.quiet_hours]
                next[index] = { ...schedule, enabled: event.target.checked }
                onPatchRule(rule.id, { quiet_hours: next })
              }}
            />
          </Box>
        </Paper>
      ))}
    </Box>
  )

  return (
    <Card sx={{ height: 'calc(100vh - 220px)', minHeight: 520 }}>
      <CardContent sx={{ height: '100%', p: 0, '&:last-child': { pb: 0 } }}>
        <Box display="flex" height="100%">
          {!isCompact && (
            <Box sx={{ width: 260, borderRight: 1, borderColor: 'divider', flexShrink: 0, display: 'flex', flexDirection: 'column' }}>
              <Box p={2}>
                <Typography variant="subtitle1" fontWeight={600}>消息类型</Typography>
              </Box>
              <Box sx={{ borderBottom: 1, borderColor: 'divider' }} />
              <List sx={{ flex: 1, overflow: 'auto' }}>
                {EVENT_TYPES.map((type) => {
                  const Icon = EVENT_ICONS[type.key]
                  const stats = ruleCountForType(type.key)
                  const hasEnabledRule = stats.enabled > 0
                  return (
                    <ListItemButton
                      key={type.key}
                      selected={selectedEventType === type.key}
                      onClick={() => onSelectedEventTypeChange(type.key)}
                      sx={{ gap: 1.25, py: 1.25 }}
                    >
                      <Avatar sx={{
                        width: 32,
                        height: 32,
                        bgcolor: (theme: Theme) => hasEnabledRule ? alpha(theme.palette.primary.main, 0.12) : theme.palette.action.hover,
                        color: hasEnabledRule ? 'primary.main' : 'text.secondary',
                      }}>
                        <Icon fontSize="small" />
                      </Avatar>
                      <ListItemText
                        sx={{ minWidth: 0 }}
                        primary={(
                          <Box display="flex" alignItems="center" gap={1} width="100%">
                            <Typography variant="body2" fontWeight={600} noWrap sx={{ minWidth: 0 }}>
                              {type.label}
                            </Typography>
                            <Typography variant="body2" color="text.secondary" noWrap sx={{ ml: 'auto', flexShrink: 0 }}>
                              ({stats.enabled}/{stats.total})
                            </Typography>
                          </Box>
                        )}
                      />
                    </ListItemButton>
                  )
                })}
              </List>
            </Box>
          )}
          <Box sx={{ flex: 1, minWidth: 0, display: 'flex', flexDirection: 'column', height: '100%', overflow: 'hidden' }}>
            <Box sx={{ p: 2, pb: 0 }}>
              {isCompact && (
                <TextField
                  select
                  size="small"
                  label="消息类型"
                  value={selectedEventType}
                  onChange={(event: ChangeEvent<HTMLInputElement>) => onSelectedEventTypeChange(event.target.value as NotificationEventType)}
                  sx={{ width: '100%', mb: 2 }}
                >
                  {EVENT_TYPES.map((type) => {
                    const stats = ruleCountForType(type.key)
                    return (
                      <MenuItem key={type.key} value={type.key}>
                        {`${type.label} (${stats.enabled}/${stats.total})`}
                      </MenuItem>
                    )
                  })}
                </TextField>
              )}
              <Box display="flex" alignItems="center" mb={2} gap={1} flexWrap="wrap">
                <Typography variant="subtitle1" fontWeight={600} noWrap sx={{ minWidth: 0 }}>{eventLabel(selectedEventType)} 规则</Typography>
                <Chip size="small" label={`共 ${rulesForType.length} 条`} />
                <Box flexGrow={1} />
                <Button variant="contained" startIcon={<Add />} onClick={onAddRule} sx={{ whiteSpace: 'nowrap' }}>新建规则</Button>
                <Button variant="outlined" startIcon={saving ? <CircularProgress size={18} /> : <Save />} disabled={saving} onClick={onSave} sx={{ whiteSpace: 'nowrap' }}>保存配置</Button>
              </Box>
            </Box>

            <Box sx={{ flex: 1, minWidth: 0, px: 2, pb: 2, overflow: 'auto' }}>
              {rulesForType.map((rule) => (
                <Accordion
                  key={rule.id}
                  defaultExpanded={rulesForType.length === 1}
                  sx={{
                    mb: 1.5,
                    border: 1,
                    borderColor: 'divider',
                    borderRadius: 1,
                    boxShadow: 'none',
                    overflow: 'hidden',
                    '&:before': { display: 'none' },
                    '&.Mui-expanded': { borderRadius: 1 },
                  }}
                >
                  <AccordionSummary expandIcon={<ExpandMore />}>
                    <Box display="flex" alignItems="center" gap={1.5} width="100%">
                      <Typography fontWeight={700} noWrap sx={{ minWidth: 0 }}>{rule.name}</Typography>
                      <Chip
                        size="small"
                        color={rule.enabled ? 'primary' : 'default'}
                        variant={rule.enabled ? 'filled' : 'outlined'}
                        label={rule.enabled ? '已启用' : '已停用'}
                      />
                      <Typography variant="body2" color="text.secondary" noWrap sx={{ display: { xs: 'none', sm: 'block' } }}>已绑定 {rule.channel_ids.length} 个通道</Typography>
                      <Box flexGrow={1} />
                      <Switch
                        checked={rule.enabled}
                        onClick={(event) => event.stopPropagation()}
                        onFocus={(event) => event.stopPropagation()}
                        onChange={(event: ChangeEvent<HTMLInputElement>) => onPatchRule(rule.id, { enabled: event.target.checked })}
                        inputProps={{ 'aria-label': `${rule.name} 启用状态` }}
                      />
                    </Box>
                  </AccordionSummary>
                  <AccordionDetails>
                    <Box display="grid" gridTemplateColumns={{ xs: '1fr', md: rule.type === 'system_event' || rule.type === 'device_status' || rule.type === 'automation' ? '1fr' : '1fr 1fr' }} gap={2}>
                      <TextField label="规则名称" value={rule.name} onChange={(event: ChangeEvent<HTMLInputElement>) => onPatchRule(rule.id, { name: event.target.value })} />
                      {rule.type !== 'system_event' && rule.type !== 'device_status' && rule.type !== 'automation' && (
                        <>
                          <TextField
                            select
                            label="匹配字段"
                            value={rule.matcher.field}
                            onChange={(event: ChangeEvent<HTMLInputElement>) => onPatchRule(rule.id, { matcher: { ...rule.matcher, field: event.target.value } })}
                          >
                            {MATCH_FIELDS[rule.type].map((field) => <MenuItem key={field.value} value={field.value}>{field.label}</MenuItem>)}
                          </TextField>
                          <TextField
                            select
                            label="匹配方式"
                            value={rule.matcher.operator}
                            onChange={(event: ChangeEvent<HTMLInputElement>) => onPatchRule(rule.id, { matcher: { ...rule.matcher, operator: event.target.value as MatcherOperator } })}
                          >
                            {MATCHER_OPERATORS.map((operator) => <MenuItem key={operator.value} value={operator.value}>{operator.label}</MenuItem>)}
                          </TextField>
                          <TextField
                            label="匹配内容"
                            value={rule.matcher.value}
                            disabled={rule.matcher.operator === 'always'}
                            onChange={(event: ChangeEvent<HTMLInputElement>) => onPatchRule(rule.id, { matcher: { ...rule.matcher, value: event.target.value } })}
                          />
                        </>
                      )}
                    </Box>

                    {rule.type === 'ddns' && (
                      <Box mt={2.5}>
                        <Typography variant="subtitle2" mb={1.5}>发送策略</Typography>
                        <Box
                          sx={{
                            display: 'grid',
                            gridTemplateColumns: { xs: '1fr', lg: '260px minmax(0, 1fr)' },
                            gap: 1.5,
                            alignItems: 'center',
                          }}
                        >
                          <TextField
                            fullWidth
                            type="number"
                            label="连续失败推送阈值"
                            value={rule.ddns_failure_threshold ?? 1}
                            onChange={(event: ChangeEvent<HTMLInputElement>) => {
                              const parsed = Number(event.target.value)
                              onPatchRule(rule.id, {
                                ddns_failure_threshold: Number.isFinite(parsed) && parsed > 0 ? Math.trunc(parsed) : 1,
                              })
                            }}
                            slotProps={{
                              htmlInput: {
                                min: 1,
                                step: 1,
                              },
                            }}
                          />
                          <Typography variant="body2" color="text.secondary">
                            达到阈值后推送；持续失败时按该次数间隔再次推送；成功、未变化或更新成功后重新计数。
                          </Typography>
                        </Box>
                      </Box>
                    )}

                    {rule.type === 'system_event' && (
                      <SystemEventRuleEditor
                        eventCodes={rule.event_codes ?? []}
                        onChange={(eventCodes) => onPatchRule(rule.id, { event_codes: eventCodes })}
                      />
                    )}

                    {rule.type === 'device_status' && (
                      <DeviceStatusRuleEditor
                        items={rule.device_status_items ?? []}
                        schedule={rule.device_status_schedule}
                        smsPeriod={rule.device_status_sms_period}
                        onItemsChange={(items) => onPatchRule(rule.id, { device_status_items: items })}
                        onScheduleChange={(schedule) => onPatchRule(rule.id, { device_status_schedule: schedule })}
                        onSmsPeriodChange={(period) => onPatchRule(rule.id, { device_status_sms_period: period as NotificationRule['device_status_sms_period'] })}
                      />
                    )}

                    {rule.type === 'automation' && (
                      <AutomationRuleEditor
                        eventCodes={rule.event_codes ?? []}
                        onChange={(eventCodes) => onPatchRule(rule.id, { event_codes: eventCodes })}
                      />
                    )}

                    <Box mt={2}>
                      <Typography variant="subtitle2" mb={1}>发送通道</Typography>
                      <Box display="flex" gap={1} flexWrap="wrap">
                        {config.channels.map((channel) => {
                          const checked = rule.channel_ids.includes(channel.id)
                          return (
                            <FormControlLabel
                              key={channel.id}
                              sx={{ border: 1, borderColor: checked ? 'primary.main' : 'divider', borderRadius: 1, px: 1, py: 0.25, m: 0 }}
                              control={
                                <Checkbox
                                  checked={checked}
                                  onChange={(event: ChangeEvent<HTMLInputElement>) => {
                                    const channelIds = event.target.checked
                                      ? [...rule.channel_ids, channel.id]
                                      : rule.channel_ids.filter((id) => id !== channel.id)
                                    onPatchRule(rule.id, { channel_ids: channelIds })
                                  }}
                                />
                              }
                              label={
                                <Box display="flex" alignItems="center" gap={0.75}>
                                  <Typography variant="body2">{channel.name}</Typography>
                                  {!channel.enabled && <Chip size="small" label="停用" />}
                                </Box>
                              }
                            />
                          )
                        })}
                        {config.channels.length === 0 && <Typography color="text.secondary">请先创建转发通道</Typography>}
                      </Box>
                    </Box>

                    <Box mt={2}>
                      <Box display="flex" alignItems="center" gap={1} mb={1} flexWrap="wrap">
                        <Typography variant="subtitle2">文本模板</Typography>
                        {TEMPLATE_VARIABLES[rule.type].map((variable) => (
                          <Chip
                            key={variable.token}
                            size="small"
                            label={variable.label}
                            variant="outlined"
                            onClick={() => {
                              const nextTemplate = insertToken(rule.id, rule.template, variable.token)
                              onPatchRule(rule.id, { template: nextTemplate })
                            }}
                          />
                        ))}
                      </Box>
                      <TextField
                        fullWidth
                        multiline
                        minRows={5}
                        value={rule.template}
                        inputRef={(el) => {
                          textareaRefs.current[rule.id] = el
                        }}
                        onChange={(event: ChangeEvent<HTMLInputElement>) => onPatchRule(rule.id, { template: event.target.value })}
                      />
                      <Button size="small" sx={{ mt: 1 }} onClick={() => onPatchRule(rule.id, { template: DEFAULT_TEMPLATES[rule.type] })}>恢复默认模板</Button>
                    </Box>

                    {renderQuietHours(rule)}

                    <Box display="flex" justifyContent="flex-end" gap={1} mt={2} flexWrap="wrap">
                      <Button variant="outlined" color="error" startIcon={<DeleteOutline />} onClick={() => onDeleteRule(rule.id)}>删除规则</Button>
                      <Button variant="contained" startIcon={saving ? <CircularProgress size={18} /> : <Save />} disabled={saving} onClick={onSave}>保存规则</Button>
                    </Box>
                  </AccordionDetails>
                </Accordion>
              ))}

              {rulesForType.length === 0 && (
                <Paper variant="outlined" sx={{ p: 4, textAlign: 'center', color: 'text.secondary' }}>
                  <Typography>暂无规则</Typography>
                </Paper>
              )}
            </Box>
          </Box>
        </Box>
      </CardContent>
    </Card>
  )
}
