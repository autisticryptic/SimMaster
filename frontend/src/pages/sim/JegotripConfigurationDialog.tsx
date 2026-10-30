import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogActions, DialogContent, DialogTitle, Stack, TextField, Typography } from '@mui/material'
import { api, type JegotripConfigurationView } from '../../api/current'
import { appLoginError } from '../../policies/appLogin'

export default function JegotripConfigurationDialog({ open, onClose, onSaved }: { open: boolean; onClose: () => void; onSaved: () => void }) {
  const [view, setView] = useState<JegotripConfigurationView | null>(null)
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef({ active: false })
  useEffect(() => {
    const scope = { active: open }
    generation.current = scope
    setView(null); setText(''); setError(null); setBusy(false)
    if (open) void api.getJegotripConfiguration().then((response) => {
      if (scope.active) setView(response.data ?? null)
    }).catch(() => { if (scope.active) setError('无法读取安装配置状态。') })
    return () => { scope.active = false }
  }, [open])
  const save = async () => {
    if (busy || !view?.initialized || !text.trim()) return
    const scope = generation.current
    let configuration: unknown
    try {
      if (new TextEncoder().encode(text).length > 256 * 1024) throw new Error('size')
      configuration = JSON.parse(text)
      if (!configuration || typeof configuration !== 'object' || Array.isArray(configuration)) throw new Error('shape')
    } catch { setError('请提供不超过 256 KiB 的完整安装配置 JSON 对象，不要填个人账号密码或电话 token。'); return }
    setBusy(true); setError(null)
    try {
      const response = await api.saveJegotripConfiguration(view.revision, configuration)
      if (scope.active) { setView(response.data ?? null); setText(''); onSaved() }
    } catch (failure) { if (scope.active) setError(appLoginError(failure)) }
    finally { if (scope.active) { setBusy(false); setText('') } }
  }
  const clear = async () => {
    if (busy || !view?.initialized || !window.confirm('清空安装配置？已提交任务保持原快照，已有账号/历史和接收器不会被删除或自动停止。')) return
    const scope = generation.current
    setBusy(true); setError(null)
    try {
      const response = await api.clearJegotripConfiguration(view.revision)
      if (scope.active) { setView(response.data ?? null); setText(''); onSaved() }
    } catch (failure) { if (scope.active) setError(appLoginError(failure)) }
    finally { if (scope.active) setBusy(false) }
  }
  return <Dialog open={open} onClose={busy ? undefined : onClose} fullWidth maxWidth="md">
    <DialogTitle>无忧行安装配置（独立数据库表）</DialogTitle>
    <DialogContent><Stack spacing={2} sx={{ pt: 1 }}>
      <Alert severity="info">配置写入 jegotrip_login_settings，不再必须放置 JSON 文件。旧秘密值不会回显；保存或关闭后清空输入。安装参数不是个人 SDK 凭据。</Alert>
      {view && <Typography variant="body2">来源：{view.configuration_source} · 版本：{view.revision} · App HTTP：{view.http_configured ? '已录入' : '未录入'} · 原生接收：{view.native_configured ? '已录入，运行时另核验' : '未配置，不能登录启动'}</Typography>}
      <Typography variant="caption" color="text.secondary">格式为 version: 1、http 和 runtime。主入口必须使用完整接收上下文登录并启动；runtime 为 null 仅保留安装配置兼容，不会让主入口退回仅认证。原 App 共享默认参数的自动装配仍待原生启动链核实；不要用占位值或个人 phone token/SipPwd 补齐。</Typography>
      <Button component="label" variant="outlined" disabled={busy}>选择私有安装 JSON
        <input hidden type="file" accept=".json,application/json" onChange={(event) => {
          const file = event.target.files?.[0]
          event.target.value = ''
          const scope = generation.current
          if (!file) return
          if (file.size > 256 * 1024) { setError('文件超过 256 KiB。'); return }
          void file.text().then((value) => { if (scope.active) setText(value) }).catch(() => { if (scope.active) setError('无法读取所选文件。') })
        }} />
      </Button>
      <TextField label="写入新安装配置（旧值不回显）" value={text} multiline minRows={7} disabled={busy} onChange={(event) => setText(event.target.value)} />
      <Typography variant="caption" color="text.secondary">数据库包含私有参数，不是加密保险箱。仅通过 HTTPS 或可信本机管理；不要把文件、参数或数据库提交到公开仓库。</Typography>
      {error && <Alert severity="error">{error}</Alert>}
    </Stack></DialogContent>
    <DialogActions>
      <Button color="warning" disabled={busy || !view?.initialized || view.revision === 0} onClick={() => { void clear() }}>清空配置</Button>
      <Button disabled={busy} onClick={onClose}>关闭</Button>
      <Button variant="contained" disabled={busy || !view?.initialized || !text.trim()} onClick={() => { void save() }}>保存到数据库</Button>
    </DialogActions>
  </Dialog>
}
