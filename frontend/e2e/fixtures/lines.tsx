import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { CssBaseline, ThemeProvider, createTheme } from '@mui/material'
import ModemLinesPanel from '../../src/pages/sim/ModemLinesPanel'
import type { CellularImsLineControlResponse } from '../../src/api/contracts'

export function Fixture() {
  const [selected, setSelected] = useState<CellularImsLineControlResponse | null>(null)
  return <ThemeProvider theme={createTheme()}><CssBaseline />
    <div style={{ padding: 16 }}><div data-testid="selected-line">{selected?.modem.line_id}</div>
      <ModemLinesPanel workbench onSelectionChange={setSelected}
        basicInfoForLine={(line, controls) => <><div data-testid="original-basic-info">原基本信息 {line.modem.model}</div>{controls}</>}
        workbenchEsim={<div>原 eSIM 标签内容</div>}
        workbenchSms={<div>原短信标签内容</div>}
      />
    </div>
  </ThemeProvider>
}
const root = document.getElementById('root')
if (root) createRoot(root).render(<Fixture />)
