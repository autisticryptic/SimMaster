import type { ReactNode } from 'react'
import { Alert, Box, Chip, Typography } from '@mui/material'
import Grid from '@mui/material/Grid'
import type { TrunkProfileResponse, VolteLineControlResponse, VowifiLineConfigResponse } from '../../api/current'

function Field({ label, value }: { label: string, value: ReactNode }) {
  return (
    <Box minWidth={0}>
      <Typography variant="caption" color="text.secondary">{label}</Typography>
      <Typography variant="body2" sx={{ mt: 0.25, wordBreak: 'break-word' }}>{value}</Typography>
    </Box>
  )
}

export function LineCsDetails({ line }: { line: VolteLineControlResponse }) {
  return (
    <Grid container spacing={2}>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="基带状态" value={line.modem.state || '未知'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运营商" value={line.modem.operator_id || '未读取'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="当前设备" value={line.modem.present ? '已连接' : '未连接'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="ModemManager 路径" value={line.modem.modem_path || '未发现'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="主控制端口" value={line.modem.primary_port || '未发现'} /></Grid>
    </Grid>
  )
}

export function LineVolteDetails({ line }: { line: VolteLineControlResponse }) {
  const attempts = [...(line.runtime.connection_attempts ?? [])].reverse()
  return (
    <Grid container spacing={2}>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="IMS 阶段" value={`${line.runtime.phase} / ${line.runtime.stage}`} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="注册状态" value={line.runtime.registered ? <Chip size="small" color="success" label="已注册" /> : '未注册'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="注册方式" value={line.runtime.registration_mode || '未确定'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="地址族" value={line.runtime.current_ip_family || '尚未选择'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="Bearer 类型" value={line.runtime.bearer_ip_type || '尚未建立'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="Bearer 网卡" value={line.runtime.bearer_interface || '尚未建立'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="数据 QMI 端口" value={line.runtime.qmi_device || line.modem.qmi_device || '未发现'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}>
        <Field
          label="IMS QMI 端口"
          value={line.runtime.secondary_qmi_device
            ? `${line.runtime.secondary_qmi_device}${line.runtime.secondary_qmi_channel ? ` · ${line.runtime.secondary_qmi_channel}` : ''}`
            : '未启用（IMS 与数据共用主端口）'}
        />
      </Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="P-CSCF" value={line.runtime.pcscf || '尚未发现'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="IMS 数据路径" value={line.runtime.data_path_mode || '尚未建立'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="REGISTER 续期" value={`${line.runtime.register_refresh_count ?? 0} 次${line.runtime.last_register_refresh_at ? ` · ${new Date(line.runtime.last_register_refresh_at).toLocaleString()}` : ''}`} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="身份来源" value={line.runtime.identity_source || '尚未读取'} /></Grid>
      <Grid size={{ xs: 12, sm: 6 }}><Field label="ISIM" value={line.runtime.isim_aid ? `已发现 · ${line.runtime.isim_aid}` : '未发现，使用 IMSI 回退'} /></Grid>
      {line.runtime.last_error && <Grid size={12}><Alert severity="warning">{line.runtime.last_error}</Alert></Grid>}
      <Grid size={12}>
        <Typography variant="subtitle2" mb={1}>最近连接尝试</Typography>
        <Box sx={{ maxHeight: 260, overflowY: 'auto', borderTop: 1, borderColor: 'divider' }}>
          {attempts.length === 0 && <Typography variant="body2" color="text.secondary" py={1}>尚无连接记录</Typography>}
          {attempts.map((attempt) => (
            <Box key={`${attempt.sequence}-${attempt.at}`} display="grid" gridTemplateColumns={{ xs: '1fr auto', sm: '150px minmax(0, 1fr) auto' }} gap={1} py={0.75} borderBottom={1} borderColor="divider" alignItems="center">
              <Typography variant="caption" color="text.secondary">{new Date(attempt.at).toLocaleTimeString()}</Typography>
              <Box minWidth={0}>
                <Typography variant="body2" sx={{ wordBreak: 'break-word' }}>{attempt.stage}{attempt.ip_family ? ` · ${attempt.ip_family}` : ''}{attempt.detail ? ` · ${attempt.detail}` : ''}</Typography>
                {[attempt.at_cid !== undefined && attempt.at_cid !== null ? `CID ${attempt.at_cid}` : null, attempt.qmi_device ? `QMI ${attempt.qmi_device}` : null, attempt.interface ? `网卡 ${attempt.interface}` : null, attempt.bearer_path ? `Bearer ${attempt.bearer_path}` : null, attempt.pcscf ? `P-CSCF ${attempt.pcscf}` : null].filter(Boolean).length > 0 && (
                  <Typography variant="caption" color="text.secondary" sx={{ wordBreak: 'break-all' }}>
                    {[attempt.at_cid !== undefined && attempt.at_cid !== null ? `CID ${attempt.at_cid}` : null, attempt.qmi_device ? `QMI ${attempt.qmi_device}` : null, attempt.interface ? `网卡 ${attempt.interface}` : null, attempt.bearer_path ? `Bearer ${attempt.bearer_path}` : null, attempt.pcscf ? `P-CSCF ${attempt.pcscf}` : null].filter(Boolean).join(' · ')}
                  </Typography>
                )}
              </Box>
              <Chip size="small" variant="outlined" color={attempt.outcome === 'succeeded' ? 'success' : attempt.outcome === 'failed' ? 'error' : 'default'} label={attempt.outcome === 'succeeded' ? '成功' : attempt.outcome === 'failed' ? '失败' : '进行中'} />
              {attempt.error_code && <Typography variant="caption" color="error" sx={{ gridColumn: { xs: '1 / -1', sm: '2 / -1' }, wordBreak: 'break-all' }}>{attempt.error_code}</Typography>}
            </Box>
          ))}
        </Box>
      </Grid>
    </Grid>
  )
}

export function LineVowifiDetails({ vowifi }: { vowifi?: VowifiLineConfigResponse }) {
  if (!vowifi) return <Alert severity="info">尚未加载该线路的 VoWiFi 状态。</Alert>
  return (
    <Grid container spacing={2}>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运行阶段" value={`${vowifi.runtime_phase} / ${vowifi.runtime_stage}`} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="IMS 注册" value={vowifi.runtime_registered ? '已注册' : '未注册'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运行范围" value="线路独立运行时" /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运营商 profile" value={vowifi.matched_profile_id || '尚未匹配'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="代理模式" value={vowifi.config.proxy_mode} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="代理端点" value={vowifi.config.proxy_endpoint || '直连'} /></Grid>
      {vowifi.runtime_error && <Grid size={12}><Alert severity="warning">{vowifi.runtime_error}</Alert></Grid>}
    </Grid>
  )
}

export function LineTrunkDetails({ trunk }: { trunk?: TrunkProfileResponse }) {
  return (
    <Grid container spacing={2}>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="运行阶段" value={trunk ? `${trunk.runtime.phase} / ${trunk.runtime.stage}` : '未加载'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="注册状态" value={trunk?.runtime.registered ? '已注册' : '未注册'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="本地 SIP" value={trunk?.runtime.local_endpoint || '未监听'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="Asterisk Peer" value={trunk?.runtime.peer || '未解析'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="REGISTER / 重连" value={trunk ? `${trunk.runtime.register_attempts} / ${trunk.runtime.reconnect_count}` : '0 / 0'} /></Grid>
      <Grid size={{ xs: 12, sm: 4 }}><Field label="通话 / 对话" value={trunk ? `${trunk.runtime.active_calls} / ${trunk.runtime.active_dialogs}` : '0 / 0'} /></Grid>
      {trunk?.runtime.last_error && <Grid size={12}><Alert severity="error">{trunk.runtime.last_error}</Alert></Grid>}
    </Grid>
  )
}
