import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, Typography, createTheme } from '@mui/material'
import AppGroupManagementPane from '../../src/pages/sim/AppGroupManagementPane'
import AppRelationWritePane from '../../src/pages/sim/AppRelationWritePane'
function Fixture() {
  const [revision, setRevision] = useState(1); const [hosted, setHosted] = useState(true)
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2 }}><Typography>群组管理与成员 WRITE 共用未决排他</Typography>
    <AppGroupManagementPane lineId="app-account-0123456789abcdef0123456789abcdef" revision={revision} hosted={hosted} installed />
    <AppRelationWritePane lineId="app-account-0123456789abcdef0123456789abcdef" revision={revision} hosted={hosted} installed />
    <Button onClick={() => setRevision((v) => v + 1)}>切换群组管理版本</Button><Button onClick={() => setHosted(false)}>停止夹具 SDK</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root'); if (!root) throw new Error('Missing group fixture root')
createRoot(root).render(<Fixture />)
