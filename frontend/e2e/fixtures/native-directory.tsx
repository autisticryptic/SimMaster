import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, CssBaseline, Stack, ThemeProvider, createTheme } from '@mui/material'
import AppNativeDirectoryPane from '../../src/pages/sim/AppNativeDirectoryPane'
export default function Fixture() {
  const [line, setLine] = useState('app-account-0123456789abcdef0123456789abcdef')
  const [revision, setRevision] = useState(1)
  const [hosted, setHosted] = useState(true)
  const [installed, setInstalled] = useState(true)
  return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2, minHeight: 600 }}>
    <AppNativeDirectoryPane lineId={line} revision={revision} hosted={hosted} installed={installed} />
    <Button onClick={() => setLine('app-account-fedcba9876543210fedcba9876543210')}>切换目录账号</Button>
    <Button onClick={() => setRevision(2)}>替换目录配置版本</Button>
    <Button onClick={() => setHosted(false)}>退出原托管状态</Button>
    <Button onClick={() => setInstalled(false)}>关闭原目录策略</Button>
  </Stack></ThemeProvider>
}
const root = document.getElementById('root')
if (!root) throw new Error('Missing native directory fixture root')
createRoot(root).render(<Fixture />)
