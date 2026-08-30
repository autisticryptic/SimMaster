import { useEffect, useMemo, useState } from 'react'
import {
  Accordion,
  AccordionDetails,
  AccordionSummary,
  Alert,
  Box,
  Button,
  Chip,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
  Divider,
  FormControl,
  FormControlLabel,
  InputLabel,
  MenuItem,
  Select,
  Stack,
  Switch,
  TextField,
  Typography,
} from '@mui/material'
import { ExpandMore } from '@mui/icons-material'
import { api, type CarrierProfileRecord, type VoiceCodecPolicyRecord } from '../../api/current'

interface Props {
  open: boolean
  /** The profile being edited. Pass a derived record to create a new one. */
  record: CarrierProfileRecord | null
  onClose: () => void
  onSaved: () => void
  readOnly?: boolean
  profileIdLocked?: boolean
}

/** Multi-line text field backed by a string array, one entry per line. */
function ListField({
  label,
  value,
  helperText,
  placeholder,
  onChange,
}: {
  label: string
  value: string[]
  helperText?: string
  placeholder?: string
  onChange: (next: string[]) => void
}) {
  return (
    <TextField
      label={label}
      value={value.join('\n')}
      multiline
      minRows={2}
      maxRows={10}
      placeholder={placeholder}
      helperText={helperText ? `${helperText}（每行一条）` : '每行一条'}
      onChange={(event) =>
        onChange(
          event.target.value
            .split('\n')
            .map((line) => line.trim())
            .filter((line) => line.length > 0),
        )
      }
    />
  )
}

/** Same idea for numeric lists such as SIP status codes. */
function NumberListField({
  label,
  value,
  helperText,
  onChange,
}: {
  label: string
  value: number[]
  helperText?: string
  onChange: (next: number[]) => void
}) {
  return (
    <TextField
      label={label}
      value={value.join(', ')}
      placeholder="408, 429, 503"
      helperText={helperText ? `${helperText}（逗号分隔）` : '逗号分隔'}
      onChange={(event) =>
        onChange(
          event.target.value
            .split(',')
            .map((part) => Number(part.trim()))
            .filter((part) => Number.isInteger(part) && part > 0 && part < 700),
        )
      }
    />
  )
}

function Section({
  title,
  subtitle,
  defaultExpanded,
  children,
}: {
  title: string
  subtitle?: string
  defaultExpanded?: boolean
  children: React.ReactNode
}) {
  return (
    <Accordion defaultExpanded={defaultExpanded} disableGutters>
      <AccordionSummary expandIcon={<ExpandMore />}>
        <Box>
          <Typography variant="body2" fontWeight={700}>{title}</Typography>
          {subtitle && (
            <Typography variant="caption" color="text.secondary">{subtitle}</Typography>
          )}
        </Box>
      </AccordionSummary>
      <AccordionDetails>
        <Stack spacing={2}>{children}</Stack>
      </AccordionDetails>
    </Accordion>
  )
}

/** Mirrors the backend `validate()` so bad input is caught before the request. */
function validate(record: CarrierProfileRecord): string | null {
  const { meta, epdg, ims, ikev2, identity, voice, ut } = record
  if (!meta.profile_id.trim()) return 'Profile ID 不能为空'
  if (!/^\d{3}$/.test(meta.mcc)) return 'MCC 必须是 3 位数字'
  if (!/^\d{2,3}$/.test(meta.mnc)) return 'MNC 必须是 2 或 3 位数字'
  if (meta.mnc_len !== meta.mnc.length) return 'MNC 长度与 MNC 位数不一致'
  if (meta.plmn !== `${meta.mcc}${meta.mnc}`) return 'PLMN 必须等于 MCC + MNC'
  if (!epdg.host.trim()) return 'ePDG 主机不能为空'
  if (epdg.port < 1 || epdg.port > 65535) return 'ePDG 端口必须在 1-65535 之间'
  if (!ims.domain.trim() || !ims.realm.trim()) return 'IMS domain 与 realm 不能为空'
  if (ikev2.ike_proposals.length === 0) return 'IKE 提案不能为空'
  if (ikev2.esp_proposals.length === 0) return 'ESP 提案不能为空'
  if (meta.mcc === '999' && !ikev2.identity_template?.trim()) {
    return '私有网络（MCC 999）必须显式填写 IKE IDi 模板'
  }
  if (ikev2.identity_template) {
    const remainder = ikev2.identity_template.replace(
      /\{(?:imsi|mcc|mnc|mnc3|plmn|epdg_fqdn|ims_domain|ims_realm)\}/g,
      '',
    )
    if (/[{}]/.test(remainder)) return 'IKE IDi 模板包含不支持的占位符'
  }
  if (ims.register.expires_seconds <= 0) return 'REGISTER Expires 必须大于 0'
  for (const server of epdg.dns_servers) {
    // Accept `1.1.1.1` or `1.1.1.1:53`; IPv6 must be bracketed when a port is given.
    const bare = server.replace(/^\[(.+)\]:\d+$/, '$1').replace(/^([\d.]+):\d+$/, '$1')
    if (!/^[0-9a-f:.]+$/i.test(bare)) return `DNS 服务器格式不正确：${server}`
  }
  if (identity.device_identity_imei && !/^\d{15}$/.test(identity.device_identity_imei.trim())) {
    return 'IMEI 必须是 15 位数字'
  }
  if (ims.register.include_visited_network && !ims.register.visited_network_header?.trim()) {
    return '启用 Visited-Network 后必须填写该头的值'
  }
  if (
    (ims.register.sec_agree_mode === 'required' ||
      ims.register.require_sec_agree_headers ||
      ims.register.proxy_require_sec_agree_headers) &&
    ims.register.security_client_mechanisms.length === 0
  ) {
    return '强制 sec-agree 时必须填写 Security-Client 机制'
  }
  if (ims.register.security_client_mechanisms.some((item) => item.split('/').length !== 4)) {
    return 'Security-Client 机制必须是 4 段，如 hmac-sha-1-96/aes-cbc/esp/trans'
  }
  if (!ims.user_agent.trim()) return 'User-Agent 不能为空'
  if (
    (ims.register.request_uri_policy === 'registrar' ||
      ims.register.request_uri_policy === 'configured') &&
    !ims.registrar?.trim()
  ) {
    return '该 Request-URI 策略必须填写 registrar'
  }
  if (
    (ims.register.include_pani_initial || ims.register.include_pani_authenticated) &&
    ims.register.pani_identity_policy !== 'omit' &&
    !ims.register.access_network_info.trim()
  ) {
    return '携带 PANI 时必须填写接入网类型'
  }
  if (
    ims.register.enable_cellular_network_info &&
    ims.register.cni_identity_policy === 'static' &&
    !ims.register.cellular_network_info?.trim()
  ) {
    return 'CNI 使用 static 策略时必须填写静态蜂窝网络信息'
  }
  if (voice.preferred_codecs.length === 0) return '语音编解码优先级不能为空'
  if (ut.enabled && !ut.xcap_root?.trim()) return '启用补充业务后必须填写 XCAP root'
  return null
}

