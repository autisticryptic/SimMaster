import { useCallback, useEffect, useState } from 'react'
import {
  Alert,
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
  Tooltip,
  Typography,
} from '@mui/material'
import { Add, Delete, Edit, Refresh, Search, UploadFile } from '@mui/icons-material'
import {
  api,
  type CarrierProfileImportFormat,
  type CarrierProfileImportResult,
  type CarrierProfileRecord,
  type ProfileOrigin,
  type StoredCarrierProfile,
} from '../../api/current'
import CarrierProfileEditor from './CarrierProfileEditor'

const originLabels: Record<ProfileOrigin, string> = {
  database: '数据库',
  builtin: '内置',
  derived: '推导',
}

const sourceLabels: Record<string, string> = {
  builtin: '内置种子',
  manual: '手动编辑',
  aosp_apns: 'AOSP APN',
  aosp_carrier_config: 'AOSP CarrierConfig',
  ipcc: 'IPCC',
}

const formatHints: Record<CarrierProfileImportFormat, string> = {
  aosp_apns:
    'AOSP apns-conf.xml。只读取 type 含 ims 的条目，文件里自带 MCC/MNC，可一次导入很多运营商。',
  aosp_carrier_config:
    'AOSP CarrierConfig XML。读取 VoWiFi 支持标志与 entitlement URL，需要手动指定 MCC/MNC。',
  ipcc:
    'Apple IPCC 里的 carrier.plist（XML 格式）。SimAdmin 不分发也不下载 IPCC，请自行提供文件内容。',
}

