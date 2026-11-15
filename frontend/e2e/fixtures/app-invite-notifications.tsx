import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, createTheme } from '@mui/material'
import AppInviteNotificationsPane from '../../src/pages/sim/AppInviteNotificationsPane'
export default function Fixture() {
  const [line, setLine] = useState('app-account-0123456789abcdef0123456789abcdef')
  const [enabled, setEnabled] = useState(true)
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2, minHeight: 600 }}>
    <AppInviteNotificationsPane lineId={line} enabled={enabled} />
    <Button onClick={() => setLine('app-account-fedcba9876543210fedcba9876543210')}>切换原账号</Button>
    <Button onClick={() => setEnabled(false)}>停止本地状态读取</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root')
if (!root) throw new Error('Missing fixture root')
createRoot(root).render(<Fixture />)
