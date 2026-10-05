import { useRef, useState } from 'react'
import { Alert, Box, Button, Chip, CircularProgress, Dialog, DialogActions, DialogContent, DialogContentText, DialogTitle, IconButton, LinearProgress, Paper, Snackbar, Stack, TextField, Typography } from '@mui/material'
import { ContentCopy, Memory } from '@mui/icons-material'
import jsQR from 'jsqr'
import { useEsimManager } from '../hooks/useEsimManager'
import { maskedIccid } from '../components/modemLineFormat'
import ErrorSnackbar from '../components/ErrorSnackbar'
import { formatCarrierName } from '../utils/carriers'
import { esimSwitchBlockReason } from '../utils/esimQuickSwitch'
import { CONFIRM_DELETE_PROFILE, euiccManufacturer, extractDefaultSmdp, maskedEid, parseLpaCode, profileDeleteBlockReason, profileStateLabel, redactEid, remainingStorage, validateDownload } from '../utils/esimPresentation'
import { copyPrivateText } from '../utils/copyPrivateText'
import type { EsimDownloadRequest, EsimProfile } from '../api/types'

type ManagerState = ReturnType<typeof useEsimManager>
type ProfileDialog = { type: 'details' | 'rename' | 'switch' | 'delete'; iccid: string }
const EMPTY_DOWNLOAD: EsimDownloadRequest = { smdp: '', matching_id: '', confirmation_code: '', imei: '' }

function ProfileDetails({ profile, defaultSmdp }: { profile: EsimProfile; defaultSmdp: string }) {
  const carrier = formatCarrierName(profile.mcc, profile.mnc)
  const fields = [
    ['名称', profile.name], ['提供商', profile.provider || (carrier === 'Unknown' ? '未知提供商' : carrier)],
    ['状态', profileStateLabel(profile)], ['ICCID', profile.iccid], ['本机号码 (MSISDN)', profile.msisdn],
    ['短信中心号码 (SMSC)', profile.smsc], ['IMSI', profile.imsi],
    ['MCC / MNC', [profile.mcc, profile.mnc].filter(Boolean).join(' / ')], ['Profile Class', profile.class],
    ['SM-DP+ 服务器', profile.smdp || defaultSmdp], ['标识码 (Matching ID)', profile.matching_id],
    ['ISDP-AID', profile.isdp_aid],
    ['允许删除', profile.delete_allowed === undefined ? '未知' : profile.delete_allowed ? '是' : '否'],
    ['允许禁用', profile.disable_allowed === undefined ? '未知' : profile.disable_allowed ? '是' : '否'],
  ]
  return <Box display="grid" gridTemplateColumns={{ xs: '1fr', sm: 'repeat(2, minmax(0, 1fr))' }} gap={1.5}>
    {fields.map(([label, value]) => <Paper variant="outlined" key={label} sx={{ p: 1.5, minWidth: 0 }}>
      <Typography variant="caption" color="text.secondary">{label}</Typography>
      <Typography variant="body2" sx={{ overflowWrap: 'anywhere' }}>{redactEid(value || '未读取')}</Typography>
    </Paper>)}
  </Box>
}

