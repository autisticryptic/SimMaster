import { useCallback, useEffect, useMemo, useState } from 'react'
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
  TextField,
  Typography,
} from '@mui/material'
import { Refresh, Save, TravelExplore } from '@mui/icons-material'
import {
  api,
  type ApnContext,
  type BandLockRequest,
  type BandLockStatus,
  type CellsResponse,
  type OperatorListResponse,
  type RadioMode,
  type SetApnRequest,
} from '../../api/current'

export type CellularSettingsSection = 'cells' | 'apn' | 'operator'

type Props = {
  section: CellularSettingsSection
  lineLabel: string
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

function CellsSettings({ lineLabel }: { lineLabel: string }) {
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
        api.getCellsInfo(), api.getBandLockStatus(), api.getRadioMode(),
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
  }, [])

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
        <Button size="small" color="warning" onClick={() => void run('unlock-cell', () => api.unlockAllCells(), '已解除小区锁定')} disabled={busy !== null}>解除小区锁定</Button>
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
                  <TableCell align="right"><Button size="small" disabled={!canLock || busy !== null} onClick={() => void run(`cell-${index}`, () => api.setCellLock({ rat: cell.tech.toLowerCase().includes('nr') ? 16 : 12, enable: true, arfcn: Number(arfcn), pci: Number(cell.pci) }), '小区锁定已提交')}>锁定</Button></TableCell>
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
          <FormControl size="small" sx={{ minWidth: 150 }}><InputLabel>射频模式</InputLabel><Select label="射频模式" value={radioMode} onChange={(event) => { const mode = event.target.value as RadioMode; void run('radio', () => api.setRadioMode(mode), '射频模式已更新').then(() => setRadioMode(mode)) }} disabled={busy !== null}><MenuItem value="auto">自动</MenuItem><MenuItem value="lte">仅 LTE</MenuItem><MenuItem value="nr">仅 5G NR</MenuItem></Select></FormControl>
          <Button size="small" color="warning" onClick={() => void run('unlock-bands', () => api.setBandLock(EMPTY_BANDS), '已解除频段限制')} disabled={busy !== null}>使用全部频段</Button>
          <Button size="small" variant="contained" onClick={() => void run('bands', () => api.setBandLock(selection), '频段配置已应用')} disabled={busy !== null}>应用频段</Button>
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

function ApnSettings({ lineLabel }: { lineLabel: string }) {
  const [contexts, setContexts] = useState<ApnContext[]>([])
  const [selectedPath, setSelectedPath] = useState('')
  const [form, setForm] = useState<SetApnRequest>({ context_path: '', apn: '', protocol: 'dual', username: '', password: '', auth_method: 'chap' })
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const selected = useMemo(() => contexts.find((item) => item.path === selectedPath), [contexts, selectedPath])

  const choose = useCallback((context: ApnContext) => {
    setSelectedPath(context.path)
    setForm({ context_path: context.path, apn: context.apn, protocol: context.protocol || 'dual', username: context.username, password: context.password, auth_method: context.auth_method || 'chap' })
  }, [])
  const load = useCallback(async () => {
    setLoading(true)
    try {
      const response = await api.getApnList()
      const items = response.data?.contexts ?? []
      setContexts(items)
      const next = items.find((item) => item.active) ?? items[0]
      if (next) choose(next)
    } catch (err) { setError(errorText(err)) } finally { setLoading(false) }
  }, [choose])
  useEffect(() => { void load() }, [load])

  const save = async () => {
    setSaving(true); setError(null)
    try { await api.setApn({ ...form, context_path: selectedPath || form.context_path }); await load() } catch (err) { setError(errorText(err)) } finally { setSaving(false) }
  }

  return (
    <Stack spacing={2}>
      <Alert severity="info">为 {lineLabel} 选择数据承载并设置 APN。保存活动承载时可能触发数据连接重建。</Alert>
      {error && <Alert severity="error">{error}</Alert>}
      {loading ? <Box display="flex" justifyContent="center" py={8}><CircularProgress /></Box> : contexts.length === 0 ? <Alert severity="warning">未发现可配置的 APN 承载</Alert> : <>
        <FormControl fullWidth><InputLabel>APN 配置槽位</InputLabel><Select label="APN 配置槽位" value={selectedPath} onChange={(event) => { const next = contexts.find((item) => item.path === event.target.value); if (next) choose(next) }}>{contexts.map((item) => <MenuItem key={item.path} value={item.path}>{item.name} · {item.active ? '活动' : '未活动'} · {item.apn || '未设置 APN'}</MenuItem>)}</Select></FormControl>
        <TextField label="APN" value={form.apn ?? ''} onChange={(event) => setForm((current) => ({ ...current, apn: event.target.value }))} />
        <Box display="grid" gridTemplateColumns={{ xs: '1fr', sm: 'repeat(2, minmax(0, 1fr))' }} gap={2}>
          <FormControl><InputLabel>IP 协议</InputLabel><Select label="IP 协议" value={form.protocol ?? 'dual'} onChange={(event) => setForm((current) => ({ ...current, protocol: event.target.value }))}><MenuItem value="dual">IPv4 + IPv6</MenuItem><MenuItem value="ip">IPv4</MenuItem><MenuItem value="ipv6">IPv6</MenuItem></Select></FormControl>
          <FormControl><InputLabel>认证方式</InputLabel><Select label="认证方式" value={form.auth_method ?? 'chap'} onChange={(event) => setForm((current) => ({ ...current, auth_method: event.target.value }))}><MenuItem value="none">无认证</MenuItem><MenuItem value="pap">PAP</MenuItem><MenuItem value="chap">CHAP</MenuItem></Select></FormControl>
          <TextField label="用户名" value={form.username ?? ''} onChange={(event) => setForm((current) => ({ ...current, username: event.target.value }))} />
          <TextField label="密码" type="password" value={form.password ?? ''} onChange={(event) => setForm((current) => ({ ...current, password: event.target.value }))} />
        </Box>
        <Box display="flex" justifyContent="flex-end"><Button variant="contained" startIcon={saving ? <CircularProgress size={16} /> : <Save />} onClick={() => void save()} disabled={saving || !selected}>保存 APN</Button></Box>
      </>}
    </Stack>
  )
}

