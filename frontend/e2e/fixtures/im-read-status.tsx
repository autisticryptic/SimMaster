import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, createTheme } from '@mui/material'
import AppImReadStatusPane from '../../src/pages/sim/AppImReadStatusPane'
export default function Fixture() {
  const [line, setLine] = useState('app-account-0123456789abcdef0123456789abcdef')
  const [hosted, setHosted] = useState(true)
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2 }}>
    <AppImReadStatusPane lineId={line} revision={1} hosted={hosted} installed />
    <Button onClick={() => setLine('app-account-fedcba9876543210fedcba9876543210')}>切换原读位置账号</Button>
    <Button onClick={() => setLine('app-account-0123456789abcdef0123456789abcdef')}>切回原读位置账号</Button>
    <Button onClick={() => setHosted(false)}>暂停读位置运行态</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root')
if (!root) throw new Error('Missing fixture root')
createRoot(root).render(<Fixture />)
