import { useEffect, useMemo, useState } from 'react'
import {
  Alert, Box, Button, CircularProgress, Dialog, DialogActions, DialogContent, DialogTitle,
  FormControl, FormHelperText, InputLabel, MenuItem, Select, Stack, TextField, Typography,
} from '@mui/material'
import { api, type Non3gppProviderView, type VirtualSimProfileView } from '../../api/current'

interface Props {
  open: boolean
  onClose: () => void
  onSaved: (profile: VirtualSimProfileView) => void
}

type AccessKind = 'five_codes' | 'app_voice'
type KeyKind = 'op' | 'opc'

interface Draft {
  iccid: string
  imsi: string
  ki: string
  keyKind: KeyKind
  keyValue: string
  msisdn: string
}

const emptyDraft: Draft = { iccid: '', imsi: '', ki: '', keyKind: 'opc', keyValue: '', msisdn: '' }

const hexOk = (value: string) => /^[0-9a-fA-F]{32}$/.test(value.trim())
const digitsOk = (value: string, min: number, max: number) => {
  const text = value.trim()
  return text.length >= min && text.length <= max && /^\d+$/.test(text)
}

const errorMessages: Record<string, string> = {
  ims_virtual_sim_key_invalid: 'Ki 或 OP/OPc 必须是 32 位十六进制',
  ims_virtual_sim_op_opc_conflict: 'OP 与 OPc 只能填一个',
  ims_virtual_sim_op_missing: '请填写 OP 或 OPc',
  ims_virtual_sim_iccid_invalid: 'ICCID 必须是 18–22 位数字',
  ims_virtual_sim_imsi_invalid: 'IMSI 必须是 5–16 位数字',
  ims_virtual_sim_msisdn_invalid: 'MSISDN 只能是数字',
  ims_virtual_sim_storage_failed: '本地数据库写入失败',
}

function describeError(error: unknown) {
  const text = error instanceof Error ? error.message : String(error)
  for (const [code, message] of Object.entries(errorMessages)) {
    if (text.includes(code)) return message
  }
  return text || '保存失败'
}

