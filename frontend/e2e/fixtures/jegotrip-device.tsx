import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { Button, Container, CssBaseline, ThemeProvider, createTheme } from '@mui/material'
import AppAccountLoginPane from '../../src/pages/sim/AppAccountLoginPane'
import JegotripConfigurationDialog from '../../src/pages/sim/JegotripConfigurationDialog'

// Exact production components, with no SDK/runtime or device integration.
function Fixture() {
  const [configurationOpen, setConfigurationOpen] = useState(false)
  return <Container maxWidth="md" sx={{ py: 2 }}>
    <AppAccountLoginPane open />
    <Button onClick={() => setConfigurationOpen(true)}>无忧行安装配置</Button>
    <JegotripConfigurationDialog open={configurationOpen} onClose={() => setConfigurationOpen(false)} onSaved={() => undefined} />
  </Container>
}
const root = document.getElementById('root')
if (root) createRoot(root).render(<ThemeProvider theme={createTheme()}><CssBaseline /><Fixture /></ThemeProvider>)
