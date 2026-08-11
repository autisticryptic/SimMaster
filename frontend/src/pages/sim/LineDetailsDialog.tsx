import { useState, type ReactNode } from 'react'
import { Alert, Box, Chip, Dialog, DialogContent, DialogTitle, IconButton, Tab, Tabs, Typography } from '@mui/material'
import Grid from '@mui/material/Grid'
import { Close } from '@mui/icons-material'
import type { TrunkProfileResponse, VolteLineControlResponse, VowifiLineConfigResponse } from '../../api/current'
import { maskedIccid, modemSlotLabel, modemSlotSourceLabel, shortLineId } from '../../components/modemLineFormat'
import LineCellularSettings from './LineCellularSettings'

export type LineDetailTab = 'basic' | 'cs' | 'volte' | 'vowifi' | 'trunk' | 'cells' | 'apn' | 'operator'

type Props = {
  open: boolean
  line: VolteLineControlResponse | null
  trunk?: TrunkProfileResponse
  vowifi?: VowifiLineConfigResponse
  initialTab: LineDetailTab
  basicInfo?: ReactNode
  onClose: () => void
}

function Field({ label, value }: { label: string, value: ReactNode }) {
  return <Box minWidth={0}><Typography variant="caption" color="text.secondary">{label}</Typography><Typography variant="body2" sx={{ mt: 0.25, wordBreak: 'break-word' }}>{value}</Typography></Box>
}