export default function CarrierProfileEditor({
  open,
  record,
  onClose,
  onSaved,
  readOnly = false,
  profileIdLocked = false,
}: Props) {
  const [draft, setDraft] = useState<CarrierProfileRecord | null>(null)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  // Re-seed the draft whenever a different profile is opened. Cloning keeps the
  // caller's record untouched if the user cancels.
  useEffect(() => {
    if (open && record) {
      setDraft(structuredClone(record))
      setError(null)
    } else if (!open) {
      setDraft(null)
    }
  }, [open, record])

  const validationError = useMemo(() => (draft ? validate(draft) : null), [draft])

  if (!draft) return null

  /** Update one nested section without losing the rest of the document. */
  const patch = <K extends keyof CarrierProfileRecord>(
    section: K,
    changes: Partial<CarrierProfileRecord[K]>,
  ) => {
    setDraft((current) =>
      current ? { ...current, [section]: { ...current[section], ...changes } } : current,
    )
  }

  const patchRegister = (changes: Partial<CarrierProfileRecord['ims']['register']>) => {
    setDraft((current) =>
      current
        ? { ...current, ims: { ...current.ims, register: { ...current.ims.register, ...changes } } }
        : current,
    )
  }

  const save = async () => {
    if (validationError) return
    setSaving(true)
    setError(null)
    try {
      await api.saveVowifiCarrierProfile(draft)
      onSaved()
      onClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <Dialog open={open} onClose={saving ? undefined : onClose} fullWidth maxWidth="md">
      <DialogTitle>
        运营商 profile · {draft.meta.brand || draft.meta.profile_id}
        <Chip size="small" label={draft.meta.plmn} sx={{ ml: 1 }} />
      </DialogTitle>
      <DialogContent
        dividers
        sx={readOnly ? {
          '& .MuiInputBase-root, & .MuiSwitch-root': { pointerEvents: 'none' },
          '& input, & textarea': { cursor: 'default' },
        } : undefined}
      >
        <Stack spacing={1.5}>
          <Alert severity="info">
            {readOnly
              ? '当前内容来自 sealed carrier_Bundles 数据库，仅供查看。'
              : 'ePDG 主机名和 IMS 域名按 3GPP TS 23.003 从 MCC/MNC 推导，通常不需要手改。真正因运营商而异的是下面的 REGISTER 细节，这些只能靠实测确定。'}
          </Alert>

          <Section title="基本信息" subtitle="标识与归属" defaultExpanded>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="Profile ID"
                value={draft.meta.profile_id}
                helperText={profileIdLocked ? '已存在记录的主键不可修改' : '唯一标识，保存时作为主键'}
                slotProps={{ input: { readOnly: profileIdLocked } }}
                onChange={(e) => patch('meta', { profile_id: e.target.value })}
              />
              <TextField
                fullWidth
                label="运营商名称"
                value={draft.meta.brand}
                onChange={(e) => patch('meta', { brand: e.target.value })}
              />
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                label="MCC"
                value={draft.meta.mcc}
                onChange={(e) => {
                  const mcc = e.target.value.replace(/\D/g, '').slice(0, 3)
                  patch('meta', { mcc, plmn: `${mcc}${draft.meta.mnc}` })
                }}
              />
              <TextField
                label="MNC"
                value={draft.meta.mnc}
                onChange={(e) => {
                  const mnc = e.target.value.replace(/\D/g, '').slice(0, 3)
                  patch('meta', {
                    mnc,
                    mnc_len: mnc.length,
                    plmn: `${draft.meta.mcc}${mnc}`,
                  })
                }}
              />
              <TextField
                label="PLMN"
                value={draft.meta.plmn}
                slotProps={{ input: { readOnly: true } }}
                helperText="由 MCC + MNC 自动生成"
              />
              <TextField
                label="国家代码"
                value={draft.meta.country_iso2}
                onChange={(e) => patch('meta', { country_iso2: e.target.value })}
              />
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="法定名称"
                value={draft.meta.operator_legal_name}
                onChange={(e) => patch('meta', { operator_legal_name: e.target.value })}
              />
              <TextField
                fullWidth
                label="最后核验时间"
                value={draft.meta.last_verified}
                placeholder="2026-08-12"
                helperText="记录这份配置最后一次实测通过的时间"
                onChange={(e) => patch('meta', { last_verified: e.target.value })}
              />
            </Stack>
            <ListField
              label="别名"
              value={draft.meta.aliases}
              helperText="SPN / 品牌别名，用于匹配 MVNO"
              onChange={(aliases) => patch('meta', { aliases })}
            />
            <ListField
              label="来源引用"
              value={draft.meta.source_refs}
              helperText="配置依据，如固件包名或标准条款"
              onChange={(source_refs) => patch('meta', { source_refs })}
            />
          </Section>

          <Section title="ePDG 通道" subtitle="IKE 入口、APN、IP 协议栈与 DNS" defaultExpanded>
            <TextField
              label="ePDG 主机"
              value={draft.epdg.host}
              onChange={(e) => patch('epdg', { host: e.target.value })}
              helperText="标准格式：epdg.epc.mncXXX.mccYYY.pub.3gppnetwork.org"
            />
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                type="number"
                label="IKE 端口"
                value={draft.epdg.port}
                slotProps={{ htmlInput: { min: 1, max: 65535 } }}
                onChange={(e) => patch('epdg', { port: Number(e.target.value) })}
              />
              <TextField
                fullWidth
                label="APN"
                value={draft.epdg.apn ?? ''}
                placeholder="ims"
                onChange={(e) => patch('epdg', { apn: e.target.value || null })}
              />
              <FormControl fullWidth>
                <InputLabel>IP 协议栈</InputLabel>
                <Select
                  label="IP 协议栈"
                  value={draft.epdg.ip_stack}
                  onChange={(e) =>
                    patch('epdg', { ip_stack: e.target.value })
                  }
                >
                  <MenuItem value="ipv4v6">IPv4 + IPv6</MenuItem>
                  <MenuItem value="ipv4">仅 IPv4</MenuItem>
                  <MenuItem value="ipv6">仅 IPv6</MenuItem>
                </Select>
              </FormControl>
            </Stack>
            <ListField
              label="DNS 服务器"
              value={draft.epdg.dns_servers}
              placeholder={'8.8.8.8\n1.1.1.1:53'}
              helperText="按顺序尝试，前面的解析不出来自动换下一个；不写端口默认 :53。ePDG 域名解析不出来就完全连不上，所以建议填两个以上"
              onChange={(dns_servers) => patch('epdg', { dns_servers })}
            />
          </Section>

          <Section title="IKEv2 / 鉴权" subtitle="加密提案、保活与设备身份">
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                type="number"
                label="NAT 保活（秒）"
                value={draft.ikev2.nat_keepalive_seconds}
                onChange={(e) =>
                  patch('ikev2', { nat_keepalive_seconds: Number(e.target.value) })
                }
              />
              <TextField
                type="number"
                label="DPD 间隔（秒）"
                value={draft.ikev2.dpd_interval_seconds}
                onChange={(e) =>
                  patch('ikev2', { dpd_interval_seconds: Number(e.target.value) })
                }
              />
              <TextField
                type="number"
                label="重认证间隔（秒）"
                value={draft.ikev2.reauth_interval_seconds ?? ''}
                helperText="留空表示不重认证"
                onChange={(e) =>
                  patch('ikev2', {
                    reauth_interval_seconds: e.target.value ? Number(e.target.value) : null,
                  })
                }
              />
            </Stack>
            <TextField
              fullWidth
              label="IKE IDi 模板"
              value={draft.ikev2.identity_template ?? ''}
              placeholder="0{imsi}@nai.epc.mnc{mnc3}.mcc{mcc}.3gppnetwork.org"
              helperText="公开 PLMN 留空时使用 3GPP permanent NAI；MCC 999 必填。可用 {imsi}、{mcc}、{mnc}、{mnc3}、{plmn}、{epdg_fqdn}、{ims_domain}、{ims_realm}，值会结合当前线路 SIM/基带身份展开"
              onChange={(e) =>
                patch('ikev2', { identity_template: e.target.value.trim() || null })
              }
            />
            <ListField
              label="IKE 提案"
              value={draft.ikev2.ike_proposals}
              helperText="按优先级排序，整体替换而非合并"
              onChange={(ike_proposals) => patch('ikev2', { ike_proposals })}
            />
            <ListField
              label="ESP 提案"
              value={draft.ikev2.esp_proposals}
              onChange={(esp_proposals) => patch('ikev2', { esp_proposals })}
            />
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <FormControl fullWidth>
                <InputLabel>AKA 挑战模式</InputLabel>
                <Select
                  label="AKA 挑战模式"
                  value={draft.ikev2.aka_challenge_mode}
                  onChange={(e) => patch('ikev2', { aka_challenge_mode: e.target.value })}
                >
                  <MenuItem value="standard">standard（标准）</MenuItem>
                  <MenuItem value="minimal">minimal</MenuItem>
                  <MenuItem value="omit">omit</MenuItem>
                  <MenuItem value="checkcode">checkcode</MenuItem>
                  <MenuItem value="recompute">recompute</MenuItem>
                </Select>
              </FormControl>
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ikev2.include_epdg_idr}
                    onChange={(_, include_epdg_idr) => patch('ikev2', { include_epdg_idr })}
                  />
                }
                label="携带 ePDG IDr"
              />
            </Stack>
            <Divider />
            <Typography variant="caption" color="text.secondary">
              设备身份：部分运营商在 IKE_AUTH 阶段校验机型/IMEI，不匹配会直接拒绝
            </Typography>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="设备机型"
                value={draft.identity.device_model_hint}
                placeholder="iphone15,4"
                onChange={(e) => patch('identity', { device_model_hint: e.target.value })}
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.identity.device_identity_enabled}
                    onChange={(_, device_identity_enabled) =>
                      patch('identity', { device_identity_enabled })
                    }
                  />
                }
                label="发送设备身份"
              />
            </Stack>
            <TextField
              label="自定义 IMEI"
              value={draft.identity.device_identity_imei ?? ''}
              placeholder="351234567890123"
              helperText="留空则使用基带自身的 IMEI；填写时必须是 15 位数字"
              onChange={(e) =>
                patch('identity', { device_identity_imei: e.target.value || null })
              }
            />
          </Section>

          <Section title="IMS / SIP" subtitle="域名、传输与会话保活">
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="IMS domain"
                value={draft.ims.domain}
                onChange={(e) => patch('ims', { domain: e.target.value })}
              />
              <TextField
                fullWidth
                label="IMS realm"
                value={draft.ims.realm}
                onChange={(e) => patch('ims', { realm: e.target.value })}
              />
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <FormControl fullWidth>
                <InputLabel>传输方式</InputLabel>
                <Select
                  label="传输方式"
                  value={draft.ims.transport}
                  onChange={(e) => patch('ims', { transport: e.target.value })}
                >
                  <MenuItem value="tcp">TCP</MenuItem>
                  <MenuItem value="udp">UDP</MenuItem>
                </Select>
              </FormControl>
              <TextField
                type="number"
                label="本地端口"
                value={draft.ims.local_port}
                onChange={(e) => patch('ims', { local_port: Number(e.target.value) })}
              />
              <FormControl fullWidth>
                <InputLabel>身份来源</InputLabel>
                <Select
                  label="身份来源"
                  value={draft.ims.identity_source}
                  onChange={(e) => patch('ims', { identity_source: e.target.value })}
                >
                  <MenuItem value="isim">ISIM</MenuItem>
                  <MenuItem value="derived">由 IMSI 推导</MenuItem>
                  <MenuItem value="carrier_device_model">运营商机型规则</MenuItem>
                </Select>
              </FormControl>
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="Registrar"
                value={draft.ims.registrar ?? ''}
                placeholder="留空则使用 IMS domain"
                helperText="Request-URI 策略为 registrar / configured 时必填"
                onChange={(e) => patch('ims', { registrar: e.target.value || null })}
              />
              <TextField
                fullWidth
                label="User-Agent"
                value={draft.ims.user_agent}
                placeholder="SimAdmin-IMS/1.0"
                onChange={(e) => patch('ims', { user_agent: e.target.value })}
              />
            </Stack>
            <TextField
              label="P-CSCF 覆盖"
              value={draft.ims.pcscf ?? ''}
              placeholder="留空（推荐）"
              helperText="留空时从 IKEv2 CFG_REPLY 自动获取，这是正常路径；只有在网络不下发时才需要手填"
              onChange={(e) => patch('ims', { pcscf: e.target.value || null })}
            />
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                type="number"
                label="TCP 保活（秒）"
                value={draft.ims.tcp_keepalive_seconds}
                helperText="0 表示关闭。不保活时 NAT 超时会让注册悄悄掉线"
                onChange={(e) => patch('ims', { tcp_keepalive_seconds: Number(e.target.value) })}
              />
              <TextField
                fullWidth
                type="number"
                label="OPTIONS 心跳（秒）"
                value={draft.ims.options_ping_interval_seconds}
                helperText="0 表示关闭"
                onChange={(e) =>
                  patch('ims', { options_ping_interval_seconds: Number(e.target.value) })
                }
              />
            </Stack>
          </Section>

          <Section
            title="REGISTER 模板"
            subtitle="运营商差异最大的部分，注册失败优先调这里"
          >
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <FormControl fullWidth>
                <InputLabel>Request-URI 策略</InputLabel>
                <Select
                  label="Request-URI 策略"
                  value={draft.ims.register.request_uri_policy}
                  onChange={(e) => patchRegister({ request_uri_policy: e.target.value })}
                >
                  <MenuItem value="registrar">registrar</MenuItem>
                  <MenuItem value="home_domain">home_domain</MenuItem>
                  <MenuItem value="pcscf">pcscf</MenuItem>
                  <MenuItem value="configured">configured</MenuItem>
                </Select>
              </FormControl>
              <FormControl fullWidth>
                <InputLabel>初始 Authorization</InputLabel>
                <Select
                  label="初始 Authorization"
                  value={draft.ims.register.initial_authorization}
                  onChange={(e) => patchRegister({ initial_authorization: e.target.value })}
                >
                  <MenuItem value="none">none（不带）</MenuItem>
                  <MenuItem value="aka_empty">aka_empty</MenuItem>
                  <MenuItem value="digest_empty">digest_empty</MenuItem>
                  <MenuItem value="implementation_variant">implementation_variant</MenuItem>
                </Select>
              </FormControl>
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="Header 变体集"
                value={draft.ims.register.live_header_variant_set}
                placeholder="default"
                helperText="实测出的头部组合命名，用于切换整套 REGISTER 变体"
                onChange={(e) => patchRegister({ live_header_variant_set: e.target.value })}
              />
              <TextField
                fullWidth
                label="Allow 方法"
                value={draft.ims.register.allow_methods ?? ''}
                placeholder="INVITE, ACK, CANCEL, BYE, OPTIONS"
                helperText="留空则不发送 Allow 头"
                onChange={(e) => patchRegister({ allow_methods: e.target.value || null })}
              />
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <FormControl fullWidth>
                <InputLabel>sec-agree 模式</InputLabel>
                <Select
                  label="sec-agree 模式"
                  value={draft.ims.register.sec_agree_mode}
                  onChange={(e) =>
                    patchRegister({
                      sec_agree_mode: e.target
                        .value,
                    })
                  }
                >
                  <MenuItem value="auto">auto（跟随挑战）</MenuItem>
                  <MenuItem value="required">required（始终携带）</MenuItem>
                  <MenuItem value="disabled">disabled（从不携带）</MenuItem>
                </Select>
              </FormControl>
              <TextField
                fullWidth
                type="number"
                label="Expires（秒）"
                value={draft.ims.register.expires_seconds}
                helperText="部分运营商不接受默认的 3600"
                onChange={(e) => patchRegister({ expires_seconds: Number(e.target.value) })}
              />
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="PANI 静态接入网信息"
                value={draft.ims.register.access_network_info}
                disabled={draft.ims.register.pani_identity_policy === 'omit'}
                placeholder="IEEE-802.11 或 3GPP-E-UTRAN-FDD"
                helperText="static 只使用此值；dynamic_if_known 使用实时接入信息，缺失时回退此值；required_dynamic 缺失实时信息时注册失败"
                onChange={(e) => patchRegister({ access_network_info: e.target.value })}
              />
              <FormControl fullWidth>
                <InputLabel>PANI 身份策略</InputLabel>
                <Select
                  label="PANI 身份策略"
                  value={draft.ims.register.pani_identity_policy}
                  onChange={(e) => patchRegister({ pani_identity_policy: e.target.value })}
                >
                  <MenuItem value="omit">omit（不发送）</MenuItem>
                  <MenuItem value="static">static（只用 Profile 静态值）</MenuItem>
                  <MenuItem value="dynamic_if_known">dynamic_if_known（动态优先、静态兜底）</MenuItem>
                  <MenuItem value="required_dynamic">required_dynamic（无实时信息则失败）</MenuItem>
                </Select>
              </FormControl>
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="CNI 静态蜂窝网络信息"
                value={draft.ims.register.cellular_network_info ?? ''}
                disabled={
                  !draft.ims.register.enable_cellular_network_info ||
                  draft.ims.register.cni_identity_policy === 'omit'
                }
                placeholder="3GPP-E-UTRAN-FDD;utran-cell-id-3gpp=..."
                helperText="VoWiFi 的蜂窝 CNI 与 WLAN PANI 独立配置；static 需要填写此值"
                onChange={(e) =>
                  patchRegister({ cellular_network_info: e.target.value || null })
                }
              />
              <FormControl fullWidth disabled={!draft.ims.register.enable_cellular_network_info}>
                <InputLabel>CNI 身份策略</InputLabel>
                <Select
                  label="CNI 身份策略"
                  value={draft.ims.register.cni_identity_policy}
                  onChange={(e) => patchRegister({ cni_identity_policy: e.target.value })}
                >
                  <MenuItem value="omit">omit（不发送）</MenuItem>
                  <MenuItem value="static">static（只用 Profile 静态值）</MenuItem>
                  <MenuItem value="dynamic_if_known">dynamic_if_known（动态优先、静态兜底）</MenuItem>
                  <MenuItem value="required_dynamic">required_dynamic（无实时信息则失败）</MenuItem>
                </Select>
              </FormControl>
            </Stack>
            <FormControl fullWidth>
              <InputLabel>Contact 格式</InputLabel>
              <Select
                label="Contact 格式"
                value={draft.ims.register.contact_mode}
                onChange={(e) =>
                  patchRegister({
                    contact_mode: e.target.value,
                  })
                }
              >
                <MenuItem value="android_default">android_default</MenuItem>
                <MenuItem value="standard">standard</MenuItem>
                <MenuItem value="legacy">legacy</MenuItem>
                <MenuItem value="custom">custom</MenuItem>
              </Select>
            </FormControl>
            <TextField
              label="Supported 头"
              value={draft.ims.register.supported_header}
              placeholder="path,sec-agree,gruu"
              onChange={(e) => patchRegister({ supported_header: e.target.value })}
            />
            <ListField
              label="Contact 参数顺序"
              value={draft.ims.register.contact_param_order}
              helperText="留空使用所选 Contact 格式的内置顺序"
              onChange={(contact_param_order) => patchRegister({ contact_param_order })}
            />
            <ListField
              label="Security-Client 机制"
              value={draft.ims.register.security_client_mechanisms}
              placeholder="hmac-sha-1-96/aes-cbc/esp/trans"
              onChange={(security_client_mechanisms) =>
                patchRegister({ security_client_mechanisms })
              }
            />
            <TextField
              label="Visited-Network 头"
              value={draft.ims.register.visited_network_header ?? ''}
              disabled={!draft.ims.register.include_visited_network}
              placeholder="ims.mnc001.mcc460.3gppnetwork.org"
              helperText="启用 Visited-Network 开关后必填"
              onChange={(e) => patchRegister({ visited_network_header: e.target.value || null })}
            />
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2} flexWrap="wrap" useFlexGap>
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.include_pani_initial}
                    onChange={(_, include_pani_initial) =>
                      patchRegister({ include_pani_initial })
                    }
                  />
                }
                label="初始 REGISTER 带 PANI"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.include_mmtel_features}
                    onChange={(_, include_mmtel_features) =>
                      patchRegister({ include_mmtel_features })
                    }
                  />
                }
                label="携带 MMTEL 特性标签"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.include_route_header}
                    onChange={(_, include_route_header) =>
                      patchRegister({ include_route_header })
                    }
                  />
                }
                label="携带 Route 头"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.include_visited_network}
                    onChange={(_, include_visited_network) =>
                      patchRegister({ include_visited_network })
                    }
                  />
                }
                label="携带 Visited-Network"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.include_p_preferred_identity}
                    onChange={(_, include_p_preferred_identity) =>
                      patchRegister({ include_p_preferred_identity })
                    }
                  />
                }
                label="携带 P-Preferred-Identity"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.require_sec_agree_headers}
                    onChange={(_, require_sec_agree_headers) =>
                      patchRegister({ require_sec_agree_headers })
                    }
                  />
                }
                label="Require: sec-agree"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.proxy_require_sec_agree_headers}
                    onChange={(_, proxy_require_sec_agree_headers) =>
                      patchRegister({ proxy_require_sec_agree_headers })
                    }
                  />
                }
                label="Proxy-Require: sec-agree"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.use_plain_digest_placeholder}
                    onChange={(_, use_plain_digest_placeholder) =>
                      patchRegister({ use_plain_digest_placeholder })
                    }
                  />
                }
                label="使用明文 Digest 占位"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.always_add_sip_instance}
                    onChange={(_, always_add_sip_instance) =>
                      patchRegister({ always_add_sip_instance })
                    }
                  />
                }
                label="总是携带 +sip.instance"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.enable_cellular_network_info}
                    onChange={(_, enable_cellular_network_info) =>
                      patchRegister({ enable_cellular_network_info })
                    }
                  />
                }
                label="附加蜂窝网络信息"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.include_pani_authenticated}
                    onChange={(_, include_pani_authenticated) =>
                      patchRegister({ include_pani_authenticated })
                    }
                  />
                }
                label="PANI 带 network-provided"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.strict_security_server_offer}
                    onChange={(_, strict_security_server_offer) =>
                      patchRegister({ strict_security_server_offer })
                    }
                  />
                }
                label="严格校验 Security-Server"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ims.register.enable_initial_reject_fallback}
                    onChange={(_, enable_initial_reject_fallback) =>
                      patchRegister({ enable_initial_reject_fallback })
                    }
                  />
                }
                label="首次被拒时回退重试"
              />
            </Stack>
            <Divider />
            <Typography variant="caption" color="text.secondary">
              重试策略：决定收到某个 SIP 状态码后是重试、放弃还是走回退流程
            </Typography>
            <NumberListField
              label="可重试状态码"
              value={draft.ims.register.temporary_status_codes}
              helperText="网络忙/暂时不可用，稍后重试"
              onChange={(temporary_status_codes) => patchRegister({ temporary_status_codes })}
            />
            <NumberListField
              label="永久拒绝状态码"
              value={draft.ims.register.forbidden_status_codes}
              helperText="收到即停止，不再重试"
              onChange={(forbidden_status_codes) => patchRegister({ forbidden_status_codes })}
            />
            <NumberListField
              label="触发回退的状态码"
              value={draft.ims.register.initial_reject_fallback_status_codes}
              onChange={(initial_reject_fallback_status_codes) =>
                patchRegister({ initial_reject_fallback_status_codes })
              }
            />
            <TextField
              type="number"
              label="重试间隔（秒）"
              value={draft.ims.register.temporary_retry_seconds}
              onChange={(e) =>
                patchRegister({ temporary_retry_seconds: Number(e.target.value) })
              }
            />
          </Section>

          <Section title="语音" subtitle="通话链路与编解码偏好">
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2} flexWrap="wrap" useFlexGap>
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.voice.vowifi_enabled}
                    onChange={(_, vowifi_enabled) => patch('voice', { vowifi_enabled })}
                  />
                }
                label="启用 VoWiFi 语音"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.voice.carrier_fallback_enabled}
                    onChange={(_, carrier_fallback_enabled) =>
                      patch('voice', { carrier_fallback_enabled })
                    }
                  />
                }
                label="允许运营商链路回退"
              />
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.voice.amr_octet_align}
                    onChange={(_, amr_octet_align) => patch('voice', { amr_octet_align })}
                  />
                }
                label="AMR octet-align"
              />
            </Stack>
            <ListField
              label="编解码优先级"
              value={draft.voice.preferred_codecs}
              placeholder={'amr-wb\namr\npcmu'}
              onChange={(preferred_codecs) => patch('voice', { preferred_codecs })}
            />
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                type="number"
                label="ptime（毫秒）"
                value={draft.voice.ptime_ms}
                onChange={(e) => patch('voice', { ptime_ms: Number(e.target.value) })}
              />
              <TextField
                fullWidth
                label="语音信箱号码"
                value={draft.voice.voicemail_number ?? ''}
                placeholder="+8613800000000"
                onChange={(e) => patch('voice', { voicemail_number: e.target.value || null })}
              />
            </Stack>
            <FormControlLabel
              control={
                <Switch
                  checked={draft.voice.sip_endpoint_exposed}
                  onChange={(_, sip_endpoint_exposed) =>
                    patch('voice', { sip_endpoint_exposed })
                  }
                />
              }
              label="对外暴露 SIP 端点"
            />
            <Divider />
            <Box display="flex" justifyContent="space-between" alignItems="center" gap={1}>
              <Typography variant="caption" color="text.secondary">
                编解码细节：payload type 与 fmtp 不匹配会导致接通后单通
              </Typography>
              <Button
                size="small"
                onClick={() =>
                  patch('voice', {
                    codec_policies: [
                      ...draft.voice.codec_policies,
                      { codec: draft.voice.preferred_codecs[0] ?? 'amr-wb', payload_type: null, sample_rate: null, fmtp: null },
                    ],
                  })
                }
              >
                添加编解码
              </Button>
            </Box>
            {draft.voice.codec_policies.length === 0 && (
              <Typography variant="caption" color="text.secondary">
                未配置时按编解码优先级使用内置默认参数
              </Typography>
            )}
            {draft.voice.codec_policies.map((policy, index) => {
              const patchCodec = (changes: Partial<VoiceCodecPolicyRecord>) =>
                patch('voice', {
                  codec_policies: draft.voice.codec_policies.map((item, itemIndex) =>
                    itemIndex === index ? { ...item, ...changes } : item,
                  ),
                })
              return (
                <Stack
                  key={index}
                  direction={{ xs: 'column', sm: 'row' }}
                  spacing={1.5}
                  alignItems={{ sm: 'center' }}
                >
                  <TextField
                    label="编解码"
                    value={policy.codec}
                    placeholder="amr-wb"
                    onChange={(e) => patchCodec({ codec: e.target.value })}
                  />
                  <TextField
                    type="number"
                    label="Payload type"
                    value={policy.payload_type ?? ''}
                    slotProps={{ htmlInput: { min: 96, max: 127 } }}
                    onChange={(e) =>
                      patchCodec({ payload_type: e.target.value ? Number(e.target.value) : null })
                    }
                  />
                  <TextField
                    type="number"
                    label="采样率"
                    value={policy.sample_rate ?? ''}
                    placeholder="16000"
                    onChange={(e) =>
                      patchCodec({ sample_rate: e.target.value ? Number(e.target.value) : null })
                    }
                  />
                  <TextField
                    fullWidth
                    label="fmtp"
                    value={policy.fmtp ?? ''}
                    placeholder="mode-set=0,1,2; octet-align=1"
                    onChange={(e) => patchCodec({ fmtp: e.target.value || null })}
                  />
                  <Button
                    size="small"
                    color="error"
                    onClick={() =>
                      patch('voice', {
                        codec_policies: draft.voice.codec_policies.filter(
                          (_, itemIndex) => itemIndex !== index,
                        ),
                      })
                    }
                  >
                    删除
                  </Button>
                </Stack>
              )
            })}
          </Section>

          <Section title="短信 (SMSoIP)" subtitle="IMS 短信通道与 SMSC 鉴权">
            <FormControl fullWidth>
              <InputLabel>短信接收通道</InputLabel>
              <Select
                label="短信接收通道"
                value={draft.sms.receiver_transport}
                onChange={(e) => patch('sms', { receiver_transport: e.target.value })}
              >
                <MenuItem value="sip">SIP（IMS 短信）</MenuItem>
                <MenuItem value="cellular">蜂窝（CS/PS 短信）</MenuItem>
                <MenuItem value="none">不接收</MenuItem>
              </Select>
            </FormControl>
            <FormControlLabel
              control={
                <Switch
                  checked={draft.sms.smsc_auth_required}
                  onChange={(_, smsc_auth_required) => patch('sms', { smsc_auth_required })}
                />
              }
              label="SMSC 需要鉴权"
            />
          </Section>

          <Section title="补充业务 (Ut / XCAP)" subtitle="呼叫等待、呼叫转移与主叫显示的配置通道">
            <FormControlLabel
              control={
                <Switch
                  checked={draft.ut.enabled}
                  onChange={(_, enabled) => patch('ut', { enabled })}
                />
              }
              label="启用 Ut / XCAP"
            />
            <TextField
              label="XCAP root"
              value={draft.ut.xcap_root ?? ''}
              disabled={!draft.ut.enabled}
              placeholder="https://xcap.ims.mnc001.mcc460.pub.3gppnetwork.org"
              onChange={(e) => patch('ut', { xcap_root: e.target.value || null })}
            />
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="Document selector"
                value={draft.ut.document_selector ?? ''}
                disabled={!draft.ut.enabled}
                placeholder="simservs.ngn.etsi.org/users/sip:{impu}/simservs.xml"
                onChange={(e) => patch('ut', { document_selector: e.target.value || null })}
              />
              <TextField
                fullWidth
                label="XML namespace"
                value={draft.ut.namespace ?? ''}
                disabled={!draft.ut.enabled}
                onChange={(e) => patch('ut', { namespace: e.target.value || null })}
              />
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <FormControl fullWidth disabled={!draft.ut.enabled}>
                <InputLabel>鉴权方式</InputLabel>
                <Select
                  label="鉴权方式"
                  value={draft.ut.authentication}
                  onChange={(e) => patch('ut', { authentication: e.target.value })}
                >
                  <MenuItem value="none">none</MenuItem>
                  <MenuItem value="digest">digest</MenuItem>
                  <MenuItem value="aka">aka</MenuItem>
                  <MenuItem value="gba">gba</MenuItem>
                </Select>
              </FormControl>
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ut.partial_update}
                    disabled={!draft.ut.enabled}
                    onChange={(_, partial_update) => patch('ut', { partial_update })}
                  />
                }
                label="使用局部更新"
              />
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="呼叫等待 selector"
                value={draft.ut.call_waiting_selector ?? ''}
                disabled={!draft.ut.enabled || !draft.ut.partial_update}
                onChange={(e) => patch('ut', { call_waiting_selector: e.target.value || null })}
              />
              <TextField
                fullWidth
                label="呼叫转移 selector"
                value={draft.ut.diversion_rule_selector ?? ''}
                disabled={!draft.ut.enabled || !draft.ut.partial_update}
                onChange={(e) =>
                  patch('ut', { diversion_rule_selector: e.target.value || null })
                }
              />
            </Stack>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="主叫显示 (OIP) selector"
                value={draft.ut.oip_selector ?? ''}
                disabled={!draft.ut.enabled || !draft.ut.partial_update}
                onChange={(e) => patch('ut', { oip_selector: e.target.value || null })}
              />
              <TextField
                fullWidth
                label="主叫隐藏 (OIR) selector"
                value={draft.ut.oir_selector ?? ''}
                disabled={!draft.ut.enabled || !draft.ut.partial_update}
                onChange={(e) => patch('ut', { oir_selector: e.target.value || null })}
              />
            </Stack>
            <Divider />
            <Typography variant="caption" color="text.secondary">
              TLS：XCAP 走 HTTPS，运营商网关对版本和根证书有要求
            </Typography>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <FormControl fullWidth disabled={!draft.ut.enabled}>
                <InputLabel>TLS 最低版本</InputLabel>
                <Select
                  label="TLS 最低版本"
                  value={draft.ut.tls_min_version}
                  onChange={(e) => patch('ut', { tls_min_version: e.target.value })}
                >
                  <MenuItem value="1.2">1.2</MenuItem>
                  <MenuItem value="1.3">1.3</MenuItem>
                </Select>
              </FormControl>
              <FormControl fullWidth disabled={!draft.ut.enabled}>
                <InputLabel>TLS 最高版本</InputLabel>
                <Select
                  label="TLS 最高版本"
                  value={draft.ut.tls_max_version}
                  onChange={(e) => patch('ut', { tls_max_version: e.target.value })}
                >
                  <MenuItem value="1.2">1.2</MenuItem>
                  <MenuItem value="1.3">1.3</MenuItem>
                </Select>
              </FormControl>
              <FormControlLabel
                control={
                  <Switch
                    checked={draft.ut.tls_builtin_roots}
                    disabled={!draft.ut.enabled}
                    onChange={(_, tls_builtin_roots) => patch('ut', { tls_builtin_roots })}
                  />
                }
                label="使用内置根证书"
              />
            </Stack>
            <TextField
              label="附加 CA 证书 (PEM)"
              value={draft.ut.tls_additional_ca_pem ?? ''}
              disabled={!draft.ut.enabled}
              multiline
              minRows={2}
              maxRows={8}
              helperText="运营商使用私有 CA 时填写"
              onChange={(e) => patch('ut', { tls_additional_ca_pem: e.target.value || null })}
            />
          </Section>

          {!readOnly && validationError && <Alert severity="error">{validationError}</Alert>}
          {error && <Alert severity="error">{error}</Alert>}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>{readOnly ? '关闭' : '取消'}</Button>
        {!readOnly && <Button
          variant="contained"
          onClick={() => void save()}
          disabled={saving || Boolean(validationError)}
        >
          {saving ? '保存中…' : '保存 profile'}
        </Button>}
      </DialogActions>
    </Dialog>
  )
}
