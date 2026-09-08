import { Alert, Box, CircularProgress, FormControl, InputLabel, MenuItem, Select, Stack, Typography } from '@mui/material'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { api, type ImsAccessPreference } from '../../api/current'
import ImsRegistrationPolicyHint from '../../components/ImsRegistrationPolicyHint'
import {
  isSelectableRegistrationPreference,
  registrationModeOptions,
  registrationPreferenceLabel,
  registrationSupportText,
} from '../../policies/imsRegistration'

const preferenceKey = (lineId: string) => ['ims-access-preference', lineId] as const
const statusKey = (lineId: string) => ['ims-registration-status', lineId] as const

export default function ImsRegistrationSettings({ lineId, disabled = false }: { lineId: string; disabled?: boolean }) {
  const client = useQueryClient()
  const preference = useQuery({
    queryKey: preferenceKey(lineId),
    queryFn: async () => {
      const response = await api.getImsAccessPreference(lineId)
      if (!response.data) throw new Error('未读取到 IMS 注册模式')
      return response.data
    },
    enabled: Boolean(lineId),
  })
  const status = useQuery({
    queryKey: statusKey(lineId),
    queryFn: async () => {
      const response = await api.getLineImsStatus(lineId)
      if (!response.data) throw new Error('未读取到 IMS 注册状态')
      return response.data
    },
    enabled: Boolean(lineId),
    refetchInterval: 10_000,
  })
  const save = useMutation({
    mutationFn: async (change: { lineId: string; preference: ImsAccessPreference }) => {
      const response = await api.setImsAccessPreference(change.lineId, change.preference)
      if (!response.data || response.data.preference !== change.preference) {
        throw new Error('服务器未确认注册模式，设置未更新')
      }
      return response.data
    },
    onSuccess: (data, change) => {
      // A late response belongs to its original line, never a newly selected SIM.
      client.setQueryData(preferenceKey(change.lineId), data)
      void client.invalidateQueries({ queryKey: statusKey(change.lineId) })
    },
  })
  const selected = preference.data?.preference ?? ''
  const policy = status.data?.registration_policy
  const error = save.error ?? preference.error ?? status.error

  return (
    <Stack spacing={1.25} sx={{ mt: 2, pt: 2, borderTop: 1, borderColor: 'divider' }}>
      <Box display="flex" alignItems="center" gap={1} flexWrap="wrap">
        <Typography variant="subtitle2">IMS 注册模式</Typography>
        {(preference.isPending || save.isPending) && <CircularProgress size={16} />}
      </Box>
      <FormControl fullWidth size="small" disabled={disabled || !preference.data || save.isPending}>
        <InputLabel id={`ims-registration-mode-${lineId}`}>注册模式</InputLabel>
        <Select
          labelId={`ims-registration-mode-${lineId}`}
          label="注册模式"
          value={selected}
          onChange={(event) => {
            const next = event.target.value
            if (isSelectableRegistrationPreference(next) && next !== selected) {
              save.mutate({ lineId, preference: next })
            }
          }}
        >
          {registrationModeOptions.map((option) => <MenuItem key={option.value} value={option.value}>{option.label}</MenuItem>)}
          {selected === 'cellular_preferred' && (
            <MenuItem value={selected} disabled>{registrationPreferenceLabel(selected)}</MenuItem>
          )}
        </Select>
      </FormControl>
      <Typography variant="caption" color="text.secondary">
        自动模式默认请求多流能力；网络接受且通道有效时双注册，否则优先 VoWiFi，再考虑已启用的 4G/5G IMS。CS 仅作为允许的短信／通话业务回退，不是 IMS 注册。
      </Typography>
      <Typography variant="caption" color="text.secondary">
        此设置不修改两路连接开关，也不解除短信或 Trunk 的“仅 VoWiFi”资费限制。有通话时延后注册切换，已有通话不迁移。
      </Typography>
      {policy && <Typography variant="caption" color="text.secondary">{registrationSupportText(policy)}</Typography>}
      <ImsRegistrationPolicyHint ims={status.data ?? null} />
      {save.isSuccess && save.variables.lineId === lineId && (
        <Alert severity="success">注册模式已保存；实际应用状态以上方显示为准。</Alert>
      )}
      {error && <Alert severity="error">{error.message}</Alert>}
    </Stack>
  )
}
