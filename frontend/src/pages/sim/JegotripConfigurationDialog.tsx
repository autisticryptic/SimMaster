import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogActions, DialogContent, DialogTitle, Stack, TextField, Typography } from '@mui/material'
import { api, type JegotripConfigurationView } from '../../api/current'
import { appLoginError } from '../../policies/appLogin'

export default function JegotripConfigurationDialog({ open, onClose, onSaved }: { open: boolean; onClose: () => void; onSaved: () => void }) {
  const [view, setView] = useState<JegotripConfigurationView | null>(null)
  const [text, setText] = useState('')
  const [advanced, setAdvanced] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const generation = useRef({ active: false })
  const fileGeneration = useRef(0)
  useEffect(() => {
    const scope = { active: open }
    generation.current = scope
    fileGeneration.current += 1
    setView(null); setText(''); setAdvanced(false); setError(null); setBusy(false)
    if (open) void api.getJegotripConfiguration().then((response) => {
      if (scope.active) setView(response.data ?? null)
    }).catch(() => { if (scope.active) setError('无法读取安装配置状态。') })
    return () => { scope.active = false }
  }, [open])
  const save = async () => {
    if (busy || !advanced || !view?.initialized || !text.trim()) return
    const scope = generation.current
    fileGeneration.current += 1
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
    fileGeneration.current += 1
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
      <Alert severity="info">
        <Typography variant="subtitle2">常规使用的最小输入</Typography>
        <Typography variant="body2">只需已绑定号码的本人账号、密码和明确的托管选择；手机号登录另填区号。电话 token/SIP 密码由后台取得，不用手工填写。</Typography>
        <Typography variant="body2">App 设备 UUID 默认随机生成并持久复用，不需要手机硬件 IMEI、型号或模拟器。手工指定自己的标识是可选项，不代表已授信。</Typography>
      </Alert>
      <Alert severity="warning">授信由服务器认可的账号/号码/设备上下文决定，不是一串能替代所有配置的永久代码。运营商认证 token、App 会话、电话 token 和 SDK 设备标识不能混用；接收器与媒体仍须真实核验。</Alert>
      <Alert severity="info">
        <Typography variant="subtitle2">手机授信后迁移到 SimAdmin</Typography>
        <Typography variant="body2">在本人手机完成合法授信后，复用同一账号/号码与 App 的 phone_uuid / device.dat 原样 UUID。登录表单选择「复用手机 App 已授信 UUID」，默认只接管已开启托管；本次 getCallStatus 明确认可后才继续，不 enroll、不请求 VTD 或生成另一 UUID。</Typography>
        <Typography variant="body2">业务未开启时，另选「复用已授信 UUID 并开启托管」并明确授权；不从旧查询记录取得许可。可选设备同步只使用本人手机真实报告资料，缺值不猜，也不把报告接受当作授信或收件成功。</Typography>
        <Typography variant="body2">已有账号要换 UUID，请先在设备标识管理中选择原号码并录入手机 UUID；版本守卫和旧值保护不变。不要复制本地 phone_safe_num、enrollId 或 SDK 的另一个 UUID，也不要勾选本地“已授信”来替代服务器结果。</Typography>
      </Alert>
      <Typography variant="body2" color="text.secondary">服务地址、可信证书和 SDK 应用参数属于安装侧共享协议配置，不是每个账号的手机硬件信息。正常登录无需反复修改；完整原生安装上下文尚未准备好时，不会降级为仅认证或假装托管成功。</Typography>
      <Button size="small" variant="outlined" disabled={busy} aria-expanded={advanced} aria-controls="jegotrip-private-installation"
        onClick={() => { fileGeneration.current += 1; setText(''); setError(null); setAdvanced(!advanced) }}>
        {advanced ? '收起高级安装配置并清空输入' : '展开高级安装配置'}
      </Button>
      {advanced && <Stack id="jegotrip-private-installation" spacing={2}>
        <Typography variant="caption" color="text.secondary">完整 JSON 格式为 version: 1、http 和 runtime。http.device_identity_mode 默认为 store，device_id 可省略；static 仅保留旧固定值兼容，不能跨号码复用。SDK 设备参数、ClientId、接收器 namespace 和媒体仍是独立原生上下文，不会被 App UUID 擅自替换。runtime 为 null 仅保留认证兼容。</Typography>
        <Typography variant="caption" color="text.secondary">可选 http.endpoints.device_sync 与 http.device_sync_profile 仅供已授信复用后的原设备报告。只填写本人手机观察到的 App 版本/版本码、型号、OS、渠道等资料；不制造厂商推送 token、MAC 或硬件参数。userId 从本次 user-info.accountid 取得，device_id/imei 固定使用所选原 App UUID；未配置报告资料时不发送同步。</Typography>
        <Button component="label" variant="outlined" disabled={busy}>选择私有安装 JSON
          <input hidden type="file" accept=".json,application/json" onChange={(event) => {
            const file = event.target.files?.[0]
            event.target.value = ''
            const scope = generation.current
            const selected = ++fileGeneration.current
            if (!file) return
            setText(''); setError(null)
            if (file.size > 256 * 1024) { setError('文件超过 256 KiB。'); return }
            void file.text().then((value) => {
              if (scope.active && selected === fileGeneration.current) setText(value)
            }).catch(() => { if (scope.active && selected === fileGeneration.current) setError('无法读取所选文件。') })
          }} />
        </Button>
        <TextField label="写入新安装配置（旧值不回显）" value={text} multiline minRows={7} disabled={busy}
          onChange={(event) => { fileGeneration.current += 1; setText(event.target.value) }} />
        <Typography variant="caption" color="text.secondary">数据库包含私有参数，不是加密保险箱。仅通过 HTTPS 或可信本机管理；不要把文件、参数或数据库提交到公开仓库。收起、保存或关闭都会清空输入。</Typography>
      </Stack>}
      {error && <Alert severity="error">{error}</Alert>}
    </Stack></DialogContent>
    <DialogActions>
      <Button color="warning" disabled={busy || !view?.initialized || view.revision === 0} onClick={() => { void clear() }}>清空配置</Button>
      <Button disabled={busy} onClick={onClose}>关闭</Button>
      <Button variant="contained" disabled={busy || !advanced || !view?.initialized || !text.trim()} onClick={() => { void save() }}>保存到数据库</Button>
    </DialogActions>
  </Dialog>
}
