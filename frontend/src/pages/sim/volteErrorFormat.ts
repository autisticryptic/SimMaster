const PROFILE_NOT_READY = /carrier_catalog_profile_not_ready:([^:]+):lte_epc:([^:]+)/
const PROFILE_PLMN = /(?:home_plmn|imsi_prefix):([0-9]{5,6}|unknown):access:lte_epc:no_ready_profile/

function profileStatusLabel(status: string) {
  switch (status) {
    case 'partial': return '配置不完整'
    case 'unknown': return '尚无可信配置'
    case 'disabled': return '已停用'
    default: return status
  }
}

function plmnLabel(plmn: string) {
  if (plmn === 'unknown') return '未知 PLMN'
  if (['46000', '46002', '46004', '46007', '46008'].includes(plmn)) return `中国移动（PLMN ${plmn}）`
  if (['46001', '46006', '46009'].includes(plmn)) return `中国联通（PLMN ${plmn}）`
  if (['46003', '46005', '46011'].includes(plmn)) return `中国电信（PLMN ${plmn}）`
  if (plmn === '46015') return `中国广电（PLMN ${plmn}）`
  return `PLMN ${plmn}`
}

export function standardDerivedProfileMessage(
  source?: string | null,
  fallbackReason?: string | null,
) {
  if (source !== 'derived') return null

  const profileNotReady = fallbackReason?.match(PROFILE_NOT_READY)
  if (profileNotReady) {
    return `运营商数据库没有可用配置（已有条目${profileStatusLabel(profileNotReady[2])}），当前使用未经运营商验证的 3GPP 标准自动推断。`
  }
  if (fallbackReason?.includes('carrier_catalog_open_failed')) {
    return '运营商数据库无法读取，当前使用未经运营商验证的 3GPP 标准自动推断。'
  }
  return '运营商数据库没有可用配置，当前使用未经运营商验证的 3GPP 标准自动推断。'
}

export function volteErrorMessage(error?: string | null) {
  if (!error) return null

  const profileNotReady = error.match(PROFILE_NOT_READY)
  if (profileNotReady) {
    return `SIM 身份已读取，但运营商 VoLTE profile 不可用（${profileStatusLabel(profileNotReady[2])}）。请更新 carrier catalog，或为当前线路导入经过验证的运营商配置。`
  }

  const profilePlmn = error.match(PROFILE_PLMN)
  if (profilePlmn) {
    return `SIM 身份已读取，但 ${plmnLabel(profilePlmn[1])} 没有可用的 VoLTE profile。请更新 carrier catalog，或为当前线路导入经过验证的运营商配置。`
  }

  if (error.includes('carrier_catalog_open_failed')) {
    return 'SIM 身份已读取，但运营商配置库无法打开。请在运营商 Profile 页面安装或重新安装 carrier catalog。'
  }
  if (error.includes('carrier_catalog_schema_') || error.includes('carrier_catalog_config_contract_')) {
    return 'SIM 身份已读取，但运营商配置库版本与当前程序不兼容。请更新 carrier catalog。'
  }
  if (error.includes('volte_carrier_profile_missing')) {
    return 'SIM 身份已读取，但未匹配到可用的运营商 VoLTE profile。请检查线路 Profile 或更新 carrier catalog。'
  }
  if (error.includes('volte_carrier_ims_apn_missing')) {
    return '已匹配运营商 Profile，但其中缺少 IMS APN，无法建立 VoLTE Bearer。'
  }
  if (error.includes('volte_mm_imsi_missing') || error.includes('volte_imsi_missing')) {
    return 'ModemManager SIM 属性与 AT+CIMI 均未返回有效 IMSI。请确认 SIM 已就绪，并检查基带 AT 端口状态。'
  }
  if (error.includes('volte_usim_aka_failed')) {
    return 'SIM 身份已读取，但 USIM AKA 鉴权失败。请检查 UIM 通道、卡槽映射和运营商鉴权响应。'
  }
  if (error.includes('volte_runtime_all_pcscf_failed')) {
    return 'IMS Bearer 已建立，但所有 P-CSCF 候选均连接失败。请检查运营商 Profile、PCO/DNS 返回和 IMS 路由。'
  }
  if (error.includes('volte_bearer_netdev_runtime_error')) {
    return 'IMS Bearer 已建立，但 Qualcomm bam-dmux 网卡报告 runtime 错误，底层基带数据通道未打开。系统已停止继续切换地址族，避免反复激活导致基带崩溃；请检查基带/remoteproc 日志或重启设备。'
  }
  if (error.includes('volte_bearer_netdev_not_up') || error.includes('volte_bearer_netdev_not_ready')) {
    return 'IMS Bearer 已建立，但其网卡没有完成 OPEN/UP 握手。系统已停止继续安装路由和重复重试，避免把底层链路故障误报成 P-CSCF 失败。'
  }
  return error
}
