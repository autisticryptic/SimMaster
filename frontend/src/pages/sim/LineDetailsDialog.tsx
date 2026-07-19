import { useState, type ReactNode } from 'react'
import { Alert, Box, Chip, Dialog, DialogContent, DialogTitle, IconButton, Tab, Tabs, Typography } from '@mui/material'
import Grid from '@mui/material/Grid'
import { Close } from '@mui/icons-material'
import type { TrunkProfileResponse, VolteLineControlResponse, VowifiLineConfigResponse } from '../../api/current'
import { maskedIccid, modemSlotLabel, modemSlotSourceLabel, shortLineId } from '../../components/modemLineFormat'

export type LineDetailTab = 'basic' | 'cs' | 'volte' | 'vowifi' | 'trunk'

type Props = {
  open: boolean
  line: VolteLineControlResponse | null
  trunk?: TrunkProfileResponse
  vowifi?: VowifiLineConfigResponse
  initialTab: LineDetailTab
  primaryBasicInfo?: ReactNode
  onClose: () => void
}

function Field({ label, value }: { label: string, value: ReactNode }) {
  return <Box minWidth={0}><Typography variant="caption" color="text.secondary">{label}</Typography><Typography variant="body2" sx={{ mt: 0.25, wordBreak: 'break-word' }}>{value}</Typography></Box>
}

export default function LineDetailsDialog({ open, line, trunk, vowifi, initialTab, primaryBasicInfo, onClose }: Props) {
  const [tab, setTab] = useState<LineDetailTab>(initialTab)
  if (!line) return null

  const tabs: Array<{ value: LineDetailTab, label: string }> = [
    { value: 'basic', label: '基本信息' },
    { value: 'cs', label: 'CS 连接' },
    { value: 'volte', label: 'VoLTE' },
    { value: 'vowifi', label: 'VoWiFi' },
    { value: 'trunk', label: 'Trunk' },
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
        {tab === 'basic' && (primaryBasicInfo ?? <Grid container spacing={2}><Grid size={{ xs: 12, sm: 6 }}><Field label="ICCID" value={maskedIccid(line.modem.sim_iccid)} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="物理槽位" value={`${modemSlotLabel(line.modem)} · ${modemSlotSourceLabel(line.modem.slot_source, line.modem.slot_stable)}`} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="基带" value={`${line.modem.manufacturer || '未知厂商'} ${line.modem.model || ''}`} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="QMI / UIM" value={`${line.modem.qmi_device || '未发现'} · Slot ${line.modem.uim_slot}`} /></Grid></Grid>)}
        {tab === 'cs' && <Grid container spacing={2}><Grid size={{ xs: 12, sm: 4 }}><Field label="基带状态" value={line.modem.state || '未知'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="运营商" value={line.modem.operator_id || '未读取'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="当前设备" value={line.modem.present ? '已连接' : '未连接'} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="ModemManager 路径" value={line.modem.modem_path} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="主控制端口" value={line.modem.primary_port || '未发现'} /></Grid></Grid>}
        {tab === 'volte' && <Grid container spacing={2}><Grid size={{ xs: 12, sm: 4 }}><Field label="IMS 阶段" value={`${line.runtime.phase} / ${line.runtime.stage}`} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="注册状态" value={line.runtime.registered ? <Chip size="small" color="success" label="已注册" /> : '未注册'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="注册方式" value={line.runtime.registration_mode || '未确定'} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="IMS 数据路径" value={line.runtime.data_path_mode || '尚未建立'} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="P-CSCF" value={line.runtime.pcscf || '尚未发现'} /></Grid>{line.runtime.last_error && <Grid size={12}><Alert severity="warning">{line.runtime.last_error}</Alert></Grid>}</Grid>}
        {tab === 'vowifi' && vowifi && <Grid container spacing={2}><Grid size={{ xs: 12, sm: 4 }}><Field label="运行阶段" value={`${vowifi.runtime_phase} / ${vowifi.runtime_stage}`} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="IMS 注册" value={vowifi.runtime_registered ? '已注册' : '未注册'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="运行范围" value={vowifi.runtime_scope === 'primary_shared_runtime' ? '主线路实时运行时' : '仅保存线路配置'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="ePDG" value={vowifi.config.epdg_host || '运营商自动发现'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="DNS" value={vowifi.config.dns_server || '系统解析器'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="运营商 profile" value={vowifi.matched_profile_id || '尚未匹配'} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="代理模式" value={vowifi.config.proxy_mode} /></Grid><Grid size={{ xs: 12, sm: 6 }}><Field label="代理端点" value={vowifi.config.proxy_endpoint || '直连'} /></Grid>{vowifi.runtime_error && <Grid size={12}><Alert severity="warning">{vowifi.runtime_error}</Alert></Grid>}{!vowifi.is_primary && <Grid size={12}><Alert severity="info">该线路目前只完成独立配置持久化，实时 IKE/IMS 执行器仍仅接入主线路。</Alert></Grid>}</Grid>}
        {tab === 'trunk' && <Grid container spacing={2}><Grid size={{ xs: 12, sm: 4 }}><Field label="运行阶段" value={trunk ? `${trunk.runtime.phase} / ${trunk.runtime.stage}` : '未加载'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="注册状态" value={trunk?.runtime.registered ? '已注册' : '未注册'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="本地 SIP" value={trunk?.runtime.local_endpoint || '未监听'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="Asterisk Peer" value={trunk?.runtime.peer || '未解析'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="REGISTER / 重连" value={trunk ? `${trunk.runtime.register_attempts} / ${trunk.runtime.reconnect_count}` : '0 / 0'} /></Grid><Grid size={{ xs: 12, sm: 4 }}><Field label="通话 / 对话" value={trunk ? `${trunk.runtime.active_calls} / ${trunk.runtime.active_dialogs}` : '0 / 0'} /></Grid>{trunk?.runtime.last_error && <Grid size={12}><Alert severity="error">{trunk.runtime.last_error}</Alert></Grid>}</Grid>}
      </DialogContent>
    </Dialog>
  )
}
