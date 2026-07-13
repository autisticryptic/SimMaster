import {
  Box,
  Button,
  Checkbox,
  FormControlLabel,
  Paper,
  Typography,
} from '@mui/material'
import {
  SYSTEM_EVENT_GROUPS,
  defaultSystemEventCodes,
} from './systemEventModel'

type SystemEventRuleEditorProps = {
  eventCodes: string[]
  onChange: (eventCodes: string[]) => void
}

export default function SystemEventRuleEditor({ eventCodes, onChange }: SystemEventRuleEditorProps) {
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

  return (
    <Box mt={2}>
      <Box display="flex" alignItems="center" gap={1} mb={1}>
        <Typography variant="subtitle2">系统事件</Typography>
        <Button size="small" onClick={() => onChange(defaultSystemEventCodes())}>恢复默认</Button>
        <Button size="small" onClick={() => onChange(SYSTEM_EVENT_GROUPS.flatMap((group) => group.events.map((event) => event.code)))}>全选</Button>
        <Button size="small" onClick={() => onChange([])}>清空</Button>
      </Box>
      <Box
        display="grid"
        gridTemplateColumns={{ xs: '1fr', lg: 'repeat(2, minmax(0, 1fr))' }}
        gap={1.25}
        alignItems="stretch"
      >
        {SYSTEM_EVENT_GROUPS.map((group) => {
          const codes = group.events.map((event) => event.code)
          const checkedCount = codes.filter((code) => selected.has(code)).length
          const isResourceGroup = group.key === 'resource'
          return (
            <Paper
              key={group.key}
              variant="outlined"
              sx={{
                p: 1.5,
                borderRadius: 1,
                height: '100%',
                display: 'flex',
                flexDirection: 'column',
                gridColumn: isResourceGroup ? { lg: '1 / -1' } : undefined,
              }}
            >
              <Box display="flex" alignItems="center" justifyContent="space-between" mb={0.75}>
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
                  label={<Typography variant="caption">{checkedCount}/{codes.length}</Typography>}
                />
              </Box>
              <Box
                display="grid"
                gridTemplateColumns={isResourceGroup
                  ? { xs: '1fr', sm: 'repeat(2, minmax(0, 1fr))', lg: 'repeat(4, minmax(0, 1fr))' }
                  : { xs: '1fr', sm: 'repeat(2, minmax(0, 1fr))' }}
                columnGap={1.25}
                rowGap={0.25}
                sx={{ alignContent: 'start' }}
              >
                {group.events.map((event) => (
                  <FormControlLabel
                    key={event.code}
                    sx={{
                      m: 0,
                      minHeight: 30,
                      alignItems: 'flex-start',
                      '& .MuiFormControlLabel-label': { minWidth: 0 },
                    }}
                    control={
                      <Checkbox
                        size="small"
                        sx={{ pt: 0.5 }}
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
