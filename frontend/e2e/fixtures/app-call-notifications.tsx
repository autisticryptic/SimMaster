import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, createTheme } from '@mui/material'
import AppCallNotificationsPane from '../../src/pages/sim/AppCallNotificationsPane'
function Fixture() {
  const [line, setLine] = useState('app-account-0123456789abcdef0123456789abcdef')
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2, minHeight: 600 }}>
    <AppCallNotificationsPane lineId={line} />
    <Button onClick={() => setLine('app-account-fedcba9876543210fedcba9876543210')}>切换原账号</Button>
  </Stack></ThemeProvider>
}
createRoot(document.getElementById('root')!).render(<Fixture />)
