import { Box, Card, CardContent, Typography } from '@mui/material'
import { Tune } from '@mui/icons-material'

export function QuickControls() {
  return (
    <Card sx={{ height: '100%' }}>
      <CardContent>
        <Box display="flex" alignItems="center" gap={1} mb={2}>
          <Tune color="primary" />
          <Typography variant="subtitle1" fontWeight={700}>快捷控制</Typography>
        </Box>

        <Box minHeight={72} />
      </CardContent>
    </Card>
  )
}
