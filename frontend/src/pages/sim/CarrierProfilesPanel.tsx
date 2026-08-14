import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  Alert,
  Avatar,
  Box,
  Button,
  Card,
  CardContent,
  CardHeader,
  Chip,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  IconButton,
  MenuItem,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TablePagination,
  TableRow,
  TextField,
  Tooltip,
  Typography,
} from '@mui/material'
import {
  Business,
  ContentCopy,
  Delete,
  Download,
  Edit,
  Refresh,
  Search,
  Visibility,
} from '@mui/icons-material'
import {
  api,
  type CarrierCatalogStatusResponse,
  type CarrierProfileRecord,
  type ProfileOrigin,
  type StoredCarrierProfile,
} from '../../api/current'
import CarrierProfileEditor from './CarrierProfileEditor'
import GithubDownloadProxyControl from '../../components/GithubDownloadProxyControl'

const originLabels: Record<ProfileOrigin, string> = {
  carrier_catalog: '运营商数据库',
  database: '用户自定义',
  builtin: '内置',
  derived: '推导',
}

const CATALOG_ASSETS = [
  {
    label: 'Pixel / Android（推荐）',
    url: 'https://github.com/autisticryptic/carrier_Bundles/releases/download/v0.3.0-catalog-v7/carrier-bundles-pixel-mustang.sqlite3',
  },
  {
    label: 'iPhone 16 Pro Max',
    url: 'https://github.com/autisticryptic/carrier_Bundles/releases/download/v0.3.0-catalog-v7/carrier-bundles-iphone16promax-26.6.sqlite3',
  },
  {
    label: 'iOS IPCC 汇总',
    url: 'https://github.com/autisticryptic/carrier_Bundles/releases/download/v0.3.0-catalog-v7/carrier-bundles-ios-ipcc.sqlite3',
  },
]

