import { useCallback, useEffect, useState } from 'react'
import {
  Alert,
  Box,
  Button,
  Chip,
  CircularProgress,
  FormControl,
  InputLabel,
  MenuItem,
  Select,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Typography,
} from '@mui/material'
import { Refresh, TravelExplore } from '@mui/icons-material'
import {
  api,
  type BandLockRequest,
  type BandLockStatus,
  type CellsResponse,
  type OperatorListResponse,
  type RadioMode,
} from '../../api/current'

export type CellularSettingsSection = 'cells' | 'operator'

type Props = {
  section: CellularSettingsSection
  lineLabel: string
  /**
   * The line these settings belong to. Without it every read and write lands on
   * whichever baseband the backend picks first, so a multi-SIM device would
   * silently configure the wrong card while the dialog claims otherwise.
   */
  lineId: string
}

type SectionProps = { lineLabel: string; lineId: string }

export function LineNetworkOverview({ lineId, lineLabel }: SectionProps) {
  const [operators, setOperators] = useState<OperatorListResponse | null>(null)
  const [cells, setCells] = useState<CellsResponse | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [operatorResponse, cellsResponse] = await Promise.all([
        api.getOperators(lineId),
        api.getCellsInfo(lineId),
      ])
      setOperators(operatorResponse.data ?? null)
      setCells(cellsResponse.data ?? null)
    } catch (err) {
      setError(errorText(err))
    } finally {
      setLoading(false)
    }
  }, [lineId])

  useEffect(() => { void load() }, [load])

  if (loading) return <Box display="flex" justifyContent="center" py={3}><CircularProgress size={22} /></Box>

  return (
    <Stack spacing={1.5}>
      {error && <Alert severity="warning" onClose={() => setError(null)}>{error}</Alert>}
      <Box display="flex" justifyContent="flex-end" alignItems="center" gap={1}>
        <Button size="small" startIcon={<Refresh />} onClick={() => void load()}>刷新</Button>
      </Box>
      <Box display="grid" gridTemplateColumns={{ xs: '1fr', sm: 'repeat(2, minmax(0, 1fr))' }} gap={1.25}>
        <Box><Typography variant="caption" color="text.secondary">服务小区</Typography><Typography variant="body2">{cells?.serving_cell ? `${cells.serving_cell.tech.toUpperCase()} · ${cells.serving_cell.cell_id}` : '未读取'}</Typography></Box>
        <Box><Typography variant="caption" color="text.secondary">发现网络</Typography><Typography variant="body2">{operators?.operators.length ?? 0} 个运营商 · {cells?.cells.length ?? 0} 个小区</Typography></Box>
      </Box>
      <TableContainer sx={{ maxHeight: 220 }}>
        <Table size="small" stickyHeader>
          <TableHead><TableRow><TableCell>小区</TableCell><TableCell>频段 / ARFCN</TableCell><TableCell>PCI</TableCell><TableCell>RSRP</TableCell></TableRow></TableHead>
          <TableBody>
            {(cells?.cells ?? []).slice(0, 8).map((cell, index) => <TableRow key={`${cell.tech}-${cell.arfcn}-${cell.pci}-${index}`}><TableCell><Chip size="small" label={cell.is_serving ? '服务' : cell.tech.toUpperCase()} color={cell.is_serving ? 'primary' : 'default'} /> </TableCell><TableCell>{cell.band || '-'} · {cell.arfcn || cell.earfcn || cell.nrarfcn || '-'}</TableCell><TableCell>{cell.pci || '-'}</TableCell><TableCell>{cell.rsrp || cell.ssb_rsrp || '-'}</TableCell></TableRow>)}
            {(cells?.cells.length ?? 0) === 0 && <TableRow><TableCell colSpan={4} align="center">暂无小区数据</TableCell></TableRow>}
          </TableBody>
        </Table>
      </TableContainer>
      <Box borderTop={1} borderColor="divider" pt={2}>
        <Typography variant="subtitle2" fontWeight={700} mb={1.5}>扫描与注册</Typography>
        <OperatorSettings lineId={lineId} lineLabel={lineLabel} />
      </Box>
      <Box borderTop={1} borderColor="divider" pt={2}>
        <Typography variant="subtitle2" fontWeight={700} mb={1.5}>小区与频段控制</Typography>
        <CellsSettings lineId={lineId} lineLabel={lineLabel} />
      </Box>
    </Stack>
  )
}

