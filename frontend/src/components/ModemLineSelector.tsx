import { FormControl, InputLabel, MenuItem, Select, type SelectChangeEvent } from '@mui/material'
import type { ModemBinding } from '../api/current'
import { modemLineLabel } from './modemLineFormat'

interface ModemLineSelectorProps {
  lines: Array<{ modem: ModemBinding }>
  value: string
  onChange: (lineId: string) => void
  disabled?: boolean
  label?: string
  includeAutomatic?: boolean
  fullWidth?: boolean
  size?: 'small' | 'medium'
}

export default function ModemLineSelector({
  lines,
  value,
  onChange,
  disabled = false,
  label = '发送线路',
  includeAutomatic = true,
  fullWidth = true,
  size = 'small',
}: ModemLineSelectorProps) {
  const handleChange = (event: SelectChangeEvent<string>) => onChange(event.target.value)

  return (
    <FormControl size={size} fullWidth={fullWidth} disabled={disabled || lines.length === 0}>
      <InputLabel>{label}</InputLabel>
      <Select value={value} label={label} onChange={handleChange}>
        {includeAutomatic && <MenuItem value="">自动选择（兼容主线路）</MenuItem>}
        {lines.map((line, index) => (
          <MenuItem key={line.modem.line_id} value={line.modem.line_id} disabled={!line.modem.present}>
            {modemLineLabel(line, index)}
            {line.modem.line_kind === 'reader' ? ' · 读卡器' : ''}
            {!line.modem.present ? '（离线）' : ''}
            {line.modem.slot_conflict ? '（槽位冲突）' : ''}
          </MenuItem>
        ))}
      </Select>
    </FormControl>
  )
}