/** Inline cards only; each dialog is scoped to one operation, never a page. */
export function EsimProfileManager({ state, disabled = false, visible = true }: { state: ManagerState; disabled?: boolean; visible?: boolean }) {
  const { manager, euicc, profiles, lpac, loading, operation, needsRefresh, error, success } = state
  const [dialog, setDialog] = useState<ProfileDialog | null>(null)
  const [rename, setRename] = useState('')
  const [confirmation, setConfirmation] = useState('')
  const [downloadOpen, setDownloadOpen] = useState(false)
  const [smartInput, setSmartInput] = useState('')
  const [form, setForm] = useState<EsimDownloadRequest>(EMPTY_DOWNLOAD)
  const [qrLoading, setQrLoading] = useState(false)
  const qrEpoch = useRef(0)
  const selected = profiles.find((profile) => profile.iccid === dialog?.iccid)
  const busy = Boolean(operation)
  const unavailable = disabled || loading || busy || needsRefresh || !lpac?.usable || !state.detected
  const safeText = (value: string) => redactEid(value, euicc?.eid)

  const copyEid = async () => {
    if (!euicc?.eid) return
    try {
      await copyPrivateText(euicc.eid)
      manager.notify('完整 EID 已复制')
    } catch (err) { manager.reportError(err) }
  }
  const applyLpa = (input: string) => {
    setSmartInput(input)
    const parsed = parseLpaCode(input)
    if (parsed) setForm((previous) => ({ ...previous, ...parsed, confirmation_code: parsed.confirmation_code || '' }))
    return Boolean(parsed)
  }
  const readQr = async (file?: File) => {
    if (!file || unavailable || qrLoading) return
    const epoch = ++qrEpoch.current
    setQrLoading(true)
    try {
      const data = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader()
        reader.onload = () => typeof reader.result === 'string' ? resolve(reader.result) : reject(new Error('读取二维码图片失败'))
        reader.onerror = () => reject(new Error('读取二维码图片失败'))
        reader.readAsDataURL(file)
      })
      const image = await new Promise<HTMLImageElement>((resolve, reject) => {
        const img = new Image()
        img.onload = () => resolve(img)
        img.onerror = () => reject(new Error('无法读取二维码图片'))
        img.src = data
      })
      if (epoch !== qrEpoch.current) return
      const canvas = document.createElement('canvas')
      canvas.width = image.width
      canvas.height = image.height
      const context = canvas.getContext('2d')
      if (!context) throw new Error('当前浏览器不支持二维码图片解析')
      context.drawImage(image, 0, 0)
      const pixels = context.getImageData(0, 0, image.width, image.height)
      const result = jsQR(pixels.data, pixels.width, pixels.height)
      if (!result?.data || !applyLpa(result.data)) throw new Error('未在图片中检测到有效的 eSIM LPA 二维码')
      manager.notify('二维码解析成功，已填充参数')
    } catch (err) { if (epoch === qrEpoch.current) manager.reportError(err) }
    finally { if (epoch === qrEpoch.current) setQrLoading(false) }
  }
  const closeDownload = () => {
    if (busy) return
    qrEpoch.current++
    setQrLoading(false)
    setDownloadOpen(false)
  }
  const confirmProfileAction = async () => {
    if (!selected || !dialog || unavailable) return
    const done = dialog.type === 'rename' ? await manager.rename(selected.iccid, rename)
      : dialog.type === 'delete' ? await manager.delete(selected.iccid, confirmation)
        : dialog.type === 'switch' ? await manager.switch(selected.iccid) : false
    if (done) setDialog(null)
  }

  return <Box sx={{ minWidth: 0 }}>
    <ErrorSnackbar error={error} onClose={manager.clearError} />
    <Snackbar open={Boolean(success)} autoHideDuration={4000} onClose={manager.clearSuccess} anchorOrigin={{ vertical: 'top', horizontal: 'center' }}>
      <Alert severity="success" variant="filled" onClose={manager.clearSuccess}>{success}</Alert>
    </Snackbar>
    {visible && <>
      {loading && <LinearProgress aria-label="正在读取 eSIM" sx={{ mb: 1.5 }} />}
      {error && <Alert severity="warning" sx={{ mb: 1.5 }}>{error}</Alert>}
      {needsRefresh && <Alert severity="warning" sx={{ mb: 1.5 }}>请先刷新确认当前状态，勿连续重复提交。</Alert>}
      {operation && <Alert severity="info" sx={{ mb: 1.5 }}>{operation === 'switch' ? '切换中，请等待配置及网络恢复…' : '正在执行 eSIM 操作，请勿重复提交…'}</Alert>}
      {(lpac?.usable || euicc) && <>
        <Box data-testid="esim-summary" display="grid" gridTemplateColumns="repeat(2, minmax(0, 1fr))" gap={1.25} mb={2}>
          <Paper data-testid="esim-eid-card" variant="outlined" sx={{ p: 1.25, minWidth: 0 }}>
            <Typography variant="caption" color="text.secondary" display="block">EID</Typography>
            <Box display="flex" alignItems="center" gap={0.5}>
              <Typography component="span" variant="body2" data-testid="esim-eid" sx={{ fontFamily: 'monospace', overflowWrap: 'anywhere', minWidth: 0 }}>{maskedEid(euicc?.eid)}</Typography>
              <IconButton size="small" aria-label="复制完整 EID" onClick={() => void copyEid()} disabled={!euicc?.eid?.trim()} sx={{ flexShrink: 0 }}>
                <ContentCopy fontSize="small" />
              </IconButton>
            </Box>
          </Paper>
          <Paper variant="outlined" sx={{ p: 1.25, minWidth: 0 }}>
            <Typography variant="caption" color="text.secondary" display="block">剩余存储</Typography>
            <Typography variant="body2" sx={{ overflowWrap: 'anywhere' }}>{remainingStorage(euicc)}</Typography>
          </Paper>
          <Paper variant="outlined" sx={{ p: 1.25, minWidth: 0 }}>
            <Typography variant="caption" color="text.secondary" display="block">Profile 数量</Typography>
            <Typography variant="body2" fontWeight={700}>{profiles.length}</Typography>
          </Paper>
          <Paper variant="outlined" sx={{ p: 1.25, minWidth: 0 }}>
            <Typography variant="caption" color="text.secondary" display="block">下载配置</Typography>
            <Button size="small" variant="outlined" disabled={unavailable} onClick={() => { setForm(EMPTY_DOWNLOAD); setSmartInput(''); setDownloadOpen(true) }}>下载</Button>
          </Paper>
        </Box>
        <Box data-testid="esim-profiles" sx={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit, minmax(min(100%, 260px), 1fr))', gap: 1 }}>
          {profiles.map((profile) => {
            const switchBlocked = esimSwitchBlockReason(profile, profiles)
            const deleteBlocked = profileDeleteBlockReason(profile)
            return <Paper data-testid="esim-profile-card" variant="outlined" key={profile.iccid} sx={{ p: 1, minWidth: 0 }}>
              <Box display="flex" alignItems="center" justifyContent="space-between" gap={1} mb={0.5}>
                <Typography variant="body2" fontWeight={600} noWrap>{safeText(profile.name || profile.provider || '未命名 Profile')}</Typography>
                <Chip size="small" label={profileStateLabel(profile)} sx={{ height: 20, fontSize: '0.65rem', flexShrink: 0 }} />
              </Box>
              <Typography variant="caption" color="text.secondary" display="block" noWrap mb={1}>{maskedIccid(profile.iccid)}</Typography>
              <Box data-testid="esim-profile-actions" sx={{ display: 'grid', gridTemplateColumns: 'repeat(4, minmax(0, 1fr))', gap: 0.5, '& .MuiButton-root': { minWidth: 0, px: 0.25, fontSize: '0.75rem', whiteSpace: 'nowrap' } }}>
                <Button size="small" variant="outlined" disabled={busy} onClick={() => setDialog({ type: 'details', iccid: profile.iccid })}>详情</Button>
                <Button size="small" variant="outlined" disabled={unavailable} onClick={() => { setRename(profile.name || ''); setDialog({ type: 'rename', iccid: profile.iccid }) }}>重命名</Button>
                <Button size="small" variant="outlined" title={switchBlocked || undefined} disabled={unavailable || Boolean(switchBlocked)} onClick={() => setDialog({ type: 'switch', iccid: profile.iccid })}>切换</Button>
                <Button size="small" variant="outlined" color="error" title={deleteBlocked || undefined} disabled={unavailable || Boolean(deleteBlocked)} onClick={() => { setConfirmation(''); setDialog({ type: 'delete', iccid: profile.iccid }) }}>删除</Button>
              </Box>
            </Paper>
          })}
        </Box>
        {!loading && profiles.length === 0 && <Typography variant="body2" color="text.secondary">尚未读取到 Profile，可下载配置。</Typography>}
      </>}
    </>}

    <Dialog open={Boolean(dialog)} onClose={busy ? undefined : () => setDialog(null)} fullWidth maxWidth={dialog?.type === 'details' ? 'sm' : 'xs'}>
      <DialogTitle>{dialog?.type === 'details' ? 'Profile 详情' : dialog?.type === 'rename' ? '重命名 Profile' : dialog?.type === 'switch' ? '切换 eSIM 配置' : '确认删除 Profile'}</DialogTitle>
      <DialogContent>
        {selected && dialog?.type === 'details' && <ProfileDetails profile={selected} defaultSmdp={extractDefaultSmdp(euicc?.raw)} />}
        {dialog?.type === 'rename' && <TextField fullWidth autoFocus margin="dense" label="Profile 名称" value={rename} disabled={busy} onChange={(event) => setRename(event.target.value)} />}
        {dialog?.type === 'switch' && <DialogContentText>确认切换到 {safeText(selected?.name || selected?.provider || '所选配置')}？当前线路连接和通话可能中断。</DialogContentText>}
        {dialog?.type === 'delete' && <>
          <DialogContentText sx={{ mb: 2 }}>确定删除「{safeText(selected?.name || '未命名')} · {maskedIccid(selected?.iccid || '')}」？该操作不可撤销，请输入「{CONFIRM_DELETE_PROFILE}」继续操作。</DialogContentText>
          <TextField fullWidth autoFocus label={`请输入 ${CONFIRM_DELETE_PROFILE}`} value={confirmation} disabled={busy} onChange={(event) => setConfirmation(event.target.value)} />
        </>}
        {busy && <Box display="flex" alignItems="center" gap={1} mt={2}><CircularProgress size={18} /><Typography variant="body2">{operation === 'switch' ? '正在切换并核实状态…' : '正在处理…'}</Typography></Box>}
      </DialogContent>
      <DialogActions>
        <Button disabled={busy} onClick={() => setDialog(null)}>{dialog?.type === 'details' ? '关闭' : '取消'}</Button>
        {dialog?.type !== 'details' && <Button variant="contained" color={dialog?.type === 'delete' ? 'error' : 'primary'} onClick={() => void confirmProfileAction()} disabled={unavailable || !selected || (dialog?.type === 'rename' && !rename.trim()) || (dialog?.type === 'delete' && (confirmation !== CONFIRM_DELETE_PROFILE || Boolean(selected && profileDeleteBlockReason(selected)))) || (dialog?.type === 'switch' && Boolean(selected && esimSwitchBlockReason(selected, profiles)))}>
          {dialog?.type === 'delete' ? '确认删除' : dialog?.type === 'switch' ? '确认切换' : '保存'}
        </Button>}
      </DialogActions>
    </Dialog>

    <Dialog open={downloadOpen} onClose={closeDownload} fullWidth maxWidth="sm">
      <DialogTitle>下载 eSIM 配置</DialogTitle>
      <DialogContent>
        <Stack spacing={2} sx={{ pt: 1 }}>
          <TextField fullWidth multiline rows={3} label="LPA 激活码" placeholder="LPA:1$smdp.io$matching-id" value={smartInput} disabled={busy || qrLoading} onChange={(event) => applyLpa(event.target.value)} helperText={smartInput ? parseLpaCode(smartInput) ? '解析成功，请核对下方参数' : '未识别有效 LPA 激活码，请检查或手动填写参数' : '支持粘贴激活码、二维码或手动填写'} />
          <Box onDragOver={(event) => event.preventDefault()} onDrop={(event) => { event.preventDefault(); void readQr(event.dataTransfer.files[0]) }}>
            <Button component="label" size="small" variant="outlined" disabled={unavailable || qrLoading}>
              {qrLoading ? '解析中…' : '选择或拖入二维码图片'}
              <input type="file" accept="image/*" hidden onChange={(event) => { void readQr(event.target.files?.[0]); event.target.value = '' }} />
            </Button>
          </Box>
          <TextField required fullWidth label="SM-DP+ 服务器地址" value={form.smdp} disabled={busy} onChange={(event) => setForm({ ...form, smdp: event.target.value })} />
          <TextField required fullWidth label="标识码 (Matching ID)" value={form.matching_id} disabled={busy} onChange={(event) => setForm({ ...form, matching_id: event.target.value })} />
          <TextField fullWidth label="确认码 (选填)" value={form.confirmation_code} disabled={busy} onChange={(event) => setForm({ ...form, confirmation_code: event.target.value })} />
          <TextField fullWidth label="绑定 IMEI (选填)" value={form.imei} disabled={busy} onChange={(event) => setForm({ ...form, imei: event.target.value })} />
          {operation === 'download' && <><LinearProgress /><Typography variant="body2">正在下载并写入配置，请勿断电或重复提交…</Typography></>}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button disabled={busy} onClick={closeDownload}>取消</Button>
        <Button variant="contained" disabled={unavailable || qrLoading || Boolean(validateDownload(form))} onClick={() => void manager.download(form).then((done) => { if (done) closeDownload() })}>开始写卡</Button>
      </DialogActions>
    </Dialog>
  </Box>
}

function StandaloneEsimManager({ lineId }: { lineId: string }) {
  const state = useEsimManager(lineId)
  return <Box>
    <Box display="flex" justifyContent="space-between" gap={1} mb={2}>
      <Box><Typography variant="subtitle1" fontWeight={800}><Memory fontSize="small" /> eSIM 管理</Typography><Typography variant="caption" color="text.secondary">{euiccManufacturer(state.euicc)}</Typography></Box>
      <Button size="small" variant="outlined" disabled={state.loading || Boolean(state.operation)} onClick={() => void state.manager.load(true)}>刷新</Button>
    </Box>
    <EsimProfileManager state={state} />
  </Box>
}

// Retain the existing line-specific navigation/import contract, with identical
// cards and store rather than a second full-management implementation.
export default function EsimManagerPage({ lineId }: { lineId: string }) {
  return <StandaloneEsimManager key={lineId} lineId={lineId} />
}