function OperatorSettings({ lineLabel }: { lineLabel: string }) {
  const [data, setData] = useState<OperatorListResponse | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const load = useCallback(async () => { try { const response = await api.getOperators(); setData(response.data ?? null) } catch (err) { setError(errorText(err)) } }, [])
  useEffect(() => { void load() }, [load])
  const run = async (key: string, action: () => Promise<unknown>) => { setBusy(key); setError(null); try { await action(); await load() } catch (err) { setError(errorText(err)) } finally { setBusy(null) } }
  return (
    <Stack spacing={2}>
      <Alert severity="warning">运营商扫描和重新注册会中断 {lineLabel} 当前驻网，通话或短信期间请勿操作。</Alert>
      {error && <Alert severity="error">{error}</Alert>}
      <Box display="flex" gap={1} justifyContent="flex-end" flexWrap="wrap"><Button startIcon={<Refresh />} onClick={() => void load()} disabled={busy !== null}>刷新</Button><Button startIcon={<TravelExplore />} onClick={() => void run('scan', async () => { const response = await api.scanOperators(); setData(response.data ?? null) })} disabled={busy !== null}>{busy === 'scan' ? '扫描中…' : '扫描运营商'}</Button><Button variant="contained" onClick={() => void run('auto', () => api.registerOperatorAuto())} disabled={busy !== null}>自动注册</Button></Box>
      <TableContainer><Table size="small"><TableHead><TableRow><TableCell>运营商</TableCell><TableCell>PLMN</TableCell><TableCell>制式</TableCell><TableCell>状态</TableCell><TableCell align="right">操作</TableCell></TableRow></TableHead><TableBody>{(data?.operators ?? []).map((item) => { const plmn = `${item.mcc}${item.mnc}`; return <TableRow key={`${item.path}-${plmn}`}><TableCell>{item.name || '未知运营商'}</TableCell><TableCell sx={{ fontFamily: 'monospace' }}>{plmn || '-'}</TableCell><TableCell>{item.technologies.join(' / ') || '-'}</TableCell><TableCell><Chip size="small" label={item.status === 'current' ? '当前网络' : item.status} color={item.status === 'current' ? 'success' : 'default'} variant="outlined" /></TableCell><TableCell align="right"><Button size="small" disabled={!plmn || item.status === 'current' || busy !== null} onClick={() => void run(plmn, () => api.registerOperatorManual(plmn))}>注册</Button></TableCell></TableRow>})}{(data?.operators.length ?? 0) === 0 && <TableRow><TableCell colSpan={5} align="center">暂无运营商数据</TableCell></TableRow>}</TableBody></Table></TableContainer>
    </Stack>
  )
}

export default function LineCellularSettings({ section, lineLabel }: Props) {
  if (section === 'cells') return <CellsSettings lineLabel={lineLabel} />
  if (section === 'apn') return <ApnSettings lineLabel={lineLabel} />
  return <OperatorSettings lineLabel={lineLabel} />
}
