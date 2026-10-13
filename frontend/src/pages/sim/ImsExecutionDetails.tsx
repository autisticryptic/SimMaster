import { Box, Typography } from '@mui/material'
import Grid from '@mui/material/Grid'
import type { ImsRegistrationDiagnostics } from '../../api/contracts.ts'
import { imsDiagnosticRows } from './imsDiagnosticsFormat.ts'

export function ImsExecutionDetails({ facts, access }: { facts?: ImsRegistrationDiagnostics | null; access: 'cellular' | 'wifi' }) {
  const rows = imsDiagnosticRows(facts)
  if (!rows.length) return null
  return (
    <Box component="details" data-testid={`ims-diagnostics-${access}`} sx={{ border: 1, borderColor: 'divider', borderRadius: 1, p: 1.5 }}>
      <Box component="summary" sx={{ cursor: 'pointer', fontWeight: 600 }}>实际注册与恢复依据</Box>
      <Typography variant="caption" color="text.secondary" component="p" sx={{ mt: 1, mb: 1.5 }}>
        以下来自最近执行记录，不把配置值当作已协商结果。socket 创建不代表网络注册，最近成功协商也不代表当前仍已注册。
      </Typography>
      <Grid container spacing={1.5}>
        {rows.map(({ label, value }) => (
          <Grid key={label} size={{ xs: 12, sm: 6 }}>
            <Typography variant="caption" color="text.secondary">{label}</Typography>
            <Typography variant="body2" sx={{ mt: 0.25, overflowWrap: 'anywhere' }}>{value}</Typography>
          </Grid>
        ))}
      </Grid>
    </Box>
  )
}
