import { Checkbox, FormControlLabel, MenuItem, Stack, TextField, Alert } from '@mui/material'
import type { JegotripCommunicationAction, JegotripCommunicationOperation } from '../../api/current'
import { communicationActionLabel, communicationWrites, isCommunicationAction } from '../../policies/communicationOps'

export default function JegotripCommunicationInput({ actions, operation, writeConsent, disabled, onChange, onConsent }: {
  actions: JegotripCommunicationAction[]; operation?: JegotripCommunicationOperation; writeConsent: boolean; disabled: boolean
  onChange: (operation: JegotripCommunicationOperation | undefined) => void; onConsent: (consent: boolean) => void
}) {
  return <Stack spacing={1}>
    <Alert severity="info">仅使用当前原UUID和本人账号的本次App会话。查询不授予SDK权限；业务写须安装侧批准并再次勾选。App退出、电话服务、来显或关闭呼转不等于本地SDK停止。结果未知时保留原任务，不换任务标识重发。</Alert>
    <TextField select size="small" label="本次通信操作" value={operation?.kind ?? ''} disabled={disabled}
      onChange={(event) => {
        const kind = event.target.value
        if (!isCommunicationAction(kind)) { onChange(undefined); return }
        if (kind === 'set_phone_service') onChange({ kind, status: NaN })
        else if (kind === 'set_voip_profile') onChange({ kind, cs_forward: '' })
        else if (kind === 'set_jego_m_status') onChange({ kind, jegom_status: '', region_code: '', country_code: '' })
        else onChange({ kind })
      }}>
      <MenuItem value="">选择明确操作</MenuItem>
      {actions.map((action) => <MenuItem key={action} value={action}>{communicationActionLabel(action)}</MenuItem>)}
    </TextField>
    {operation?.kind === 'set_phone_service' && <TextField size="small" label="电话服务原状态整数" type="number" disabled={disabled}
      value={Number.isFinite(operation.status) ? operation.status : ''} inputProps={{ min: 0, max: 999999 }}
      helperText="按本人原App服务配置填写整数，不把未知代码猜成开/关。"
      onChange={(event) => onChange({ ...operation, status: event.target.value === '' ? NaN : Number(event.target.value) })} />}
    {operation?.kind === 'set_voip_profile' && <TextField select size="small" label="cs_forward原值" disabled={disabled} value={operation.cs_forward}
      onChange={(event) => onChange({ ...operation, cs_forward: event.target.value })}>
      <MenuItem value="">选择原值</MenuItem><MenuItem value="1">1</MenuItem><MenuItem value="2">2</MenuItem>
    </TextField>}
    {operation?.kind === 'set_jego_m_status' && <Stack spacing={1}>
      <TextField select size="small" label="jegom_status原值" value={operation.jegom_status} disabled={disabled}
        onChange={(event) => onChange({ ...operation, jegom_status: event.target.value })}>
        <MenuItem value="">选择原值</MenuItem><MenuItem value="1">1</MenuItem><MenuItem value="2">2</MenuItem>
      </TextField>
      <TextField size="small" label="region_code原值" value={operation.region_code} disabled={disabled} inputProps={{ maxLength: 6 }}
        onChange={(event) => onChange({ ...operation, region_code: event.target.value })} />
      <TextField size="small" label="country_code原值" value={operation.country_code} disabled={disabled} inputProps={{ maxLength: 6 }}
        onChange={(event) => onChange({ ...operation, country_code: event.target.value })} />
    </Stack>}
    {operation && communicationWrites(operation.kind) && <FormControlLabel control={<Checkbox checked={writeConsent} disabled={disabled}
      onChange={(event) => onConsent(event.target.checked)} />} label="我明确批准本次远端业务写操作；可能影响原手机业务或会话，不是本地接收器控制" />}
  </Stack>
}
