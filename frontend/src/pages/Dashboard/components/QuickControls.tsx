import { Box, Card, CardContent, Typography, Stack, Switch, Chip } from '@mui/material'
import { TravelExplore, Tune } from '@mui/icons-material'
import type { RoamingResponse } from '@/api/types'

interface QuickControlsProps {
  roaming: RoamingResponse | null
  onToggleRoaming: () => void
}

export function QuickControls({
  roaming,
  onToggleRoaming,
}: QuickControlsProps) {
  return (
    <Card sx={{ height: '100%' }}>
      <CardContent>
        <Box display="flex" alignItems="center" gap={1} mb={2}>
          <Tune color="primary" />
          <Typography variant="subtitle1" fontWeight={700}>快捷控制</Typography>
        </Box>

        <Stack spacing={2}>
          <Box display="flex" alignItems="center" justifyContent="space-between">
            <Box display="flex" alignItems="center" gap={1}>
              <TravelExplore color={roaming?.roaming_allowed ? 'info' : 'disabled'} />
              <Typography variant="body2">漫游数据</Typography>
              {roaming?.is_roaming && (
                <Chip label="漫游中" size="small" color="warning" sx={{ height: 18, fontSize: '0.65rem' }} />
              )}
            </Box>
            <Switch
              checked={roaming?.roaming_allowed || false}
              onChange={() => {
                void onToggleRoaming()
              }}
              color="info"
              size="small"
            />
          </Box>

        </Stack>
      </CardContent>
    </Card>
  )
}