export default function CarrierProfilesPanel() {
  const [profiles, setProfiles] = useState<StoredCarrierProfile[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [busyKey, setBusyKey] = useState<string | null>(null)
  const [catalogStatus, setCatalogStatus] = useState<CarrierCatalogStatusResponse | null>(null)
  const [catalogAssetUrl, setCatalogAssetUrl] = useState(CATALOG_ASSETS[0].url)
  const [catalogDialogOpen, setCatalogDialogOpen] = useState(false)

  const [editing, setEditing] = useState<CarrierProfileRecord | null>(null)
  const [editorOpen, setEditorOpen] = useState(false)
  const [editorReadOnly, setEditorReadOnly] = useState(true)
  const [profileIdLocked, setProfileIdLocked] = useState(true)
  const [deleteTarget, setDeleteTarget] = useState<StoredCarrierProfile | null>(null)

  const [lookupPlmn, setLookupPlmn] = useState('')
  const [page, setPage] = useState(0)
  const [rowsPerPage, setRowsPerPage] = useState(20)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [profileResponse, statusResponse] = await Promise.all([
        api.listVowifiCarrierProfiles(),
        api.getCarrierCatalogStatus(),
      ])
      setProfiles(profileResponse.data ?? [])
      setCatalogStatus(statusResponse.data ?? null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  const pagedProfiles = useMemo(
    () => profiles.slice(page * rowsPerPage, page * rowsPerPage + rowsPerPage),
    [page, profiles, rowsPerPage],
  )

  useEffect(() => {
    if (page > 0 && page * rowsPerPage >= profiles.length) setPage(0)
  }, [page, profiles.length, rowsPerPage])

  const installCatalog = async () => {
    setBusyKey('catalog')
    setError(null)
    setSuccess(null)
    try {
      const response = await api.installCarrierCatalog({ asset_url: catalogAssetUrl })
      const installed = response.data
      setSuccess(installed
        ? `已覆盖并启用 ${installed.release_id}：VoLTE ${installed.volte_profiles} 条，VoWiFi ${installed.vowifi_profiles} 条`
        : '运营商数据库安装完成')
      setCatalogDialogOpen(false)
      await load()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusyKey(null)
    }
  }

  useEffect(() => {
    void load()
  }, [load])

  const openEditor = (
    record: CarrierProfileRecord,
    options: { readOnly: boolean; lockProfileId: boolean },
  ) => {
    setEditing(structuredClone(record))
    setEditorReadOnly(options.readOnly)
    setProfileIdLocked(options.lockProfileId)
    setEditorOpen(true)
  }

  const openForPlmn = async (plmn: string) => {
    const digits = plmn.replace(/\D/g, '')
    if (digits.length < 5 || digits.length > 6) {
      setError('PLMN 必须是 5 或 6 位数字，例如 46001')
      return
    }
    setBusyKey('lookup')
    setError(null)
    try {
      const response = await api.resolveVowifiCarrierProfile(digits)
      const resolved = response.data
      if (!resolved) throw new Error('未能解析该 PLMN')
      const readOnly = resolved.origin !== 'database'
      openEditor(resolved.record, { readOnly, lockProfileId: true })
      setSuccess(`${digits} 当前来自「${originLabels[resolved.origin]}」`)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusyKey(null)
    }
  }

  const deleteCustomProfile = async () => {
    if (!deleteTarget) return
    setBusyKey(`delete:${deleteTarget.profile_id}`)
    setError(null)
    try {
      await api.deleteVowifiCarrierProfile(deleteTarget.profile_id)
      setSuccess(`已删除自定义 Profile：${deleteTarget.record.meta.brand || deleteTarget.profile_id}`)
      setDeleteTarget(null)
      await load()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusyKey(null)
    }
  }

  return (
    <Card variant="outlined">
      <CardHeader
        title="运营商 IMS Profile"
        subheader="VoLTE、VoWiFi、ViLTE、SMS over IMS 与 UT/XCAP 的运营商接入配置"
        action={
          <Stack direction="row" spacing={1}>
            <Button
              size="small"
              variant="outlined"
              startIcon={<Download />}
              onClick={() => setCatalogDialogOpen(true)}
            >
              数据库下载
            </Button>
            <Button size="small" startIcon={<Refresh />} onClick={() => void load()} disabled={loading}>
              刷新
            </Button>
          </Stack>
        }
      />
      <CardContent>
        <Stack spacing={2}>
          {!catalogStatus?.usable && (
            <Alert severity="warning">
              尚无可用的 carrier_Bundles schema v7 数据库，请通过右上角“数据库下载”安装。
            </Alert>
          )}

          <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1.5} alignItems="flex-start">
            <TextField
              size="small"
              label="按 PLMN 查找"
              value={lookupPlmn}
              placeholder="46001"
              helperText="填入 MCC+MNC，打开当前生效的配置"
              onChange={(event) => setLookupPlmn(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') void openForPlmn(lookupPlmn)
              }}
            />
            <Button
              startIcon={busyKey === 'lookup' ? <CircularProgress size={16} /> : <Search />}
              onClick={() => void openForPlmn(lookupPlmn)}
              disabled={busyKey !== null}
              sx={{ mt: 0.5 }}
            >
              打开
            </Button>
          </Stack>

          {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
          {success && <Alert severity="success" onClose={() => setSuccess(null)}>{success}</Alert>}

          {loading && profiles.length === 0 ? (
            <Box display="flex" justifyContent="center" py={6}>
              <CircularProgress />
            </Box>
          ) : (
            <>
              <TableContainer>
                <Table size="small">
                  <TableHead>
                    <TableRow>
                      <TableCell padding="checkbox">图标</TableCell>
                      <TableCell>运营商</TableCell>
                      <TableCell>PLMN</TableCell>
                      <TableCell>IMS 能力</TableCell>
                      <TableCell>来源</TableCell>
                      <TableCell>更新时间</TableCell>
                      <TableCell align="right">操作</TableCell>
                    </TableRow>
                  </TableHead>
                  <TableBody>
                    {pagedProfiles.map((profile) => (
                      <TableRow key={profile.profile_id} hover>
                        <TableCell padding="checkbox">
                          <Avatar
                            variant="rounded"
                            src={`/api/vowifi/carrier-profiles/${encodeURIComponent(profile.profile_id)}/icon`}
                            alt=""
                            sx={{ width: 32, height: 32, bgcolor: 'action.hover', color: 'text.secondary' }}
                          >
                            <Business fontSize="small" />
                          </Avatar>
                        </TableCell>
                        <TableCell>
                          <Typography variant="body2" fontWeight={600}>
                            {profile.record.meta.brand || profile.profile_id}
                          </Typography>
                          <Typography variant="caption" color="text.secondary">
                            {profile.profile_id}
                          </Typography>
                        </TableCell>
                        <TableCell sx={{ fontFamily: 'monospace' }}>{profile.plmn}</TableCell>
                        <TableCell>
                          <Stack direction="row" spacing={0.5} flexWrap="wrap" useFlexGap>
                            {profile.volte_ready && <Chip size="small" label="VoLTE" color="primary" variant="outlined" />}
                            {profile.vowifi_ready && <Chip size="small" label="VoWiFi" color="success" variant="outlined" />}
                            {profile.vilte_enabled && <Chip size="small" label="ViLTE" color="secondary" variant="outlined" />}
                            {profile.smsoip_enabled && <Chip size="small" label="SMS IMS" variant="outlined" />}
                            {profile.ut_xcap_enabled && <Chip size="small" label="UT/XCAP" variant="outlined" />}
                          </Stack>
                        </TableCell>
                        <TableCell>
                          <Chip
                            size="small"
                            variant="outlined"
                            color={profile.origin === 'database' ? 'info' : 'default'}
                            label={originLabels[profile.origin]}
                          />
                        </TableCell>
                        <TableCell>
                          <Typography variant="caption" color="text.secondary">
                            {profile.updated_at.slice(0, 19).replace('T', ' ')}
                          </Typography>
                        </TableCell>
                        <TableCell align="right" sx={{ whiteSpace: 'nowrap' }}>
                          {profile.origin === 'database' ? (
                            <>
                              <Tooltip title="编辑自定义 Profile">
                                <IconButton
                                  size="small"
                                  onClick={() => openEditor(profile.record, { readOnly: false, lockProfileId: true })}
                                >
                                  <Edit fontSize="small" />
                                </IconButton>
                              </Tooltip>
                              <Tooltip title="删除自定义 Profile">
                                <IconButton size="small" color="error" onClick={() => setDeleteTarget(profile)}>
                                  <Delete fontSize="small" />
                                </IconButton>
                              </Tooltip>
                            </>
                          ) : (
                            <>
                              <Tooltip title="查看 Profile">
                                <IconButton
                                  size="small"
                                  onClick={() => openEditor(profile.record, { readOnly: true, lockProfileId: true })}
                                >
                                  <Visibility fontSize="small" />
                                </IconButton>
                              </Tooltip>
                              <Tooltip title="复制为自定义 Profile">
                                <IconButton
                                  size="small"
                                  color="primary"
                                  onClick={() => openEditor(profile.record, { readOnly: false, lockProfileId: true })}
                                >
                                  <ContentCopy fontSize="small" />
                                </IconButton>
                              </Tooltip>
                            </>
                          )}
                        </TableCell>
                      </TableRow>
                    ))}
                    {profiles.length === 0 && (
                      <TableRow>
                        <TableCell colSpan={7} align="center">
                          尚无可用 Profile
                        </TableCell>
                      </TableRow>
                    )}
                  </TableBody>
                </Table>
              </TableContainer>
              <TablePagination
                component="div"
                count={profiles.length}
                page={page}
                rowsPerPage={rowsPerPage}
                rowsPerPageOptions={[20, 40]}
                labelRowsPerPage="每页"
                labelDisplayedRows={({ from, to, count }) => `${from}-${to} / ${count}`}
                onPageChange={(_, nextPage) => setPage(nextPage)}
                onRowsPerPageChange={(event) => {
                  setRowsPerPage(Number(event.target.value))
                  setPage(0)
                }}
              />
            </>
          )}
        </Stack>
      </CardContent>

      <Dialog
        open={catalogDialogOpen}
        onClose={busyKey === 'catalog' ? undefined : () => setCatalogDialogOpen(false)}
        fullWidth
        maxWidth="sm"
      >
        <DialogTitle>运营商数据库下载</DialogTitle>
        <DialogContent dividers>
          <Stack spacing={2}>
            <Box>
              <Stack direction="row" alignItems="center" justifyContent="space-between" gap={1}>
                <Typography variant="subtitle2" fontWeight={700}>carrier_Bundles schema v7</Typography>
                {catalogStatus?.usable && <Chip size="small" color="success" label="数据库已就绪" />}
              </Stack>
              <Typography variant="caption" color="text.secondary">
                {catalogStatus?.usable
                  ? `${catalogStatus.release_id} · ${catalogStatus.generated_at} · VoLTE ${catalogStatus.volte_profiles} 条 · VoWiFi ${catalogStatus.vowifi_profiles} 条`
                  : `尚无可用运营商数据库。${catalogStatus?.message || ''}`}
              </Typography>
            </Box>
            <TextField
              select
              fullWidth
              size="small"
              label="数据库来源"
              value={catalogAssetUrl}
              onChange={(event) => setCatalogAssetUrl(event.target.value)}
            >
              {CATALOG_ASSETS.map((asset) => (
                <MenuItem key={asset.url} value={asset.url}>{asset.label}</MenuItem>
              ))}
            </TextField>
            <GithubDownloadProxyControl compact />
            {catalogStatus?.path && (
              <Typography variant="caption" color="text.secondary">
                安装位置：{catalogStatus.path}。安装时会验证数据库并原子覆盖当前 catalog。
              </Typography>
            )}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setCatalogDialogOpen(false)} disabled={busyKey === 'catalog'}>取消</Button>
          <Button
            variant="contained"
            startIcon={busyKey === 'catalog' ? <CircularProgress size={16} color="inherit" /> : <Download />}
            onClick={() => void installCatalog()}
            disabled={busyKey !== null}
          >
            {catalogStatus?.usable ? '下载并覆盖' : '下载并安装'}
          </Button>
        </DialogActions>
      </Dialog>

      <Dialog open={Boolean(deleteTarget)} onClose={() => setDeleteTarget(null)} maxWidth="xs" fullWidth>
        <DialogTitle>删除自定义 Profile</DialogTitle>
        <DialogContent dividers>
          <Typography variant="body2">
            删除后将恢复使用同一 PLMN 的 catalog 配置（如存在）。此操作不会修改下载的运营商数据库。
          </Typography>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setDeleteTarget(null)} disabled={busyKey?.startsWith('delete:')}>取消</Button>
          <Button
            color="error"
            variant="contained"
            startIcon={busyKey?.startsWith('delete:') ? <CircularProgress size={16} color="inherit" /> : <Delete />}
            onClick={() => void deleteCustomProfile()}
            disabled={busyKey !== null}
          >
            删除
          </Button>
        </DialogActions>
      </Dialog>

      <CarrierProfileEditor
        open={editorOpen}
        record={editing}
        readOnly={editorReadOnly}
        profileIdLocked={profileIdLocked}
        onClose={() => setEditorOpen(false)}
        onSaved={() => {
          setSuccess('自定义 Profile 已保存到 data.db，并已应用到运行时解析')
          void load()
        }}
      />
    </Card>
  )
}
