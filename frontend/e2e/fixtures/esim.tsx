import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { CssBaseline, ThemeProvider, createTheme } from '@mui/material'
import EsimManagerPage from '../../src/pages/EsimManager'

export function Fixture() {
  const [line, setLine] = useState('line-a')
  return <ThemeProvider theme={createTheme()}><CssBaseline />
    <div style={{ padding: 16 }}>
      <button onClick={() => setLine('line-a')}>测试线路 A</button>
      <button onClick={() => setLine('line-b')}>测试线路 B</button>
      <EsimManagerPage lineId={line} />
    </div>
  </ThemeProvider>
}
const root = document.getElementById('root')
if (root) createRoot(root).render(<Fixture />)
