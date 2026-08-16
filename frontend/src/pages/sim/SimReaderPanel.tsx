import { useEffect, useMemo, useState } from 'react'
import {
  Add,
  DeleteOutline,
  Refresh,
  Save,
  SimCardOutlined,
  Usb,
} from '@mui/icons-material'
import {
  Alert,
  Box,
  Button,
  Chip,
  FormControlLabel,
  Grid,
  IconButton,
  MenuItem,
  Paper,
  Stack,
  Switch,
  TextField,
  Tooltip,
  Typography,
} from '@mui/material'
import { api } from '../../api/current'
import type { PcscReaderInfo, StandaloneSimSlotConfig } from '../../api/contracts'

function newSlot(index: number): StandaloneSimSlotConfig {
  return {
    id: `reader-${Date.now()}-${index}`,
    label: `USB SIM 读卡器 ${index + 1}`,
    reader_path: '',
    uim_slot: 1,
    enabled: true,
  }
}

export default function SimReaderPanel() {
  const [slots, setSlots] = useState<StandaloneSimSlotConfig[]>([])
  const [readers, setReaders] = useState<PcscReaderInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState('')

  const readerBySelector = useMemo(
    () => new Map(readers.map((reader) => [reader.selector, reader])),
    [readers],
  )

  const load = async () => {
    setLoading(true)
    setError('')
    try {
      const [slotResponse, readerResponse] = await Promise.all([
        api.getStandaloneSimSlots(),
        api.getPcscReaders(),
      ])
      if (slotResponse.status !== 'ok' || !slotResponse.data) {
        throw new Error(slotResponse.message || '读取读卡器配置失败')
      }
      setSlots(slotResponse.data)
      if (readerResponse.status === 'ok' && readerResponse.data) {
        setReaders(readerResponse.data)
      } else {
        setReaders([])
        setError(readerResponse.message || 'PC/SC 服务或 OpenSC 工具不可用')
      }
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : '读取读卡器配置失败')
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    void load()
  }, [])

  const updateSlot = (index: number, patch: Partial<StandaloneSimSlotConfig>) => {
    setSlots((current) => current.map((slot, slotIndex) => (
      slotIndex === index ? { ...slot, ...patch } : slot
    )))
  }

  const save = async () => {
    setSaving(true)
    setError('')
    try {
      const response = await api.setStandaloneSimSlots(slots)
      if (response.status !== 'ok' || !response.data) {
        throw new Error(response.message || '保存读卡器配置失败')
      }
      setSlots(response.data)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : '保存读卡器配置失败')
    } finally {
      setSaving(false)
    }
  }

  return (
    <Stack spacing={2}>
      <Box display="flex" justifyContent="space-between" alignItems="center" gap={2} flexWrap="wrap">
        <Box display="flex" alignItems="center" gap={1}>
          <Usb color="primary" />
          <Typography variant="h6" fontWeight={700}>USB SIM 读卡器</Typography>
        </Box>
        <Stack direction="row" spacing={1}>
          <Tooltip title="重新扫描 PC/SC 读卡器">
            <span><IconButton onClick={() => void load()} disabled={loading}><Refresh /></IconButton></span>
          </Tooltip>
          <Button startIcon={<Add />} onClick={() => setSlots((current) => [...current, newSlot(current.length)])}>
            添加
          </Button>
          <Button variant="contained" startIcon={<Save />} onClick={() => void save()} disabled={saving || loading}>
            保存
          </Button>
        </Stack>
      </Box>

      {error && <Alert severity="warning">{error}</Alert>}

      {!loading && slots.length === 0 && (
        <Alert severity="info">当前没有配置独立读卡器。</Alert>
      )}

      {slots.map((slot, index) => {
        const reader = readerBySelector.get(slot.reader_path)
        return (
          <Paper key={slot.id} variant="outlined" sx={{ p: 2, borderRadius: 1 }}>
            <Grid container spacing={2} alignItems="center">
              <Grid size={{ xs: 12, md: 3 }}>
                <TextField
                  fullWidth
                  size="small"
                  label="线路名称"
                  value={slot.label}
                  onChange={(event) => updateSlot(index, { label: event.target.value })}
                />
              </Grid>
              <Grid size={{ xs: 12, md: 5 }}>
                <TextField
                  select
                  fullWidth
                  size="small"
                  label="PC/SC 读卡器"
                  value={slot.reader_path}
                  onChange={(event) => updateSlot(index, { reader_path: event.target.value })}
                >
                  {slot.reader_path && !readerBySelector.has(slot.reader_path) && (
                    <MenuItem value={slot.reader_path}>{slot.reader_path}（当前未发现）</MenuItem>
                  )}
                  {readers.map((item) => (
                    <MenuItem key={item.selector} value={item.selector}>
                      {item.name} · {item.card_present ? '已插卡' : '未插卡'}
                    </MenuItem>
                  ))}
                </TextField>
              </Grid>
              <Grid size={{ xs: 6, md: 1.5 }}>
                <TextField
                  fullWidth
                  size="small"
                  type="number"
                  label="槽位"
                  value={slot.uim_slot}
                  slotProps={{ htmlInput: { min: 1, max: 255 } }}
                  onChange={(event) => updateSlot(index, { uim_slot: Math.max(1, Number(event.target.value) || 1) })}
                />
              </Grid>
              <Grid size={{ xs: 6, md: 1.5 }}>
                <FormControlLabel
                  control={<Switch checked={slot.enabled} onChange={(_, checked) => updateSlot(index, { enabled: checked })} />}
                  label="启用"
                />
              </Grid>
              <Grid size={{ xs: 12, md: 2 }}>
                <Box display="flex" justifyContent={{ xs: 'space-between', md: 'flex-end' }} alignItems="center" gap={1}>
                  <Chip
                    icon={<SimCardOutlined />}
                    size="small"
                    color={reader?.card_present ? 'success' : 'default'}
                    label={reader?.card_present ? 'SIM 就绪' : reader ? '未插卡' : '离线'}
                  />
                  <Tooltip title="删除配置">
                    <IconButton color="error" onClick={() => setSlots((current) => current.filter((_, slotIndex) => slotIndex !== index))}>
                      <DeleteOutline />
                    </IconButton>
                  </Tooltip>
                </Box>
              </Grid>
            </Grid>
          </Paper>
        )
      })}
    </Stack>
  )
}
