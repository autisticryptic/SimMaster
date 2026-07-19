import { useCallback, useEffect, useState } from 'react'
import {
  Alert, Box, Button, Card, CardContent, CardHeader, CircularProgress,
  IconButton, Stack, Switch, TextField, Tooltip, Typography,
} from '@mui/material'
import Grid from '@mui/material/Grid'
import { Add, Delete, Save, Usb } from '@mui/icons-material'
import { api, type StandaloneSimSlotConfig } from '../../api/current'

function newSlot(): StandaloneSimSlotConfig {
  return {
    id: `reader-${Date.now()}`,
    label: '',
    reader_path: '',
    uim_slot: 1,
    enabled: true,
  }
}

export default function StandaloneSimSlotsPanel() {
  const [slots, setSlots] = useState<StandaloneSimSlotConfig[]>([])
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    try {
      const response = await api.getStandaloneSimSlots()
      setSlots(response.data ?? [])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  const update = (index: number, patch: Partial<StandaloneSimSlotConfig>) => {
    setSlots((current) => current.map((slot, itemIndex) => itemIndex === index ? { ...slot, ...patch } : slot))
  }

  const save = async () => {
    if (slots.some((slot) => !slot.label.trim() || !slot.reader_path.trim() || slot.uim_slot < 1)) {
      setError('每个独立卡槽都需要名称、读取器路径和有效槽位号')
      return
    }
    setSaving(true)
    setError(null)
    setSuccess(null)
    try {
      const response = await api.setStandaloneSimSlots(slots)
      setSlots(response.data ?? slots)
      setSuccess('独立读卡器与卡槽配置已保存')
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Card>
      <CardHeader
        avatar={<Usb color="primary" />}
        title="独立 SIM 读卡器与卡槽"
        subheader="为不依赖蜂窝基带的 VoWiFi SIM 预留稳定槽位"
        action={<Button startIcon={<Add />} onClick={() => setSlots((current) => [...current, newSlot()])}>添加卡槽</Button>}
      />
      <CardContent sx={{ pt: 0 }}>
        <Alert severity="info" sx={{ mb: 2 }}>
          可登记 PC/SC、QMI UIM 或后续适配器路径。当前仅负责持久化槽位身份；硬件接入后由对应读取器适配器完成 SIM AKA。
        </Alert>
        {error && <Alert severity="error" sx={{ mb: 2 }} onClose={() => setError(null)}>{error}</Alert>}
        {success && <Alert severity="success" sx={{ mb: 2 }} onClose={() => setSuccess(null)}>{success}</Alert>}
        {loading ? <Box display="flex" justifyContent="center" py={3}><CircularProgress size={28} /></Box> : (
          <Stack spacing={1.5}>
            {slots.length === 0 && <Typography variant="body2" color="text.secondary">尚未登记独立读卡器。</Typography>}
            {slots.map((slot, index) => (
              <Grid container spacing={1.5} alignItems="center" key={slot.id}>
                <Grid size={{ xs: 12, md: 3 }}>
                  <TextField fullWidth size="small" label="槽位名称" value={slot.label} onChange={(event) => update(index, { label: event.target.value })} />
                </Grid>
                <Grid size={{ xs: 12, md: 6 }}>
                  <TextField fullWidth size="small" label="读取器路径 / 标识" value={slot.reader_path} placeholder="pcsc://Reader 0 或 /dev/cdc-wdm1" onChange={(event) => update(index, { reader_path: event.target.value })} />
                </Grid>
                <Grid size={{ xs: 6, md: 1.5 }}>
                  <TextField fullWidth size="small" type="number" label="槽位" value={slot.uim_slot} slotProps={{ htmlInput: { min: 1, max: 255 } }} onChange={(event) => update(index, { uim_slot: Number(event.target.value) })} />
                </Grid>
                <Grid size={{ xs: 6, md: 1.5 }} display="flex" alignItems="center" justifyContent="flex-end">
                  <Switch checked={slot.enabled} onChange={(_, enabled) => update(index, { enabled })} />
                  <Tooltip title="删除卡槽"><IconButton color="error" onClick={() => setSlots((current) => current.filter((_, itemIndex) => itemIndex !== index))}><Delete /></IconButton></Tooltip>
                </Grid>
              </Grid>
            ))}
            <Box display="flex" justifyContent="flex-end">
              <Button variant="contained" startIcon={<Save />} onClick={() => void save()} disabled={saving || loading}>
                {saving ? '保存中...' : '保存读卡器配置'}
              </Button>
            </Box>
          </Stack>
        )}
      </CardContent>
    </Card>
  )
}