function VolteDetails({ line }: { line: VolteLineControlResponse }) {
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
        <Box sx={{ maxHeight: 220, overflowY: 'auto', borderTop: 1, borderColor: 'divider' }}>
          {attempts.length === 0 && <Typography variant="body2" color="text.secondary" py={1}>尚无连接记录</Typography>}
          {attempts.map((attempt) => (
            <Box key={`${attempt.sequence}-${attempt.at}`} display="grid" gridTemplateColumns={{ xs: '1fr auto', sm: '150px minmax(0, 1fr) auto' }} gap={1} py={0.75} borderBottom={1} borderColor="divider" alignItems="center">
              <Typography variant="caption" color="text.secondary">{new Date(attempt.at).toLocaleTimeString()}</Typography>
              <Box minWidth={0}>
                <Typography variant="body2" sx={{ wordBreak: 'break-word' }}>{attempt.stage}{attempt.ip_family ? ` · ${attempt.ip_family}` : ''}{attempt.detail ? ` · ${attempt.detail}` : ''}</Typography>
                {(() => {
                  const meta = [
                    attempt.at_cid !== undefined && attempt.at_cid !== null ? `CID ${attempt.at_cid}` : null,
                    attempt.qmi_device ? `QMI ${attempt.qmi_device}` : null,
                    attempt.interface ? `网卡 ${attempt.interface}` : null,
                    attempt.bearer_path ? `Bearer ${attempt.bearer_path}` : null,
                    attempt.pcscf ? `P-CSCF ${attempt.pcscf}` : null,
                  ].filter(Boolean)
                  return meta.length > 0
                    ? <Typography variant="caption" color="text.secondary" sx={{ wordBreak: 'break-all' }}>{meta.join(' · ')}</Typography>
                    : null
                })()}
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

export default function LineDetailsDialog({ open, line, trunk, vowifi, initialTab, basicInfo, onClose }: Props) {
  const [tab, setTab] = useState<LineDetailTab>(initialTab)
  if (!line) return null

  const tabs: Array<{ value: LineDetailTab, label: string }> = [
    { value: 'basic', label: '基本信息' },
    { value: 'cs', label: 'CS 连接' },
    { value: 'volte', label: 'VoLTE' },
    { value: 'vowifi', label: 'VoWiFi' },
    { value: 'trunk', label: 'Trunk' },
    { value: 'cells', label: '小区与锁定' },
    { value: 'apn', label: 'APN 配置' },
    { value: 'operator', label: '运营商管理' },
  ]

  return (
    <Dialog open={open} onClose={onClose} fullWidth maxWidth="lg">
      <DialogTitle sx={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 2 }}>
        <Box minWidth={0}>
          <Typography variant="h6">{modemSlotLabel(line.modem)} · 卡槽 {line.modem.uim_slot}</Typography>
          <Typography variant="caption" color="text.secondary">线路 {shortLineId(line.modem.line_id)} · {line.modem.present ? '在线' : '离线保留'}</Typography>
        </Box>
        <IconButton aria-label="关闭" onClick={onClose}><Close /></IconButton>
      </DialogTitle>
      <Tabs value={tabs.some((item) => item.value === tab) ? tab : 'basic'} onChange={(_, value: LineDetailTab) => setTab(value)} variant="scrollable" scrollButtons="auto" sx={{ px: 3, borderBottom: 1, borderColor: 'divider' }}>
        {tabs.map((item) => <Tab key={item.value} value={item.value} label={item.label} />)}
      </Tabs>
      <DialogContent sx={{ minHeight: 360 }}>
        {tab === 'basic' && (basicInfo ?? <Grid container spacing={2}><Grid size={{ xs: 12, sm: 6 }}><Field label="ICCID" value={maskedIccid(line.modem.sim_iccid)} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="物理槽位" value={`${modemSlotLabel(line.modem)} · ${modemSlotSourceLabel(line.modem.slot_source, line.modem.slot_stable)}`} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="基带" value={`${line.modem.manufacturer || '未知厂商'} ${line.modem.model || ''}`} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="QMI / UIM" value={`${line.modem.qmi_device || '未发现'} · Slot ${line.modem.uim_slot}`} /></Grid></Grid>)}
        {tab === 'cs' && <Grid container spacing={2}><Grid size={{ xs: 12, sm: 4 }}><Field label="基带状态" value={line.modem.state || '未知'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="运营商" value={line.modem.operator_id || '未读取'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="当前设备" value={line.modem.present ? '已连接' : '未连接'} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="ModemManager 路径" value={line.modem.modem_path} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="主控制端口" value={line.modem.primary_port || '未发现'} /></Grid></Grid>}
        {tab === 'volte' && <VolteDetails line={line} />}
        {tab === 'vowifi' && vowifi && <Grid container spacing={2}><Grid size={{ xs: 12, sm: 4 }}><Field label="运行阶段" value={`${vowifi.runtime_phase} / ${vowifi.runtime_stage}`} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="IMS 注册" value={vowifi.runtime_registered ? '已注册' : '未注册'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="运行范围" value="线路独立运行时" /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="运营商 profile" value={vowifi.matched_profile_id || '尚未匹配'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="SIM 网络参数" value="按 SIM 覆写解析" /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="代理模式" value={vowifi.config.proxy_mode} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="代理端点" value={vowifi.config.proxy_endpoint || '直连'} /></Grid>{vowifi.runtime_error && <Grid size={12}><Alert severity="warning">{vowifi.runtime_error}</Alert></Grid>}</Grid>}
        {tab === 'trunk' && <Grid container spacing={2}><Grid size={{ xs: 12, sm: 4 }}><Field label="运行阶段" value={trunk ? `${trunk.runtime.phase} / ${trunk.runtime.stage}` : '未加载'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="注册状态" value={trunk?.runtime.registered ? '已注册' : '未注册'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="本地 SIP" value={trunk?.runtime.local_endpoint || '未监听'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="Asterisk Peer" value={trunk?.runtime.peer || '未解析'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="REGISTER / 重连" value={trunk ? `${trunk.runtime.register_attempts} / ${trunk.runtime.reconnect_count}` : '0 / 0'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="通话 / 对话" value={trunk ? `${trunk.runtime.active_calls} / ${trunk.runtime.active_dialogs}` : '0 / 0'} /></Grid>{trunk?.runtime.last_error && <Grid size={12}><Alert severity="error">{trunk.runtime.last_error}</Alert></Grid>}</Grid>}
        {(tab === 'cells' || tab === 'apn' || tab === 'operator') && <LineCellularSettings section={tab} lineId={line.modem.line_id} lineLabel={`${modemSlotLabel(line.modem)} · 卡槽 ${line.modem.uim_slot}`} />}
      </DialogContent>
    </Dialog>
  )
}