const EMPTY_BANDS: BandLockRequest = {
  lte_fdd_bands: [],
  lte_tdd_bands: [],
  nr_fdd_bands: [],
  nr_tdd_bands: [],
}

function errorText(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

function BandGroup({ label, supported, selected, onChange, prefix }: {
  label: string
  supported: number[]
  selected: number[]
  onChange: (next: number[]) => void
  prefix: string
}) {
  return (
    <Box>
      <Typography variant="caption" color="text.secondary">{label}</Typography>
      <Stack direction="row" spacing={0.75} mt={0.75} flexWrap="wrap" useFlexGap>
        {supported.length > 0 ? supported.map((band) => {
          const active = selected.includes(band)
          return <Chip key={band} clickable size="small" label={`${prefix}${band}`} color={active ? 'primary' : 'default'} variant={active ? 'filled' : 'outlined'} onClick={() => onChange(active ? selected.filter((item) => item !== band) : [...selected, band])} />
        }) : <Typography variant="body2" color="text.secondary">基带未报告支持频段</Typography>}
      </Stack>
    </Box>
  )
}

function CellsSettings({ lineLabel, lineId }: SectionProps) {
  const [cells, setCells] = useState<CellsResponse | null>(null)
  const [bands, setBands] = useState<BandLockStatus | null>(null)
  const [selection, setSelection] = useState<BandLockRequest>(EMPTY_BANDS)
  const [radioMode, setRadioMode] = useState<RadioMode>('auto')
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [message, setMessage] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [cellsResponse, bandResponse, modeResponse] = await Promise.all([
        api.getCellsInfo(lineId), api.getBandLockStatus(lineId), api.getRadioMode(lineId),
      ])
      setCells(cellsResponse.data ?? null)
      if (bandResponse.data) {
        setBands(bandResponse.data)
        setSelection({
          lte_fdd_bands: bandResponse.data.lte_fdd_bands,
          lte_tdd_bands: bandResponse.data.lte_tdd_bands,
          nr_fdd_bands: bandResponse.data.nr_fdd_bands,
          nr_tdd_bands: bandResponse.data.nr_tdd_bands,
        })
      }
      const mode = modeResponse.data?.mode
      if (mode === 'auto' || mode === 'lte' || mode === 'nr') setRadioMode(mode)
    } catch (err) {
      setError(errorText(err))
    } finally {
      setLoading(false)
    }
  }, [lineId])

  useEffect(() => { void load() }, [load])

  const run = async (key: string, action: () => Promise<unknown>, success: string) => {
    setBusy(key)
    setError(null)
    try {
      await action()
      setMessage(success)
      await load()
    } catch (err) {
      setError(errorText(err))
    } finally {
      setBusy(null)
    }
  }

  if (loading && !cells) return <Box display="flex" justifyContent="center" py={8}><CircularProgress /></Box>

  return (
    <Stack spacing={2.5}>
      <Alert severity="info">当前配置入口归属于 {lineLabel}。执行锁定或射频切换会短暂中断该基带驻网。</Alert>
      {error && <Alert severity="error">{error}</Alert>}
      {message && <Alert severity="success" onClose={() => setMessage(null)}>{message}</Alert>}
      <Box display="flex" alignItems="center" gap={1} flexWrap="wrap">
        <Typography variant="subtitle1" fontWeight={700} flexGrow={1}>小区与锁定</Typography>
        <Button size="small" startIcon={<Refresh />} onClick={() => void load()} disabled={loading || busy !== null}>刷新</Button>
        <Button size="small" color="warning" onClick={() => void run('unlock-cell', () => api.unlockAllCells(lineId), '已解除小区锁定')} disabled={busy !== null}>解除小区锁定</Button>
      </Box>
      <TableContainer sx={{ maxHeight: 300 }}>
        <Table size="small" stickyHeader>
          <TableHead><TableRow><TableCell>类型/频段</TableCell><TableCell>ARFCN</TableCell><TableCell>PCI</TableCell><TableCell>RSRP</TableCell><TableCell align="right">操作</TableCell></TableRow></TableHead>
          <TableBody>
            {(cells?.cells ?? []).map((cell, index) => {
              const arfcn = cell.arfcn || cell.earfcn || cell.nrarfcn || ''
              const canLock = Number.isFinite(Number(arfcn)) && Number.isFinite(Number(cell.pci)) && arfcn !== '' && cell.pci !== ''
              return (
                <TableRow key={`${cell.tech}-${arfcn}-${cell.pci}-${index}`}>
                  <TableCell><Stack direction="row" spacing={0.5}><Chip size="small" label={cell.tech.toUpperCase()} color={cell.is_serving ? 'primary' : 'default'} /><Typography variant="body2">{cell.band || '-'}</Typography></Stack></TableCell>
                  <TableCell sx={{ fontFamily: 'monospace' }}>{arfcn || '-'}</TableCell>
                  <TableCell sx={{ fontFamily: 'monospace' }}>{cell.pci || '-'}</TableCell>
                  <TableCell>{cell.rsrp || cell.ssb_rsrp || '-'}</TableCell>
                  <TableCell align="right"><Button size="small" disabled={!canLock || busy !== null} onClick={() => void run(`cell-${index}`, () => api.setCellLock(lineId, { rat: cell.tech.toLowerCase().includes('nr') ? 16 : 12, enable: true, arfcn: Number(arfcn), pci: Number(cell.pci) }), '小区锁定已提交')}>锁定</Button></TableCell>
                </TableRow>
              )
            })}
            {(cells?.cells.length ?? 0) === 0 && <TableRow><TableCell colSpan={5} align="center">暂无小区数据</TableCell></TableRow>}
          </TableBody>
        </Table>
      </TableContainer>

      <Box borderTop={1} borderColor="divider" pt={2.5}>
        <Box display="flex" alignItems="center" gap={1.5} flexWrap="wrap" mb={2}>
          <Typography variant="subtitle1" fontWeight={700} flexGrow={1}>射频与频段</Typography>
          <FormControl size="small" sx={{ minWidth: 150 }}><InputLabel>射频模式</InputLabel><Select label="射频模式" value={radioMode} onChange={(event) => { const mode = event.target.value as RadioMode; void run('radio', () => api.setRadioMode(mode, lineId), '射频模式已更新').then(() => setRadioMode(mode)) }} disabled={busy !== null}><MenuItem value="auto">自动</MenuItem><MenuItem value="lte">仅 LTE</MenuItem><MenuItem value="nr">仅 5G NR</MenuItem></Select></FormControl>
          <Button size="small" color="warning" onClick={() => void run('unlock-bands', () => api.setBandLock(EMPTY_BANDS, lineId), '已解除频段限制')} disabled={busy !== null}>使用全部频段</Button>
          <Button size="small" variant="contained" onClick={() => void run('bands', () => api.setBandLock(selection, lineId), '频段配置已应用')} disabled={busy !== null}>应用频段</Button>
        </Box>
        <Stack spacing={2}>
          <BandGroup label="LTE FDD" prefix="B" supported={bands?.supported_lte_fdd_bands ?? []} selected={selection.lte_fdd_bands} onChange={(next) => setSelection((current) => ({ ...current, lte_fdd_bands: next }))} />
          <BandGroup label="LTE TDD" prefix="B" supported={bands?.supported_lte_tdd_bands ?? []} selected={selection.lte_tdd_bands} onChange={(next) => setSelection((current) => ({ ...current, lte_tdd_bands: next }))} />
          <BandGroup label="5G NR FDD" prefix="n" supported={bands?.supported_nr_fdd_bands ?? []} selected={selection.nr_fdd_bands} onChange={(next) => setSelection((current) => ({ ...current, nr_fdd_bands: next }))} />
          <BandGroup label="5G NR TDD" prefix="n" supported={bands?.supported_nr_tdd_bands ?? []} selected={selection.nr_tdd_bands} onChange={(next) => setSelection((current) => ({ ...current, nr_tdd_bands: next }))} />
        </Stack>
      </Box>
    </Stack>
  )
}

