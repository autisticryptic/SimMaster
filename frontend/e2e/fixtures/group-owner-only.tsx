import { createRoot } from 'react-dom/client'
import { CssBaseline, Stack, ThemeProvider, createTheme } from '@mui/material'
import AppGroupCreatePane from '../../src/pages/sim/AppGroupCreatePane'
import AppGroupManagementPane from '../../src/pages/sim/AppGroupManagementPane'
const line = 'app-account-0123456789abcdef0123456789abcdef'
export default function Fixture() { return <ThemeProvider theme={createTheme()}><CssBaseline /><Stack spacing={2} sx={{ p: 2 }}><AppGroupCreatePane lineId={line} revision={1} hosted installed readInstalled ownerOnlyInstalled /><AppGroupManagementPane lineId={line} revision={1} hosted installed /></Stack></ThemeProvider> }
const root = document.getElementById('root'); if (!root) throw new Error('Missing owner-only fixture root')
createRoot(root).render(<Fixture />)