export default function CarrierProfilesPanel() {
  const [profiles, setProfiles] = useState<StoredCarrierProfile[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [success, setSuccess] = useState<string | null>(null)
  const [busyKey, setBusyKey] = useState<string | null>(null)

  const [editing, setEditing] = useState<CarrierProfileRecord | null>(null)
  const [editingE911Expected, setEditingE911Expected] = useState(false)
  const [editorOpen, setEditorOpen] = useState(false)

  const [lookupPlmn, setLookupPlmn] = useState('')
  const [importOpen, setImportOpen] = useState(false)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const response = await api.listVowifiCarrierProfiles()
      setProfiles(response.data ?? [])
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load()
  }, [load])

  /** Open the editor on the profile a PLMN currently resolves to. */
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
      setEditing(resolved.record)
      setEditingE911Expected(resolved.e911_expected)
      setEditorOpen(true)
      if (resolved.origin !== 'database') {
        setSuccess(
          `${digits} 当前来自「${originLabels[resolved.origin]}」，保存后会写入数据库并优先生效`,
        )
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusyKey(null)
    }
  }

  const remove = async (profileId: string) => {
    setBusyKey(`delete:${profileId}`)
    setError(null)
    try {
      await api.deleteVowifiCarrierProfile(profileId)
      setSuccess(`${profileId} 已删除，该 PLMN 将回退到内置或推导配置`)
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
        title="运营商 VoWiFi Profile"
        subheader="数据库优先，其次内置配置，最后按 3GPP 规则从 MCC/MNC 推导"
        action={
          <Stack direction="row" spacing={1}>
            <Button size="small" startIcon={<UploadFile />} onClick={() => setImportOpen(true)}>
              导入
            </Button>
            <Button size="small" startIcon={<Refresh />} onClick={() => void load()} disabled={loading}>
              刷新
            </Button>
          </Stack>
        }
      />
      <CardContent>
        <Stack spacing={2}>
          <Alert severity="info">
            没有对应记录的 SIM 卡也能用：ePDG 主机名和 IMS 域名会按 3GPP TS 23.003
            从 MCC/MNC 推导出来。只有当某家运营商的 REGISTER 细节和标准不一样时，才需要在这里加一条。
          </Alert>

          <Stack direction={{ xs: 'column', sm: 'row' }} spacing={1.5} alignItems="flex-start">
            <TextField
              size="small"
              label="按 PLMN 查找或新建"
              value={lookupPlmn}
              placeholder="46001"
              helperText="填入 MCC+MNC，会打开该运营商当前生效的配置"
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
            <Button
              startIcon={<Add />}
              onClick={() => void openForPlmn(lookupPlmn)}
              disabled={busyKey !== null || !lookupPlmn.trim()}
              sx={{ mt: 0.5 }}
            >
              基于推导值新建
            </Button>
          </Stack>

          {error && <Alert severity="error" onClose={() => setError(null)}>{error}</Alert>}
          {success && <Alert severity="success" onClose={() => setSuccess(null)}>{success}</Alert>}

          {loading && profiles.length === 0 ? (
            <Box display="flex" justifyContent="center" py={6}>
              <CircularProgress />
            </Box>
          ) : (
            <TableContainer>
              <Table size="small">
                <TableHead>
                  <TableRow>
                    <TableCell>运营商</TableCell>
                    <TableCell>PLMN</TableCell>
                    <TableCell>ePDG</TableCell>
                    <TableCell>来源</TableCell>
                    <TableCell>更新时间</TableCell>
                    <TableCell align="right">操作</TableCell>
                  </TableRow>
                </TableHead>
                <TableBody>
                  {profiles.map((profile) => (
                    <TableRow key={profile.profile_id} hover>
                      <TableCell>
                        <Typography variant="body2" fontWeight={600}>
                          {profile.record.meta.brand || profile.profile_id}
                        </Typography>
                        <Typography variant="caption" color="text.secondary">
                          {profile.profile_id}
                        </Typography>
                      </TableCell>
                      <TableCell sx={{ fontFamily: 'monospace' }}>{profile.plmn}</TableCell>
                      <TableCell
                        sx={{ fontFamily: 'monospace', fontSize: '0.75rem', wordBreak: 'break-all' }}
                      >
                        {profile.record.epdg.host}
                      </TableCell>
                      <TableCell>
                        <Chip
                          size="small"
                          variant="outlined"
                          label={sourceLabels[profile.source] ?? profile.source}
                        />
                      </TableCell>
                      <TableCell>
                        <Typography variant="caption" color="text.secondary">
                          {profile.updated_at.slice(0, 19).replace('T', ' ')}
                        </Typography>
                      </TableCell>
                      <TableCell align="right">
                        <Tooltip title="编辑">
                          <Button
                            size="small"
                            startIcon={<Edit />}
                            onClick={() => {
                              setEditing(profile.record)
                              setEditingE911Expected(/^31[0-6]/.test(profile.plmn))
                              setEditorOpen(true)
                            }}
                          >
                            编辑
                          </Button>
                        </Tooltip>
                        <Tooltip title="删除后该 PLMN 回退到内置或推导配置">
                          <Button
                            size="small"
                            color="warning"
                            startIcon={
                              busyKey === `delete:${profile.profile_id}` ? (
                                <CircularProgress size={14} />
                              ) : (
                                <Delete />
                              )
                            }
                            onClick={() => void remove(profile.profile_id)}
                            disabled={busyKey !== null}
                          >
                            删除
                          </Button>
                        </Tooltip>
                      </TableCell>
                    </TableRow>
                  ))}
                  {profiles.length === 0 && (
                    <TableRow>
                      <TableCell colSpan={6} align="center">
                        还没有任何数据库 profile，所有运营商都走内置或推导配置
                      </TableCell>
                    </TableRow>
                  )}
                </TableBody>
              </Table>
            </TableContainer>
          )}
        </Stack>
      </CardContent>

      <CarrierProfileEditor
        open={editorOpen}
        record={editing}
        e911Expected={editingE911Expected}
        onClose={() => setEditorOpen(false)}
        onSaved={() => {
          setSuccess('profile 已保存，下一次连接即生效')
          void load()
        }}
      />

      <ImportDialog
        open={importOpen}
        onClose={() => setImportOpen(false)}
        onImported={(message) => {
          setSuccess(message)
          void load()
        }}
      />
    </Card>
  )
}

