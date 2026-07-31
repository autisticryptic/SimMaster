import { useEffect, useState } from 'react'
import {
  Alert, Button, Checkbox, Dialog, DialogActions, DialogContent, DialogTitle,
  FormControlLabel, IconButton, List, ListItem, ListItemText, Stack, Switch, Typography,
} from '@mui/material'
import { ArrowUpward, ArrowDownward } from '@mui/icons-material'
import { api, type VolteIpFamily, type VolteLineControlResponse } from '../../api/current'
import { shortLineId } from '../../components/modemLineFormat'

interface Props {
  open: boolean
  lineId: string | null
  /** The line's persisted `volte_ip_families`; null/undefined means "follow default". */
  families: VolteIpFamily[] | null | undefined
  onClose: () => void
  onSaved: (line: VolteLineControlResponse) => void
}

const familyLabel: Record<VolteIpFamily, string> = {
  ipv4v6: '双栈 (IPv4v6)',
  ipv4: 'IPv4',
  ipv6: 'IPv6',
}

// All attempt kinds, in the default order (dual-stack → IPv4 → IPv6), which is
// what "follow default" resolves to. A custom list is any reordering / subset.
const ALL_FAMILIES: VolteIpFamily[] = ['ipv4v6', 'ipv4', 'ipv6']

// Normalize a persisted list into an ordered [attempt, enabled] table so every
// row always renders (an unchecked row keeps its place for reordering).
function toRows(families: VolteIpFamily[] | null | undefined): { family: VolteIpFamily; enabled: boolean }[] {
  if (!families || families.length === 0) {
    return ALL_FAMILIES.map((family) => ({ family, enabled: true }))
  }
  const enabled = families.map((family) => ({ family, enabled: true }))
  const missing = ALL_FAMILIES.filter((family) => !families.includes(family)).map((family) => ({ family, enabled: false }))
  return [...enabled, ...missing]
}

export default function VolteLineDialog({ open, lineId, families, onClose, onSaved }: Props) {
  const [custom, setCustom] = useState(false)
  const [rows, setRows] = useState<{ family: VolteIpFamily; enabled: boolean }[]>(toRows(null))
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (open) {
      setCustom(Boolean(families && families.length > 0))
      setRows(toRows(families))
      setError(null)
    }
  }, [open, families])

  const move = (index: number, delta: number) => {
    setRows((current) => {
      const next = [...current]
      const target = index + delta
      if (target < 0 || target >= next.length) return current
      ;[next[index], next[target]] = [next[target], next[index]]
      return next
    })
  }

  const toggle = (index: number) => {
    setRows((current) => current.map((row, i) => (i === index ? { ...row, enabled: !row.enabled } : row)))
  }

  const selected = rows.filter((row) => row.enabled).map((row) => row.family)
  const validationError = custom && selected.length === 0 ? '至少启用一项' : null

  const save = async () => {
    if (!lineId || validationError) return
    setSaving(true)
    setError(null)
    try {
      // Toggle off = follow the global default (null). On = the ordered enabled list.
      const payload = custom ? selected : null
      const response = await api.setVolteLineIpFamilies(lineId, payload)
      if (response.data) onSaved(response.data)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  if (!lineId) return null

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="sm">
      <DialogTitle>VoLTE 地址族 · {shortLineId(lineId)}</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <Alert severity="info">
            按下面的顺序依次尝试建立 IMS 承载，失败则回退到下一项。双栈也是可排序项，
            可以放在任意位置或整个不启用。网络显式要求 IPv6-only / IPv4-only 时以网络为准。
          </Alert>
          <FormControlLabel
            control={<Switch checked={custom} onChange={(event) => setCustom(event.target.checked)} />}
            label="自定义尝试顺序（关闭时跟随全局默认：双栈 → IPv4 → IPv6）"
          />
          {custom && (
            <List dense>
              {rows.map((row, index) => (
                <ListItem
                  key={row.family}
                  secondaryAction={
                    <Stack direction="row" spacing={0.5}>
                      <IconButton size="small" onClick={() => move(index, -1)} disabled={index === 0}><ArrowUpward fontSize="small" /></IconButton>
                      <IconButton size="small" onClick={() => move(index, 1)} disabled={index === rows.length - 1}><ArrowDownward fontSize="small" /></IconButton>
                    </Stack>
                  }
                >
                  <Checkbox edge="start" checked={row.enabled} onChange={() => toggle(index)} />
                  <ListItemText
                    primary={familyLabel[row.family]}
                    secondary={index === 0 ? '优先尝试' : `回退顺序 ${index + 1}`}
                  />
                </ListItem>
              ))}
            </List>
          )}
          {custom && (
            <Typography variant="caption" color="text.secondary">
              当前顺序：{selected.length > 0 ? selected.map((family) => familyLabel[family]).join(' → ') : '（未启用任何项）'}
              {selected.length === 1 ? '（仅此一项，不回退）' : ''}
            </Typography>
          )}
          {validationError && <Alert severity="error">{validationError}</Alert>}
          {error && <Alert severity="error">{error}</Alert>}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button variant="contained" onClick={() => void save()} disabled={saving || Boolean(validationError)}>
          {saving ? '保存中...' : '保存配置'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