export default function VirtualSimDialog({ open, onClose, onSaved }: Props) {
  const [accessKind, setAccessKind] = useState<AccessKind>('five_codes')
  const [draft, setDraft] = useState<Draft>(emptyDraft)
  const [providers, setProviders] = useState<Non3gppProviderView[]>([])

  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open) return
    setDraft(emptyDraft)
    setAccessKind('five_codes')
    setError(null)
    let cancelled = false
    api
      .getNon3gppProviders()
      .then((response) => {
        if (!cancelled) setProviders(response.data ?? [])
      })
      .catch(() => {
        if (!cancelled) setProviders([])
      })
    return () => {
      cancelled = true
    }
  }, [open])

  const validation = useMemo(() => {
    const iccid = digitsOk(draft.iccid, 18, 22)
    const imsi = digitsOk(draft.imsi, 5, 16)
    const ki = hexOk(draft.ki)
    const keyValue = hexOk(draft.keyValue)
    const msisdn = draft.msisdn.trim() === '' || /^\d{3,20}$/.test(draft.msisdn.trim())
    return {
      iccid,
      imsi,
      ki,
      keyValue,
      msisdn,
      ready: iccid && imsi && ki && keyValue && msisdn,
    }
  }, [draft])

  const submit = async () => {
    if (!validation.ready || saving) return
    setSaving(true)
    setError(null)
    try {
      const response = await api.createVirtualSimProfile({
        iccid: draft.iccid.trim(),
        imsi: draft.imsi.trim(),
        ki: draft.ki.trim(),
        ...(draft.keyKind === 'op'
          ? { op: draft.keyValue.trim() }
          : { opc: draft.keyValue.trim() }),
        ...(draft.msisdn.trim() ? { msisdn: draft.msisdn.trim() } : {}),
      })
      const profile = response.data
      if (!profile) throw new Error(response.message || '保存失败')
      // Keys never stay on screen after a successful save.
      setDraft(emptyDraft)
      onSaved(profile)
      onClose()
    } catch (submitError) {
      setError(describeError(submitError))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="sm">
      <DialogTitle>新建 WLAN 线路</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <FormControl fullWidth size="small">
            <InputLabel id="virtual-access-kind-label">接入类型</InputLabel>
            <Select
              labelId="virtual-access-kind-label"
              label="接入类型"
              value={accessKind}
              onChange={(event) => setAccessKind(event.target.value as AccessKind)}
            >
              <MenuItem value="five_codes">五码（虚拟 SIM）</MenuItem>
              {providers.map((provider) => (
                <MenuItem key={provider.id} value={`provider:${provider.id}`} disabled>
                  {provider.label}（{provider.availability === 'unsupported' ? '接口已预留，未接入' : '未接入'}）
                </MenuItem>
              ))}
            </Select>
            <FormHelperText>
              新建的线路与现有模组线路并存，不需要插入 SIM 卡。
            </FormHelperText>
          </FormControl>

          <Box>
            <Typography variant="subtitle2" sx={{ mb: 1 }}>
              身份
            </Typography>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                size="small"
                label="ICCID"
                value={draft.iccid}
                error={draft.iccid !== '' && !validation.iccid}
                helperText={draft.iccid !== '' && !validation.iccid ? '18–22 位数字' : ' '}
                onChange={(event) => setDraft({ ...draft, iccid: event.target.value })}
              />
              <TextField
                fullWidth
                size="small"
                label="IMSI"
                value={draft.imsi}
                error={draft.imsi !== '' && !validation.imsi}
                helperText={draft.imsi !== '' && !validation.imsi ? '5–16 位数字' : ' '}
                onChange={(event) => setDraft({ ...draft, imsi: event.target.value })}
              />
            </Stack>
            <TextField
              fullWidth
              size="small"
              label="MSISDN（可选）"
              value={draft.msisdn}
              error={!validation.msisdn}
              helperText={validation.msisdn ? ' ' : '只能是数字'}
              onChange={(event) => setDraft({ ...draft, msisdn: event.target.value })}
            />
          </Box>

          <Box>
            <Typography variant="subtitle2" sx={{ mb: 1 }}>
              密钥
            </Typography>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                size="small"
                label="Ki"
                value={draft.ki}
                error={draft.ki !== '' && !validation.ki}
                helperText={draft.ki !== '' && !validation.ki ? '32 位十六进制' : ' '}
                onChange={(event) => setDraft({ ...draft, ki: event.target.value })}
              />
              <TextField
                select
                size="small"
                label="密钥类型"
                value={draft.keyKind}
                sx={{ minWidth: 120 }}
                onChange={(event) => setDraft({ ...draft, keyKind: event.target.value as KeyKind })}
              >
                <MenuItem value="opc">OPc</MenuItem>
                <MenuItem value="op">OP</MenuItem>
              </TextField>
              <TextField
                fullWidth
                size="small"
                label={draft.keyKind === 'op' ? 'OP' : 'OPc'}
                value={draft.keyValue}
                error={draft.keyValue !== '' && !validation.keyValue}
                helperText={draft.keyValue !== '' && !validation.keyValue ? '32 位十六进制' : ' '}
                onChange={(event) => setDraft({ ...draft, keyValue: event.target.value })}
              />
            </Stack>
            <Typography variant="caption" color="text.secondary">
              只用于 VoWiFi 鉴权；密钥加密后存库，保存后不再显示。
            </Typography>
          </Box>

          {error ? <Alert severity="error">{error}</Alert> : null}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>
          取消
        </Button>
        <Button variant="contained" onClick={() => void submit()} disabled={!validation.ready || saving}>
          {saving ? <CircularProgress size={18} color="inherit" /> : '保存并启用'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
