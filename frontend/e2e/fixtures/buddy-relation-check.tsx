import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, Typography, createTheme } from '@mui/material'
import AppBuddyRelationCheckPane from '../../src/pages/sim/AppBuddyRelationCheckPane'
export default function Fixture() {
  const [alternate, setAlternate] = useState(false); const [revision, setRevision] = useState(1); const [hosted, setHosted] = useState(true)
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2, minHeight: 600 }}><Typography>Unknown-WRITE 保持；Buddy 检查不确认关系修改</Typography>
    <AppBuddyRelationCheckPane lineId={alternate ? 'app-account-fedcba9876543210fedcba9876543210' : 'app-account-0123456789abcdef0123456789abcdef'} revision={revision} hosted={hosted} installed />
    <Button onClick={() => setAlternate((v) => !v)}>切换检查账号</Button><Button onClick={() => setRevision((v) => v + 1)}>替换检查版本</Button><Button onClick={() => setHosted(false)}>停止夹具 SDK</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root'); if (!root) throw new Error('Missing Buddy check fixture root')
createRoot(root).render(<Fixture />)
