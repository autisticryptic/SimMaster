import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, createTheme } from '@mui/material'
import AppRelationWritePane from '../../src/pages/sim/AppRelationWritePane'
export default function Fixture() {
  const [line, setLine] = useState('app-account-0123456789abcdef0123456789abcdef'); const [revision, setRevision] = useState(1); const [hosted, setHosted] = useState(true); const [installed, setInstalled] = useState(true)
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2, minHeight: 600 }}>
    <AppRelationWritePane lineId={line} revision={revision} hosted={hosted} installed={installed} />
    <Button onClick={() => setLine((old) => old.includes('fedcba') ? 'app-account-0123456789abcdef0123456789abcdef' : 'app-account-fedcba9876543210fedcba9876543210')}>切换关系修改账号</Button>
    <Button onClick={() => setRevision((old) => old + 1)}>替换关系修改配置版本</Button>
    <Button onClick={() => setHosted(false)}>停止关系夹具 SDK</Button>
    <Button onClick={() => setInstalled(false)}>关闭关系 WRITE 策略</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root'); if (!root) throw new Error('Missing original relation write fixture root')
createRoot(root).render(<Fixture />)
