import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, Typography, createTheme } from '@mui/material'
import AppRelationHistoryPane from '../../src/pages/sim/AppRelationHistoryPane'
export default function Fixture() {
  const [line, setLine] = useState('app-account-0123456789abcdef0123456789abcdef')
  const [revision, setRevision] = useState(1); const [hosted, setHosted] = useState(true)
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2, minHeight: 600 }}>
    <Typography>SDK 观测：{hosted ? 'hosted' : 'stopped'}（不是关系写权限）</Typography>
    <AppRelationHistoryPane lineId={line} revision={revision} />
    <Button onClick={() => setLine('app-account-fedcba9876543210fedcba9876543210')}>切换记录账号</Button>
    <Button onClick={() => setRevision(2)}>替换记录配置版本</Button>
    <Button onClick={() => setHosted(false)}>停止夹具 SDK</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root')
if (!root) throw new Error('Missing relation history fixture root')
createRoot(root).render(<Fixture />)