function ImportDialog({
  open,
  onClose,
  onImported,
}: {
  open: boolean
  onClose: () => void
  onImported: (message: string) => void
}) {
  const [format, setFormat] = useState<CarrierProfileImportFormat>('aosp_apns')
  const [content, setContent] = useState('')
  const [mcc, setMcc] = useState('')
  const [mnc, setMnc] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [preview, setPreview] = useState<CarrierProfileImportResult | null>(null)

  const needsPlmn = format !== 'aosp_apns'

  const run = async (dryRun: boolean) => {
    setBusy(true)
    setError(null)
    try {
      const response = await api.importVowifiCarrierProfiles({
        format,
        content,
        mcc: needsPlmn ? mcc : undefined,
        mnc: needsPlmn ? mnc : undefined,
        dry_run: dryRun,
      })
      const result = response.data ?? null
      setPreview(result)
      if (!dryRun && result) {
        onImported(`已导入 ${result.imported.length} 条，跳过 ${result.skipped.length} 条`)
        onClose()
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog open={open} onClose={busy ? undefined : onClose} fullWidth maxWidth="md">
      <DialogTitle>从公开数据源导入</DialogTitle>
      <DialogContent dividers>
        <Stack spacing={2}>
          <Alert severity="info">
            导入值是叠加在 3GPP 推导默认值之上的，不会凭空编造 ePDG 主机名，也不会覆盖你手工验证过的字段。
            公开数据源提供不了 REGISTER 的运营商差异（sec-agree、Contact 形状等），那些只能靠实测。
          </Alert>

          <FormControl fullWidth>
            <InputLabel>数据源格式</InputLabel>
            <Select
              label="数据源格式"
              value={format}
              onChange={(event) => {
                setFormat(event.target.value as CarrierProfileImportFormat)
                setPreview(null)
              }}
            >
              <MenuItem value="aosp_apns">AOSP apns-conf.xml</MenuItem>
              <MenuItem value="aosp_carrier_config">AOSP CarrierConfig XML</MenuItem>
              <MenuItem value="ipcc">Apple IPCC carrier.plist</MenuItem>
            </Select>
          </FormControl>
          <Typography variant="caption" color="text.secondary">
            {formatHints[format]}
          </Typography>

          {needsPlmn && (
            <Stack direction="row" spacing={2}>
              <TextField
                label="MCC"
                value={mcc}
                onChange={(event) => setMcc(event.target.value.replace(/\D/g, '').slice(0, 3))}
              />
              <TextField
                label="MNC"
                value={mnc}
                onChange={(event) => setMnc(event.target.value.replace(/\D/g, '').slice(0, 3))}
              />
            </Stack>
          )}

          <TextField
            label="文件内容"
            value={content}
            multiline
            minRows={8}
            maxRows={18}
            placeholder="把 XML 内容整段粘贴到这里"
            onChange={(event) => {
              setContent(event.target.value)
              setPreview(null)
            }}
          />

          {preview && (
            <Alert severity={preview.imported.length > 0 ? 'success' : 'warning'}>
              <Typography variant="body2" fontWeight={600}>
                {preview.dry_run ? '预览结果' : '导入结果'}：可导入 {preview.imported.length} 条，
                跳过 {preview.skipped.length} 条
              </Typography>
              {preview.imported.slice(0, 12).map((item) => (
                <Typography key={item.profile_id} variant="caption" display="block">
                  {item.plmn} · {item.brand || '未命名'} · APN {item.ims_apn || '（未提供）'}
                  {item.e911_expected ? ' · 北美，建议配置紧急呼叫' : ''}
                </Typography>
              ))}
              {preview.imported.length > 12 && (
                <Typography variant="caption" display="block">
                  …以及另外 {preview.imported.length - 12} 条
                </Typography>
              )}
              {preview.skipped.slice(0, 5).map((item, index) => (
                <Typography key={`${item.plmn}-${index}`} variant="caption" display="block" color="text.secondary">
                  跳过 {item.plmn}：{item.reason}
                </Typography>
              ))}
            </Alert>
          )}

          {error && <Alert severity="error">{error}</Alert>}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={busy}>取消</Button>
        <Button onClick={() => void run(true)} disabled={busy || !content.trim()}>
          {busy ? '处理中…' : '预览'}
        </Button>
        <Button
          variant="contained"
          onClick={() => void run(false)}
          disabled={busy || !content.trim()}
        >
          确认导入
        </Button>
      </DialogActions>
    </Dialog>
  )
}