function OperatorSettings({ lineLabel, lineId }: SectionProps) {
  const [data, setData] = useState<OperatorListResponse | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = useCallback(async () => { try { const response = await api.getOperators(lineId); setData(response.data ?? null) } catch (err) { setError(errorText(err)) } }, [lineId])
  useEffect(() => { void load() }, [load])
  const run = async (key: string, action: () => Promise<unknown>) => { setBusy(key); setError(null); try { await action(); await load() } catch (err) { setError(errorText(err)) } finally { setBusy(null) } }
  return (
    <Stack spacing={2}>
      <Alert severity="warning">运营商扫描和重新注册会中断 {lineLabel} 当前驻网，通话或短信期间请勿操作。</Alert>
      {error && <Alert severity="error">{error}</Alert>}
      <Box display="flex" gap={1} justifyContent="flex-end" flexWrap="wrap"><Button startIcon={<Refresh />} onClick={() => void load()} disabled={busy !== null}>刷新</Button><Button startIcon={<TravelExplore />} onClick={() => void run('scan', async () => { const response = await api.scanOperators(lineId); setData(response.data ?? null) })} disabled={busy !== null}>{busy === 'scan' ? '扫描中…' : '扫描运营商'}</Button><Button variant="contained" onClick={() => void run('auto', () => api.registerOperatorAuto(lineId))} disabled={busy !== null}>自动注册</Button></Box>
      <TableContainer><Table size="small"><TableHead><TableRow><TableCell>运营商</TableCell><TableCell>PLMN</TableCell><TableCell>制式</TableCell><TableCell>状态</TableCell><TableCell align="right">操作</TableCell></TableRow></TableHead><TableBody>{(data?.operators ?? []).map((item) => { const plmn = `${item.mcc}${item.mnc}`; return <TableRow key={`${item.path}-${plmn}`}><TableCell>{item.name || '未知运营商'}</TableCell><TableCell sx={{ fontFamily: 'monospace' }}>{plmn || '-'}</TableCell><TableCell>{item.technologies.join(' / ') || '-'}</TableCell><TableCell><Chip size="small" label={item.status === 'current' ? '当前网络' : item.status} color={item.status === 'current' ? 'success' : 'default'} variant="outlined" /></TableCell><TableCell align="right"><Button size="small" disabled={!plmn || item.status === 'current' || busy !== null} onClick={() => void run(plmn, () => api.registerOperatorManual(plmn, lineId))}>注册</Button></TableCell></TableRow>})}{(data?.operators.length ?? 0) === 0 && <TableRow><TableCell colSpan={5} align="center">暂无运营商数据</TableCell></TableRow>}</TableBody></Table></TableContainer>
    </Stack>
  )
}

export default function LineCellularSettings({ section, lineLabel, lineId }: Props) {
  if (section === 'cells') return <CellsSettings lineLabel={lineLabel} lineId={lineId} />
  return <OperatorSettings lineLabel={lineLabel} lineId={lineId} />
}
