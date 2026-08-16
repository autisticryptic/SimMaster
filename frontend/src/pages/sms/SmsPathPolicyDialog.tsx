import { useEffect, useState } from 'react'
import {
  Alert, Box, Button, CircularProgress, Dialog, DialogActions, DialogContent, DialogTitle,
  FormControlLabel, Switch,
} from '@mui/material'
import { api, type SmsPathPolicy } from '../../api/current'

type Props = { open: boolean; lineId: string; onClose: () => void }

export default function SmsPathPolicyDialog({ open, lineId, onClose }: Props) {
  const [policy, setPolicy] = useState<SmsPathPolicy | null>(null)
  const [loading, setLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!open || !lineId) return
    setLoading(true)
    setError(null)
    setPolicy(null)
    void api.getSmsPathPolicy(lineId)
      .then((response) => setPolicy(response.data ?? null))
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoading(false))
  }, [lineId, open])

  const save = async () => {
    if (!policy) return
    setSaving(true)
    setError(null)
    try {
      const response = await api.setSmsPathPolicy(lineId, policy)
      if (response.data) setPolicy(response.data)
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="sm">
      <DialogTitle>短信发送设置</DialogTitle>
      <DialogContent dividers>
        <Alert severity="info" sx={{ mb: 2 }}>
          开启后短信只通过 VoWiFi 发送，不会回落到可能产生漫游资费的 VoLTE 或基带短信。
        </Alert>
        {error && <Alert severity="error" sx={{ mb: 2 }}>{error}</Alert>}
        {loading || !policy ? (
          <Box display="flex" justifyContent="center" py={4}><CircularProgress /></Box>
        ) : (
          <FormControlLabel
            control={<Switch checked={policy.force_vowifi_send} onChange={(_, value) => setPolicy({ ...policy, force_vowifi_send: value })} />}
            label="强制使用 VoWiFi 发送短信"
          />
        )}
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button variant="contained" onClick={() => void save()} disabled={!policy || loading || saving}>{saving ? '保存中…' : '保存'}</Button>
      </DialogActions>
    </Dialog>
  )
}
