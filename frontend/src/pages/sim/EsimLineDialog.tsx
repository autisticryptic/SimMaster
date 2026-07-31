import { Close } from '@mui/icons-material'
import { Box, Dialog, DialogContent, DialogTitle, IconButton, Typography } from '@mui/material'
import EsimManagerPage from '../EsimManager'
import { modemSlotLabel, shortLineId } from '../../components/modemLineFormat'
import type { ModemBinding } from '../../api/current'

interface Props {
  open: boolean
  modem: ModemBinding | null
  onClose: () => void
}

/**
 * Per-line eSIM management. Reuses the full eUICC/Profile/write-card manager UI
 * but scopes every lpac call to this line's reader via the `lineId` prop, so a
 * multi-reader device manages each card independently.
 */
export default function EsimLineDialog({ open, modem, onClose }: Props) {
  if (!modem) return null
  const isReader = modem.line_kind === 'reader'
  return (
    <Dialog open={open} onClose={onClose} fullWidth maxWidth="lg">
      <DialogTitle sx={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 2 }}>
        <Box minWidth={0}>
          <Typography variant="h6">
            {isReader ? '读卡器' : modemSlotLabel(modem)} · 卡槽 {modem.uim_slot} · eSIM 管理
          </Typography>
          <Typography variant="caption" color="text.secondary">
            线路 {shortLineId(modem.line_id)}
          </Typography>
        </Box>
        <IconButton aria-label="关闭" onClick={onClose}><Close /></IconButton>
      </DialogTitle>
      <DialogContent dividers>
        <EsimManagerPage lineId={modem.line_id} />
      </DialogContent>
    </Dialog>
  )
}
