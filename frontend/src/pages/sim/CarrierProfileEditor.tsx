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
import { api, type CarrierProfileRecord } from '../../api/current'

interface Props {
  open: boolean
  /** The profile being edited. Pass a derived record to create a new one. */
  record: CarrierProfileRecord | null
  /** Shown as a hint when the carrier's country expects emergency setup. */
  e911Expected?: boolean
  onClose: () => void
  onSaved: () => void
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
  const { meta, epdg, ims, ikev2, identity, e911 } = record
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
  if (ims.register.expires_seconds <= 0) return 'REGISTER Expires 必须大于 0'
  for (const server of epdg.dns_servers) {
    // Accept `1.1.1.1` or `1.1.1.1:53`; IPv6 must be bracketed when a port is given.
    const bare = server.replace(/^\[(.+)\]:\d+$/, '$1').replace(/^([\d.]+):\d+$/, '$1')
    if (!/^[0-9a-f:.]+$/i.test(bare)) return `DNS 服务器格式不正确：${server}`
  }
  if (identity.device_identity_imei && !/^\d{15}$/.test(identity.device_identity_imei.trim())) {
    return 'IMEI 必须是 15 位数字'
  }
  if (e911.enabled && !e911.websheet_host_policy?.trim()) {
    return '启用紧急呼叫后必须填写 websheet host policy'
  }
  return null
}

export default function CarrierProfileEditor({
  open,
  record,
  e911Expected,
  onClose,
  onSaved,
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
      <DialogContent dividers>
        <Stack spacing={1.5}>
          <Alert severity="info">
            ePDG 主机名和 IMS 域名按 3GPP TS 23.003 从 MCC/MNC 推导，通常不需要手改。
            真正因运营商而异的是下面的 REGISTER 细节，这些只能靠实测确定。
          </Alert>

          <Section title="基本信息" subtitle="标识与归属" defaultExpanded>
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="Profile ID"
                value={draft.meta.profile_id}
                helperText="唯一标识，保存时作为主键"
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
                label="接入网类型"
                value={draft.ims.register.access_network_info}
                placeholder="IEEE-802.11"
                helperText="写入 P-Access-Network-Info，校验此头的运营商会拒绝错误值"
                onChange={(e) => patchRegister({ access_network_info: e.target.value })}
              />
              <FormControl fullWidth>
                <InputLabel>Contact 格式</InputLabel>
                <Select
                  label="Contact 格式"
                  value={draft.ims.register.contact_mode}
                  onChange={(e) =>
                    patchRegister({
                      contact_mode: e.target
                        .value,
                    })
                  }
                >
                  <MenuItem value="android_default">android_default</MenuItem>
                  <MenuItem value="legacy">legacy</MenuItem>
                </Select>
              </FormControl>
            </Stack>
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
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2} flexWrap="wrap" useFlexGap>
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
            <TextField
              type="number"
              label="ptime（毫秒）"
              value={draft.voice.ptime_ms}
              onChange={(e) => patch('voice', { ptime_ms: Number(e.target.value) })}
            />
          </Section>

          <Section
            title="紧急呼叫 / E911"
            subtitle={
              e911Expected
                ? '这是北美运营商，通常必须配置'
                : '非北美运营商一般不需要，可以留空'
            }
          >
            {e911Expected ? (
              <Alert severity="warning">
                该运营商属于北美（MCC 310–316）。美国 FCC 要求 VoWiFi 登记紧急地址，
                运营商通常也会用 entitlement 流程卡注册，建议完整填写。
              </Alert>
            ) : (
              <Alert severity="info">
                该国家一般不强制要求紧急呼叫配置，保持关闭即可，不影响正常注册与通话。
              </Alert>
            )}
            <FormControlLabel
              control={
                <Switch
                  checked={draft.e911.enabled}
                  onChange={(_, enabled) =>
                    patch('e911', {
                      enabled,
                      // Enabling without a host policy is not actionable, so
                      // fill the usual default rather than failing validation.
                      websheet_host_policy:
                        enabled && !draft.e911.websheet_host_policy
                          ? 'public_https'
                          : draft.e911.websheet_host_policy,
                    })
                  }
                />
              }
              label="启用紧急呼叫配置"
            />
            <Stack direction={{ xs: 'column', sm: 'row' }} spacing={2}>
              <TextField
                fullWidth
                label="Provider"
                value={draft.e911.provider ?? ''}
                disabled={!draft.e911.enabled}
                placeholder="att_entitlement"
                onChange={(e) => patch('e911', { provider: e.target.value || null })}
              />
              <TextField
                fullWidth
                label="Websheet host policy"
                value={draft.e911.websheet_host_policy ?? ''}
                disabled={!draft.e911.enabled}
                placeholder="public_https"
                onChange={(e) =>
                  patch('e911', { websheet_host_policy: e.target.value || null })
                }
              />
            </Stack>
            <TextField
              label="Entitlement URL"
              value={draft.e911.entitlement_url ?? ''}
              disabled={!draft.e911.enabled}
              placeholder="https://example.carrier.net/"
              onChange={(e) => patch('e911', { entitlement_url: e.target.value || null })}
            />
          </Section>

          {validationError && <Alert severity="error">{validationError}</Alert>}
          {error && <Alert severity="error">{error}</Alert>}
        </Stack>
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose} disabled={saving}>取消</Button>
        <Button
          variant="contained"
          onClick={() => void save()}
          disabled={saving || Boolean(validationError)}
        >
          {saving ? '保存中…' : '保存 profile'}
        </Button>
      </DialogActions>
    </Dialog>
  )
}
