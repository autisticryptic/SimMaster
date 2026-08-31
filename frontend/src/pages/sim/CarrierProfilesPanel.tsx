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
  type CarrierCatalogAsset,
  type CarrierCatalogStatusResponse,
  type CarrierProfileSummary,
  type CarrierProfileRecord,
  type ProfileOrigin,
} from '../../api/current'
import CarrierProfileEditor from './CarrierProfileEditor'
import GithubDownloadProxyControl from '../../components/GithubDownloadProxyControl'

const originLabels: Record<ProfileOrigin, string> = {
  carrier_catalog: '运营商数据库',
  database: '用户自定义',
  builtin: '内置',
  derived: '推导',
}

type SearchMode = 'plmn' | 'mcc' | 'name'

const searchModeLabels: Record<SearchMode, string> = {
  plmn: 'PLMN',
  mcc: 'MCC',
  name: '运营商名称',
}

/** MB, one decimal. Sizes come from the release, so they are exact byte counts. */
function formatAssetSize(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export default function CarrierProfilesPanel() {
  const [allProfiles, setAllProfiles] = useState<CarrierProfileSummary[]>([])
  const [profiles, setProfiles] = useState<CarrierProfileSummary[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [busyKey, setBusyKey] = useState<string | null>(null)
  const [catalogStatus, setCatalogStatus] = useState<CarrierCatalogStatusResponse | null>(null)
  const [catalogAssetUrl, setCatalogAssetUrl] = useState('')
  const [catalogDialogOpen, setCatalogDialogOpen] = useState(false)
  // Databases are read from the release when the dialog opens, not compiled in:
  // the upstream set changes, and a pinned filename silently 404s after a rename.
  const [catalogAssets, setCatalogAssets] = useState<CarrierCatalogAsset[]>([])
  const [catalogAssetsLoading, setCatalogAssetsLoading] = useState(false)
  const [catalogAssetsError, setCatalogAssetsError] = useState<string | null>(null)
  const [catalogReleaseTag, setCatalogReleaseTag] = useState('')

  const [editing, setEditing] = useState<CarrierProfileRecord | null>(null)
  const [editorOpen, setEditorOpen] = useState(false)
  const [editorReadOnly, setEditorReadOnly] = useState(true)
  const [profileIdLocked, setProfileIdLocked] = useState(true)
  const [deleteTarget, setDeleteTarget] = useState<CarrierProfileSummary | null>(null)

  const [searchMode, setSearchMode] = useState<SearchMode>('plmn')
  const [searchValue, setSearchValue] = useState('')
  const [activeSearch, setActiveSearch] = useState<string | null>(null)
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
      const loadedProfiles = profileResponse.data ?? []
      setAllProfiles(loadedProfiles)
      setProfiles(loadedProfiles)
      setCatalogStatus(statusResponse.data ?? null)
      setActiveSearch(null)
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

  /** Read the release's databases. Called when the dialog opens so the list is
   *  current, and available for a manual retry when GitHub is unreachable. */
  const loadCatalogAssets = async () => {
    setCatalogAssetsLoading(true)
    setCatalogAssetsError(null)
    try {
      const response = await api.getCarrierCatalogAssets()
      const listing = response.data
      const assets = listing?.assets ?? []
      setCatalogAssets(assets)
      setCatalogReleaseTag(listing?.release_tag ?? '')
      if (assets.length === 0) {
        setCatalogAssetsError(listing?.message || '未能从 Release 读取到数据库文件')
        setCatalogAssetUrl('')
      } else {
        // Keep the current pick only if the release still offers it; otherwise
        // fall back to the first (largest) database.
        setCatalogAssetUrl((current) =>
          assets.some((asset) => asset.download_url === current)
            ? current
            : assets[0].download_url)
      }
    } catch (err) {
      setCatalogAssets([])
      setCatalogAssetUrl('')
      setCatalogAssetsError(err instanceof Error ? err.message : String(err))
    } finally {
      setCatalogAssetsLoading(false)
    }
  }

  const openCatalogDialog = () => {
    setCatalogDialogOpen(true)
    void loadCatalogAssets()
  }

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

  const openStoredProfile = async (
    profile: CarrierProfileSummary,
    options: { readOnly: boolean; lockProfileId: boolean },
  ) => {
    if (profile.origin !== 'database' && profile.origin !== 'carrier_catalog') return
    const key = `open:${profile.origin}:${profile.profile_id}`
    setBusyKey(key)
    setError(null)
    try {
      const response = await api.getVowifiCarrierProfile(profile.origin, profile.profile_id)
      if (!response.data) throw new Error('后端未返回 Profile 详情')
      openEditor(response.data.record, options)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusyKey(null)
    }
  }

  const searchProfiles = () => {
    const value = searchValue.trim()
    if (searchMode === 'plmn' && !/^\d{5,6}$/.test(value)) {
      setError('PLMN 必须是 5 或 6 位数字，例如 46001')
      return
    }
    if (searchMode === 'mcc' && !/^\d{3}$/.test(value)) {
      setError('MCC 必须是 3 位数字，例如 460')
      return
    }
    if (searchMode === 'name' && !value) {
      setError('请输入运营商名称')
      return
    }
    setError(null)
    setSuccess(null)
    const normalizedName = value.toLocaleLowerCase()
    const matches = allProfiles.filter((profile) => {
      if (searchMode === 'plmn') return profile.plmn === value
      if (searchMode === 'mcc') return profile.mcc === value
      return [profile.brand, profile.operator_legal_name, profile.profile_id, ...profile.aliases]
        .some((candidate) => candidate.toLocaleLowerCase().includes(normalizedName))
    })
    setProfiles(matches)
    setPage(0)
    setActiveSearch(`${searchModeLabels[searchMode]}：${value}`)
  }

  const showAllProfiles = () => {
    setProfiles(allProfiles)
    setActiveSearch(null)
    setPage(0)
  }

  const deleteCustomProfile = async () => {
    if (!deleteTarget) return
    setBusyKey(`delete:${deleteTarget.profile_id}`)
    setError(null)
    try {
      await api.deleteVowifiCarrierProfile(deleteTarget.profile_id)
      setSuccess(`已删除自定义 Profile：${deleteTarget.brand || deleteTarget.profile_id}`)
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
              onClick={openCatalogDialog}
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
              select
              size="small"
              label="查找方式"
              value={searchMode}
              onChange={(event) => {
                setSearchMode(event.target.value as SearchMode)
                setSearchValue('')
              }}
              sx={{ minWidth: 140 }}
            >
              <MenuItem value="plmn">PLMN</MenuItem>
              <MenuItem value="mcc">MCC（国家）</MenuItem>
              <MenuItem value="name">运营商名称</MenuItem>
            </TextField>
            <TextField
              size="small"
              label={`按${searchModeLabels[searchMode]}查找`}
              value={searchValue}
              placeholder={searchMode === 'plmn' ? '46001' : searchMode === 'mcc' ? '460' : 'China Mobile'}
              helperText="仅查找用户自定义和已下载数据库，不使用推断配置"
              onChange={(event) => setSearchValue(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === 'Enter') searchProfiles()
              }}
              sx={{ minWidth: { xs: '100%', sm: 320 } }}
            />
            <Button
              startIcon={<Search />}
              onClick={searchProfiles}
              disabled={busyKey !== null || loading}
              sx={{ mt: 0.5 }}
            >
              查找
            </Button>
            {activeSearch && (
              <Button onClick={showAllProfiles} disabled={busyKey !== null || loading} sx={{ mt: 0.5 }}>
                显示全部
              </Button>
            )}
          </Stack>

          {activeSearch && (
            <Typography variant="caption" color="text.secondary">
              当前结果：{activeSearch}，共 {profiles.length} 条
            </Typography>
          )}

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
                      <TableRow key={`${profile.origin}:${profile.profile_id}`} hover>
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
                            {profile.brand || profile.profile_id}
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
                                  onClick={() => void openStoredProfile(profile, { readOnly: false, lockProfileId: true })}
                                  disabled={busyKey !== null}
                                >
                                  {busyKey === `open:${profile.origin}:${profile.profile_id}`
                                    ? <CircularProgress size={16} />
                                    : <Edit fontSize="small" />}
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
                                  onClick={() => void openStoredProfile(profile, { readOnly: true, lockProfileId: true })}
                                  disabled={busyKey !== null}
                                >
                                  {busyKey === `open:${profile.origin}:${profile.profile_id}`
                                    ? <CircularProgress size={16} />
                                    : <Visibility fontSize="small" />}
                                </IconButton>
                              </Tooltip>
                              <Tooltip title="复制为自定义 Profile">
                                <IconButton
                                  size="small"
                                  color="primary"
                                  onClick={() => void openStoredProfile(profile, { readOnly: false, lockProfileId: true })}
                                  disabled={busyKey !== null}
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
                          {activeSearch ? '数据库中没有匹配的 Profile' : '尚无可用 Profile'}
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
              disabled={catalogAssetsLoading || catalogAssets.length === 0}
              helperText={catalogAssetsLoading
                ? '正在读取 Release 中的数据库…'
                : catalogAssetsError
                  ? catalogAssetsError
                  : catalogReleaseTag
                    ? `来自 Release ${catalogReleaseTag}，共 ${catalogAssets.length} 个数据库`
                    : ' '}
              error={Boolean(catalogAssetsError)}
              onChange={(event) => setCatalogAssetUrl(event.target.value)}
            >
              {catalogAssets.map((asset) => (
                <MenuItem key={asset.download_url} value={asset.download_url}>
                  {asset.label} · {formatAssetSize(asset.size)}
                </MenuItem>
              ))}
            </TextField>
            {catalogAssetsError && (
              <Button size="small" onClick={() => void loadCatalogAssets()} disabled={catalogAssetsLoading}>
                重新读取数据库列表
              </Button>
            )}
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
            disabled={busyKey !== null || catalogAssetsLoading || !catalogAssetUrl}
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
