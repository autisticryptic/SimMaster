import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, Typography, createTheme } from '@mui/material'
import AppRelationEventsPane from '../../src/pages/sim/AppRelationEventsPane'
export default function Fixture() {
  const [alternate, setAlternate] = useState(false); const [revision, setRevision] = useState(1)
  const line = alternate ? 'app-account-fedcba9876543210fedcba9876543210' : 'app-account-0123456789abcdef0123456789abcdef'
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2, minHeight: 600 }}>
    <Typography>出站关系 WRITE：Unknown（入站事件不能清除）</Typography>
    <Typography>记录配置版本：{revision}</Typography>
    <AppRelationEventsPane lineId={line} revision={revision} />
    <Button onClick={() => setAlternate((value) => !value)}>切换事件账号</Button>
    <Button onClick={() => setRevision((value) => value + 1)}>替换事件配置版本</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root')
if (!root) throw new Error('Missing relation event fixture root')
createRoot(root).render(<Fixture />)
