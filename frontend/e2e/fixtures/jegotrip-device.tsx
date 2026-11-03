import { createRoot } from 'react-dom/client'
import { Container, CssBaseline, ThemeProvider, createTheme } from '@mui/material'
import AppAccountLoginPane from '../../src/pages/sim/AppAccountLoginPane'

// Exact production component, with no SDK/runtime or device integration.
const root = document.getElementById('root')
if (root) createRoot(root).render(<ThemeProvider theme={createTheme()}><CssBaseline />
  <Container maxWidth="md" sx={{ py: 2 }}><AppAccountLoginPane open /></Container>
</ThemeProvider>)
