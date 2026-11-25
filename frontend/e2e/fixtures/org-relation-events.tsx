import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, Typography, createTheme } from '@mui/material'
import AppOrgRelationEventsPane from '../../src/pages/sim/AppOrgRelationEventsPane'
export default function Fixture() {
  const [alternate, setAlternate] = useState(false); const [revision, setRevision] = useState(1)
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2, minHeight: 600 }}>
    <Typography>出站 WRITE：Unknown（系统关系事件不能确认）</Typography>
    <AppOrgRelationEventsPane lineId={alternate ? 'app-account-fedcba9876543210fedcba9876543210' : 'app-account-0123456789abcdef0123456789abcdef'} revision={revision} />
    <Button onClick={() => setAlternate((v) => !v)}>切换系统事件账号</Button><Button onClick={() => setRevision((v) => v + 1)}>替换系统事件版本</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root'); if (!root) throw new Error('Missing original system control fixture root')
createRoot(root).render(<Fixture />)
