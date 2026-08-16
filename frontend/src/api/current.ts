import type {
  ApiResponse,
  AutomationConfig,
  AutomationLogsResponse,
  AuthSettingsResponse,
  AuthStatusResponse,
  BandLockRequest,
  BandLockStatus,
  BasebandRestartResponse,
  CallHistoryResponse,
  CallListResponse,
  CallSettingsResponse,
  CarrierProfileImportRequest,
  CarrierProfileImportResult,
  CarrierCatalogInstallRequest,
  CarrierCatalogInstallResponse,
  CarrierCatalogStatusResponse,
  CarrierProfileRecord,
  CellLocationResponse,
  ResolvedCarrierProfile,
  StoredCarrierProfile,
  CellLockRequest,
  CellLockResult,
  CellLockStatusResponse,
  ChangePasswordRequest,
  CellsResponse,
  ConnectionAddressesResponse,
  ConnectivityCheckResponse,
  LineNetworkControlsResponse,
  LineDataProxyConfig,
  DdnsConfig,
  DdnsLogsResponse,
  DdnsStatusResponse,
  DdnsSyncResponse,
  DeviceInfo,
  EsimCommandResponse,
  EsimDownloadRequest,
  EsimConfig,
  EsimReaderConfig,
  EsimEuiccInfo,
  EsimLpacRepairRequest,
  EsimLpacRepairResponse,
  EsimLpacStatusResponse,
  EsimProfilesResponse,
  ExternalVowifiProfile,
  LoginRequest,
  LineRuntimeStatus,
  SupplementarySnapshot,
  ManualRegisterRequest,
  TrunkProfileConfig,
  VowifiLineConfigResponse,
  LineVowifiConfig,
  ImsOverrideResponse,
  SimImsOverride,
  StandaloneSimSlotConfig,
  TrunkProfileResponse,
  VolteLineControlResponse,
  NetworkInfo,
  NetworkInterfacesResponse,
  NotificationConfig,
  NotificationLogsResponse,
  NotificationQueueResponse,
  OperatorListResponse,
  OtaStatusResponse,
  OtaLatestReleaseResponse,
  OtaOnlinePrepareRequest,
  OtaUploadResponse,
  PcscReaderInfo,
  GithubDownloadProxyConfig,
  RadioMode,
  RadioModeResponse,
  RoamingRequest,
  SecurityConfig,
  SignalStrengthResponse,
  SimInfo,
  UpdateSimCacheRequest,
  SmsMessage,
  SmsConversationRequest,
  SmsChannelResponse,
  SmsListRequest,
  SmsStats,
  SmsPathPolicy,
  SystemStatsResponse,
  VowifiProfilesResponse,
  WebhookTestResponse,
  LineEsimControlResponse,
  VoicePathPolicy,
  WebCallCapabilitiesResponse,
  VilteConfig,
  VilteStatusResponse,
  VolteVoiceStatusResponse,
  WlanConnectRequest,
  WlanForgetRequest,
  WlanProfileRequest,
  WlanProfilesResponse,
  WlanScanResponse,
  WlanStatusResponse,
  E911Capability,
  E911Operation,
  E911Status,
} from './types'

type SmsListResponse = {
  messages: SmsMessage[]
}

const API_BASE = '/api'

type RequestOptions = RequestInit & {
  returnText?: boolean
  timeoutMs?: number
  skipAuthRedirect?: boolean
}

function redirectToLogin() {
  const currentPath = `${window.location.pathname}${window.location.search}`
  if (window.location.pathname === '/login') return
  window.location.assign(currentPath === '/' ? '/login' : `/login?next=${encodeURIComponent(currentPath)}`)
}

function httpStatusMessage(status: number) {
  if (status === 400) return '请求参数有误'
  if (status === 401) return '请先登录'
  if (status === 403) return '没有权限执行此操作'
  if (status === 404) return '请求的接口不存在'
  if (status === 408) return '请求超时'
  if (status === 413) return '上传内容过大'
  if (status >= 500) return '服务器处理失败'
  return `请求失败，状态码 ${status}`
}

function throwIfApiEnvelopeError(payload: unknown): void {
  if (typeof payload !== 'object' || payload === null) return
  if (!('status' in payload)) return
  const status = (payload as { status: unknown }).status
  const message = (payload as { message?: unknown }).message
  if (status === 'error' && typeof message === 'string') {
    throw new Error(message)
  }
}

async function request<T>(
  url: string,
  options: RequestOptions = {},
): Promise<T> {
  const { returnText, timeoutMs, skipAuthRedirect, ...fetchOptions } = options
  const controller = timeoutMs ? new AbortController() : undefined
  const timeoutId = controller
    ? window.setTimeout(() => controller.abort(), timeoutMs)
    : undefined

  let response: Response
  try {
    response = await fetch(`${API_BASE}${url}`, {
      headers: {
        'Content-Type': 'application/json',
        ...fetchOptions.headers,
      },
      credentials: 'same-origin',
      ...fetchOptions,
      signal: controller?.signal ?? fetchOptions.signal,
    })
  } catch (err) {
    if (controller?.signal.aborted) {
      throw new Error(`Request timed out after ${timeoutMs}ms`)
    }
    throw err
  } finally {
    if (timeoutId !== undefined) window.clearTimeout(timeoutId)
  }

  if (!response.ok) {
    if (response.status === 401 && !skipAuthRedirect) {
      redirectToLogin()
    }
    let apiMessage: string | undefined
    try {
      const payload = await response.json()
      if (typeof payload === 'object' && payload !== null && 'message' in payload) {
        const message = (payload as { message?: unknown }).message
        if (typeof message === 'string') apiMessage = message
      }
    } catch {
      // Fall back to the HTTP status below.
    }
    if (apiMessage) throw new Error(apiMessage)
    throw new Error(httpStatusMessage(response.status))
  }

  if (returnText) {
    return (await response.text()) as T
  }

  const json = (await response.json()) as T
  throwIfApiEnvelopeError(json)
  return json
}

function modemLinePath(lineId: string, suffix: string) {
  return `/modem/lines/${encodeURIComponent(lineId)}${suffix}`
}

class SimAdminCurrentAPI {
  async getAuthStatus() {
    return request<ApiResponse<AuthStatusResponse>>('/auth/status', {
      skipAuthRedirect: true,
    })
  }

  async setupAdminPassword(password: string) {
    const body: LoginRequest = { password }
    return request<ApiResponse<null>>('/auth/setup', {
      method: 'POST',
      body: JSON.stringify(body),
      skipAuthRedirect: true,
    })
  }

  async login(password: string) {
    const body: LoginRequest = { password }
    return request<ApiResponse<null>>('/auth/login', {
      method: 'POST',
      body: JSON.stringify(body),
      skipAuthRedirect: true,
    })
  }

  async changeAdminPassword(newPassword: string) {
    const body: ChangePasswordRequest = {
      new_password: newPassword,
    }
    return request<ApiResponse<null>>('/auth/password', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  }

  async getAuthSettings() {
    return request<ApiResponse<AuthSettingsResponse>>('/auth/settings')
  }

  async setAuthSettings(settings: SecurityConfig) {
    return request<ApiResponse<SecurityConfig>>('/auth/settings', {
      method: 'POST',
      body: JSON.stringify(settings),
    })
  }

  async logout() {
    return request<ApiResponse<null>>('/auth/logout', {
      method: 'POST',
      body: JSON.stringify({}),
      skipAuthRedirect: true,
    })
  }

  async health() {
    return request<{ status: string; message: string; version: string }>('/health')
  }

  async getLineEsimControl(lineId: string) {
    return request<ApiResponse<LineEsimControlResponse>>(
      `/modem/lines/${encodeURIComponent(lineId)}/esim-control`,
    )
  }

  async setLineEsimControl(lineId: string, esimControl: boolean | null) {
    return request<ApiResponse<LineEsimControlResponse>>(
      `/modem/lines/${encodeURIComponent(lineId)}/esim-control`,
      {
        method: 'POST',
        body: JSON.stringify({ esim_control: esimControl }),
        timeoutMs: 10000,
      },
    )
  }

  async getLineEsimReaderConfig(lineId: string) {
    return request<ApiResponse<EsimReaderConfig>>(
      modemLinePath(lineId, '/esim-reader'),
    )
  }

  async setLineEsimReaderConfig(lineId: string, config: EsimReaderConfig) {
    return request<ApiResponse<EsimReaderConfig>>(
      modemLinePath(lineId, '/esim-reader'),
      {
        method: 'POST',
        body: JSON.stringify(config),
        timeoutMs: 10000,
      },
    )
  }

  async getEsimConfig() {
    return request<ApiResponse<EsimConfig>>('/esim/config')
  }

  async setEsimConfig(config: EsimConfig) {
    return request<ApiResponse<void>>('/esim/config', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getEsimEuicc(lineId: string) {
    return request<ApiResponse<EsimEuiccInfo>>(modemLinePath(lineId, '/esim/euicc'), {
      timeoutMs: 30000,
    })
  }

  async getEsimProfiles(lineId: string) {
    return request<ApiResponse<EsimProfilesResponse>>(modemLinePath(lineId, '/esim/profiles'), {
      timeoutMs: 30000,
    })
  }

  async getCachedEsimProfiles() {
    return request<ApiResponse<EsimProfilesResponse>>('/esim/profiles/cache', {
      timeoutMs: 5000,
    })
  }

  async getEsimLpacStatus() {
    return request<ApiResponse<EsimLpacStatusResponse>>('/esim/lpac/status', {
      timeoutMs: 15000,
    })
  }

  async repairEsimLpac(config: EsimLpacRepairRequest) {
    return request<ApiResponse<EsimLpacRepairResponse>>('/esim/lpac/repair', {
      method: 'POST',
      body: JSON.stringify(config),
      timeoutMs: 120000,
    })
  }

  async enableEsimProfile(lineId: string, iccid: string) {
    return request<ApiResponse<EsimCommandResponse>>(modemLinePath(lineId, `/esim/profiles/${encodeURIComponent(iccid)}/enable`), {
      method: 'POST',
      body: JSON.stringify({}),
      timeoutMs: 10000,
    })
  }


  async renameEsimProfile(lineId: string, iccid: string, name: string) {
    return request<ApiResponse<EsimCommandResponse>>(modemLinePath(lineId, `/esim/profiles/${encodeURIComponent(iccid)}/rename`), {
      method: 'POST',
      body: JSON.stringify({ name }),
      timeoutMs: 60000,
    })
  }

  async deleteEsimProfile(lineId: string, iccid: string) {
    return request<ApiResponse<EsimCommandResponse>>(modemLinePath(lineId, `/esim/profiles/${encodeURIComponent(iccid)}`), {
      method: 'DELETE',
      timeoutMs: 60000,
    })
  }

  async downloadEsimProfile(lineId: string, requestData: EsimDownloadRequest) {
    return request<ApiResponse<EsimCommandResponse>>(modemLinePath(lineId, '/esim/profiles'), {
      method: 'POST',
      body: JSON.stringify(requestData),
      timeoutMs: 180000, // 3 minutes timeout
    })
  }

  async getDeviceInfo(lineId: string) {
    return request<ApiResponse<DeviceInfo>>(modemLinePath(lineId, '/device'))
  }

  async getSimInfo(lineId: string) {
    return request<ApiResponse<SimInfo>>(modemLinePath(lineId, '/sim'))
  }

  async updateSimCache(lineId: string, data: UpdateSimCacheRequest) {
    return request<ApiResponse<void>>(modemLinePath(lineId, '/sim/cache'), {
      method: 'POST',
      body: JSON.stringify(data),
    })
  }

  async getNetworkInfo(lineId: string) {
    return request<ApiResponse<NetworkInfo>>(modemLinePath(lineId, '/network'))
  }

  async getCellsInfo(lineId: string) {
    return request<ApiResponse<CellsResponse>>(modemLinePath(lineId, '/cells'))
  }

  async startCellMonitor(lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/cell-monitor/start'), {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async stopCellMonitor(lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/cell-monitor/stop'), {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async restartBaseband(lineId: string) {
    return request<ApiResponse<BasebandRestartResponse>>(modemLinePath(lineId, '/baseband/restart'), {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getBasebandRestartStatus(lineId: string) {
    return request<ApiResponse<BasebandRestartResponse>>(modemLinePath(lineId, '/baseband/restart/status'))
  }

  async restartService() {
    return request<ApiResponse<Record<string, never>>>('/service/restart', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async restartModemManager() {
    return request<ApiResponse<Record<string, never>>>('/service/modem-manager/restart', {
      method: 'POST',
      body: JSON.stringify({}),
      timeoutMs: 30000,
    })
  }

  async rebootSystem(delaySeconds = 1) {
    return request<ApiResponse<{ delay_seconds: number }>>('/system/reboot', {
      method: 'POST',
      body: JSON.stringify({ delay_seconds: delaySeconds }),
    })
  }

  // Roaming and airplane mode are per line only — see `setLineRoaming` and
  // `setLineAirplaneMode`, and read state from `getLineNetworkControls`. The old
  // global `/roaming` and `/airplane-mode` endpoints are gone: they acted on
  // whichever modem came first while pretending to be device-wide.

  async getLineNetworkControls() {
    return request<ApiResponse<LineNetworkControlsResponse[]>>('/modem/line-controls')
  }

  async getLineDataConnection(lineId: string) {
    return request<ApiResponse<LineNetworkControlsResponse>>(
      `/modem/lines/${encodeURIComponent(lineId)}/data`,
    )
  }

  async setLineDataConnection(lineId: string, enabled: boolean) {
    return request<ApiResponse<LineNetworkControlsResponse>>(
      `/modem/lines/${encodeURIComponent(lineId)}/data`,
      { method: 'POST', body: JSON.stringify({ enabled }) },
    )
  }

  async setLineDataProxyConfig(lineId: string, config: LineDataProxyConfig) {
    return request<ApiResponse<LineNetworkControlsResponse>>(
      `/modem/lines/${encodeURIComponent(lineId)}/data/config`,
      { method: 'POST', body: JSON.stringify(config) },
    )
  }

  async setLineRoaming(lineId: string, allowed: boolean) {
    const body: RoamingRequest = { allowed }
    return request<ApiResponse<LineNetworkControlsResponse>>(
      `/modem/lines/${encodeURIComponent(lineId)}/roaming`,
      { method: 'POST', body: JSON.stringify(body) },
    )
  }

  /** Zero one line's proxied-traffic counters. */
  async resetLineDataTraffic(lineId: string) {
    return request<ApiResponse<LineNetworkControlsResponse>>(
      `/modem/lines/${encodeURIComponent(lineId)}/data/traffic/reset`,
      { method: 'POST', body: JSON.stringify({}) },
    )
  }

  async setLineAirplaneMode(lineId: string, enabled: boolean) {
    return request<ApiResponse<LineNetworkControlsResponse>>(
      `/modem/lines/${encodeURIComponent(lineId)}/airplane-mode`,
      { method: 'POST', body: JSON.stringify({ enabled }) },
    )
  }

  async getSystemStats() {
    return request<ApiResponse<SystemStatsResponse>>('/stats')
  }

  async getNetworkInterfaces() {
    return request<ApiResponse<NetworkInterfacesResponse>>('/network/interfaces')
  }

  async getNetworkConnectionAddresses() {
    return request<ApiResponse<ConnectionAddressesResponse>>('/network/connection-addresses')
  }

  async getSignalStrength(lineId: string) {
    return request<ApiResponse<SignalStrengthResponse>>(modemLinePath(lineId, '/network/signal-strength'))
  }

  async getCellLocationInfo(lineId: string) {
    return request<ApiResponse<CellLocationResponse>>(modemLinePath(lineId, '/location/cell-info'))
  }

  async getOperators(lineId: string) {
    return request<ApiResponse<OperatorListResponse>>(modemLinePath(lineId, '/network/operators'))
  }

  async scanOperators(lineId: string) {
    return request<ApiResponse<OperatorListResponse>>(modemLinePath(lineId, '/network/operators/scan'))
  }

  async registerOperatorManual(mccmnc: string, lineId: string) {
    const body: ManualRegisterRequest = { mccmnc }
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/network/register-manual'), {
      method: 'POST',
      body: JSON.stringify(body),
    })
  }

  async registerOperatorAuto(lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/network/register-auto'), {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getRadioMode(lineId: string) {
    return request<ApiResponse<RadioModeResponse>>(modemLinePath(lineId, '/radio-mode'))
  }

  async setRadioMode(mode: RadioMode, lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/radio-mode'), {
      method: 'POST',
      body: JSON.stringify({ mode }),
    })
  }

  async getBandLockStatus(lineId: string) {
    return request<ApiResponse<BandLockStatus>>(modemLinePath(lineId, '/band-lock'))
  }

  async setBandLock(config: BandLockRequest, lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/band-lock'), {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getCellLockStatus(lineId: string) {
    return request<ApiResponse<CellLockStatusResponse>>(modemLinePath(lineId, '/cell-lock'))
  }

  async setCellLock(lineId: string, config: CellLockRequest) {
    return request<ApiResponse<CellLockResult>>(modemLinePath(lineId, '/cell-lock'), {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async unlockAllCells(lineId: string) {
    return request<ApiResponse<CellLockResult>>(modemLinePath(lineId, '/cell-lock/unlock-all'), {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getConnectivity() {
    return request<ApiResponse<ConnectivityCheckResponse>>('/connectivity')
  }

  async getDdnsConfig() {
    return request<ApiResponse<DdnsConfig>>('/device-network/ddns/config')
  }

  async setDdnsConfig(config: DdnsConfig) {
    return request<ApiResponse<DdnsConfig>>('/device-network/ddns/config', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getDdnsStatus() {
    return request<ApiResponse<DdnsStatusResponse>>('/device-network/ddns/status')
  }

  async syncDdnsNow() {
    return request<ApiResponse<DdnsSyncResponse>>('/device-network/ddns/sync', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getDdnsLogs() {
    return request<ApiResponse<DdnsLogsResponse>>('/device-network/ddns/logs')
  }

  async clearDdnsLogs() {
    return request<ApiResponse<Record<string, never>>>('/device-network/ddns/logs/clear', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getWlanStatus() {
    return request<ApiResponse<WlanStatusResponse>>('/device-network/wlan/status')
  }

  async setWlanEnabled(enabled: boolean) {
    return request<ApiResponse<WlanStatusResponse>>('/device-network/wlan/enabled', {
      method: 'POST',
      body: JSON.stringify({ enabled }),
    })
  }

  async scanWlan() {
    return request<ApiResponse<WlanScanResponse>>('/device-network/wlan/scan', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getWlanProfiles() {
    return request<ApiResponse<WlanProfilesResponse>>('/device-network/wlan/profiles')
  }

  async forgetWlan(config: WlanForgetRequest) {
    return request<ApiResponse<WlanProfilesResponse>>('/device-network/wlan/forget', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async connectWlan(config: WlanConnectRequest) {
    return request<ApiResponse<WlanStatusResponse>>('/device-network/wlan/connect', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async disconnectWlan() {
    return request<ApiResponse<WlanStatusResponse>>('/device-network/wlan/disconnect', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async saveWlanProfile(config: WlanProfileRequest) {
    return request<ApiResponse<WlanStatusResponse>>('/device-network/wlan/profile', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getModemLines() {
    return request<ApiResponse<LineRuntimeStatus[]>>('/modems')
  }

  async getImsSupplementary(lineId: string) {
    return request<ApiResponse<SupplementarySnapshot>>(
      `/ims/lines/${encodeURIComponent(lineId)}/supplementary`,
    )
  }

  async getVolteLines() {
    return request<ApiResponse<VolteLineControlResponse[]>>('/volte/lines')
  }

  async getVolteLine(lineId: string) {
    return request<ApiResponse<VolteLineControlResponse>>(`/volte/lines/${encodeURIComponent(lineId)}`)
  }

  async setVolteLineConnection(lineId: string, enabled: boolean) {
    return request<ApiResponse<VolteLineControlResponse>>(
      `/volte/lines/${encodeURIComponent(lineId)}/connection`,
      {
        method: 'POST',
        body: JSON.stringify({ enabled }),
      },
    )
  }

  async retryVolteLine(lineId: string) {
    return request<ApiResponse<VolteLineControlResponse>>(
      `/volte/lines/${encodeURIComponent(lineId)}/retry`,
      { method: 'POST', body: JSON.stringify({}) },
    )
  }

  async getVowifiLines() {
    return request<ApiResponse<VowifiLineConfigResponse[]>>('/vowifi/lines')
  }

  async getVowifiLine(lineId: string) {
    return request<ApiResponse<VowifiLineConfigResponse>>(`/vowifi/lines/${encodeURIComponent(lineId)}`)
  }

  async setVowifiLineConnection(lineId: string, enabled: boolean) {
    return request<ApiResponse<VowifiLineConfigResponse>>(
      `/vowifi/lines/${encodeURIComponent(lineId)}/connection`,
      {
        method: 'POST',
        body: JSON.stringify({ enabled }),
      },
    )
  }

  async setVowifiLineConfig(lineId: string, config: LineVowifiConfig) {
    return request<ApiResponse<VowifiLineConfigResponse>>(`/vowifi/lines/${encodeURIComponent(lineId)}`, {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getImsOverride(lineId: string) {
    return request<ApiResponse<ImsOverrideResponse>>(
      `/ims/lines/${encodeURIComponent(lineId)}/override`,
    )
  }

  async setImsOverride(lineId: string, override: SimImsOverride) {
    return request<ApiResponse<ImsOverrideResponse>>(
      `/ims/lines/${encodeURIComponent(lineId)}/override`,
      { method: 'PATCH', body: JSON.stringify(override) },
    )
  }

  async getStandaloneSimSlots() {
    return request<ApiResponse<StandaloneSimSlotConfig[]>>('/sim/slots')
  }

  async setStandaloneSimSlots(slots: StandaloneSimSlotConfig[]) {
    return request<ApiResponse<StandaloneSimSlotConfig[]>>('/sim/slots', {
      method: 'POST',
      body: JSON.stringify(slots),
    })
  }

  async getPcscReaders() {
    return request<ApiResponse<PcscReaderInfo[]>>('/sim/readers')
  }

  async getTrunkLines() {
    return request<ApiResponse<TrunkProfileResponse[]>>('/trunk/lines')
  }

  async getTrunkLine(lineId: string) {
    return request<ApiResponse<TrunkProfileResponse>>(`/trunk/lines/${encodeURIComponent(lineId)}`)
  }

  async setTrunkLine(lineId: string, profile: TrunkProfileConfig) {
    return request<ApiResponse<TrunkProfileResponse>>(`/trunk/lines/${encodeURIComponent(lineId)}`, {
      method: 'POST',
      body: JSON.stringify(profile),
    })
  }

  async setTrunkLineEnabled(lineId: string, enabled: boolean) {
    return request<ApiResponse<TrunkProfileResponse>>(`/trunk/lines/${encodeURIComponent(lineId)}/enabled`, {
      method: 'POST',
      body: JSON.stringify({ enabled }),
    })
  }

  async sendSms(lineId: string, phoneNumber: string, content: string) {
    return request<ApiResponse<{ path: string; transport?: string; line_id: string }>>(modemLinePath(lineId, '/sms/send'), {
      method: 'POST',
      body: JSON.stringify({ phone_number: phoneNumber, content }),
    })
  }

  async getSmsList(params?: SmsListRequest) {
    const query = new URLSearchParams()
    if (params?.limit) query.append('limit', params.limit.toString())
    if (params?.offset) query.append('offset', params.offset.toString())
    if (params?.direction) query.append('direction', params.direction)
    if (params?.channel_id) query.append('channel_id', params.channel_id)
    const queryStr = query.toString() ? `?${query.toString()}` : ''
    return request<ApiResponse<SmsListResponse>>(`/sms/list${queryStr}`)
  }

  async getSmsConversation(params: SmsConversationRequest) {
    const query = new URLSearchParams()
    query.append('phone_number', params.phone_number)
    if (params.limit) query.append('limit', params.limit.toString())
    if (params.channel_id) query.append('channel_id', params.channel_id)
    return request<ApiResponse<SmsListResponse>>(`/sms/conversation?${query.toString()}`)
  }

  async getSmsChannels() {
    return request<ApiResponse<SmsChannelResponse[]>>('/sms/channels')
  }

  async getSmsStats(channelId?: string) {
    const query = channelId ? `?channel_id=${encodeURIComponent(channelId)}` : ''
    return request<ApiResponse<SmsStats>>(`/sms/stats${query}`)
  }

  async getSmsPathPolicy(lineId: string) {
    return request<ApiResponse<SmsPathPolicy>>(modemLinePath(lineId, '/sms/path-policy'))
  }

  async setSmsPathPolicy(lineId: string, policy: SmsPathPolicy) {
    return request<ApiResponse<SmsPathPolicy>>(modemLinePath(lineId, '/sms/path-policy'), {
      method: 'POST',
      body: JSON.stringify(policy),
    })
  }

  async deleteSmsMessage(id: number, channelId: string) {
    return request<ApiResponse<{ deleted: number }>>(`/sms/message/${id}?channel_id=${encodeURIComponent(channelId)}`, {
      method: 'DELETE',
    })
  }

  async deleteSmsConversation(phoneNumber: string, channelId: string) {
    const query = `?channel_id=${encodeURIComponent(channelId)}`
    return request<ApiResponse<{ deleted: number }>>(
      `/sms/conversation/${encodeURIComponent(phoneNumber)}${query}`,
      { method: 'DELETE' },
    )
  }

  async deleteSmsBatch(payload: { ids?: number[]; phone_numbers?: string[]; channel_id: string }) {
    return request<ApiResponse<{ deleted: number }>>('/sms/batch-delete', {
      method: 'POST',
      body: JSON.stringify(payload),
    })
  }

  async getCalls(lineId: string) {
    return request<ApiResponse<CallListResponse>>(modemLinePath(lineId, '/calls'))
  }

  async dialCall(phoneNumber: string, lineId: string) {
    return request<ApiResponse<{ path: string; line_id: string }>>(modemLinePath(lineId, '/calls/dial'), {
      method: 'POST',
      body: JSON.stringify({ phone_number: phoneNumber }),
    })
  }

  async hangupCall(path: string, lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/calls/hangup'), {
      method: 'POST',
      body: JSON.stringify({ path }),
    })
  }

  async hangupAllCalls(lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/calls/hangup-all'), {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async answerCall(path: string, lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/calls/answer'), {
      method: 'POST',
      body: JSON.stringify({ path }),
    })
  }

  async sendCallDtmf(path: string, digit: string, lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/calls/dtmf'), {
      method: 'POST',
      body: JSON.stringify({ path, digit }),
    })
  }

  async getCallHistory(params: { lineId: string; limit?: number; offset?: number }) {
    const query = new URLSearchParams()
    if (params?.limit) query.append('limit', params.limit.toString())
    if (params?.offset) query.append('offset', params.offset.toString())
    const queryStr = query.toString() ? `?${query.toString()}` : ''
    return request<ApiResponse<CallHistoryResponse>>(
      modemLinePath(params.lineId, `/calls/history${queryStr}`),
    )
  }

  async deleteCallRecord(id: number, lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, `/calls/history/${id}`), {
      method: 'DELETE',
    })
  }

  async clearCallHistory(lineId: string) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/calls/history/clear'), {
      method: 'POST',
    })
  }

  async getVoicePathPolicy(lineId: string) {
    return request<ApiResponse<VoicePathPolicy>>(modemLinePath(lineId, '/voice/path-policy'))
  }

  async setVoicePathPolicy(lineId: string, policy: VoicePathPolicy) {
    return request<ApiResponse<VoicePathPolicy>>(modemLinePath(lineId, '/voice/path-policy'), {
      method: 'POST',
      body: JSON.stringify(policy),
    })
  }

  async getWebCallCapabilities() {
    return request<ApiResponse<WebCallCapabilitiesResponse>>('/web-call/capabilities')
  }

  async getVolteVoiceStatus(lineId: string) {
    return request<ApiResponse<VolteVoiceStatusResponse>>(modemLinePath(lineId, '/volte/call/status'))
  }

  async setVolteVoice(lineId: string, enabled: boolean) {
    return request<ApiResponse<VolteVoiceStatusResponse>>(modemLinePath(lineId, '/volte/voice'), {
      method: 'POST',
      body: JSON.stringify({ enabled }),
    })
  }

  async getVilteStatus(lineId: string) {
    return request<ApiResponse<VilteStatusResponse>>(modemLinePath(lineId, '/vilte/control'))
  }

  async setVilteFeature(lineId: string, enabled: boolean) {
    return request<ApiResponse<VilteStatusResponse>>(modemLinePath(lineId, '/vilte/control'), {
      method: 'POST',
      body: JSON.stringify({ enabled }),
    })
  }

  async setVilteConfig(lineId: string, config: VilteConfig) {
    return request<ApiResponse<VilteStatusResponse>>(modemLinePath(lineId, '/vilte/config'), {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getCallSettings(lineId: string) {
    return request<ApiResponse<CallSettingsResponse>>(modemLinePath(lineId, '/calls/settings'))
  }

  async setCallWaiting(lineId: string, enabled: boolean) {
    return request<ApiResponse<Record<string, never>>>(modemLinePath(lineId, '/calls/settings'), {
      method: 'POST',
      body: JSON.stringify({ property: 'VoiceCallWaiting', value: enabled ? 'enabled' : 'disabled' }),
    })
  }

  async getNotificationConfig() {
    return request<ApiResponse<NotificationConfig>>('/notifications/config')
  }

  async setNotificationConfig(config: NotificationConfig) {
    return request<ApiResponse<Record<string, unknown>>>('/notifications/config', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async testNotificationChannel(channel: string) {
    return request<ApiResponse<WebhookTestResponse>>(`/notifications/test/${channel}`, {
      method: 'POST',
    })
  }

  async getNotificationLogs(params?: { type?: string; status?: string; line_id?: string; q?: string; start_date?: string; end_date?: string; limit?: number; offset?: number }) {
    const query = new URLSearchParams()
    if (params?.type) query.append('type', params.type)
    if (params?.status) query.append('status', params.status)
    if (params?.line_id) query.append('line_id', params.line_id)
    if (params?.q) query.append('q', params.q)
    if (params?.start_date) query.append('start_date', params.start_date)
    if (params?.end_date) query.append('end_date', params.end_date)
    if (params?.limit) query.append('limit', params.limit.toString())
    if (params?.offset) query.append('offset', params.offset.toString())
    const queryStr = query.toString() ? `?${query.toString()}` : ''
    return request<ApiResponse<NotificationLogsResponse>>(`/notifications/logs${queryStr}`)
  }

  async clearNotificationLogs(filters?: { type?: string; status?: string; line_id?: string; start_date?: string; end_date?: string }) {
    return request<ApiResponse<{ deleted: number }>>('/notifications/logs/clear', {
      method: 'POST',
      body: JSON.stringify(filters ?? {}),
    })
  }

  async getNotificationQueue(params?: { line_id?: string; limit?: number }) {
    const query = new URLSearchParams()
    if (params?.line_id) query.append('line_id', params.line_id)
    if (params?.limit) query.append('limit', params.limit.toString())
    const queryStr = query.toString() ? `?${query.toString()}` : ''
    return request<ApiResponse<NotificationQueueResponse>>(`/notifications/queue${queryStr}`)
  }

  async retryNotificationQueueItem(id: number | string) {
    return request<ApiResponse<{ updated: number }>>(`/notifications/queue/${id}/retry`, {
      method: 'POST',
    })
  }

  async deleteNotificationQueueItem(id: number | string) {
    return request<ApiResponse<{ updated: number }>>(`/notifications/queue/${id}`, {
      method: 'DELETE',
    })
  }

  async retryAllNotificationQueue() {
    return request<ApiResponse<{ updated: number }>>('/notifications/queue/retry-all', {
      method: 'POST',
    })
  }

  async clearNotificationQueue() {
    return request<ApiResponse<{ updated: number }>>('/notifications/queue/clear', {
      method: 'POST',
    })
  }

  async getOtaStatus() {
    return request<ApiResponse<OtaStatusResponse>>('/ota/status')
  }

  async getGithubDownloadProxy() {
    return request<ApiResponse<GithubDownloadProxyConfig>>('/settings/github-download-proxy')
  }

  async setGithubDownloadProxy(config: GithubDownloadProxyConfig) {
    return request<ApiResponse<GithubDownloadProxyConfig>>('/settings/github-download-proxy', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async uploadOta(file: File) {
    const response = await fetch(`${API_BASE}/ota/upload`, {
      method: 'POST',
      body: file,
      credentials: 'same-origin',
      headers: {
        'Content-Type': 'application/octet-stream',
      },
    })

    if (!response.ok) {
      if (response.status === 401) {
        redirectToLogin()
      }
      throw new Error(httpStatusMessage(response.status))
    }

    return response.json() as Promise<ApiResponse<OtaUploadResponse>>
  }

  async prepareOnlineOta(config: OtaOnlinePrepareRequest) {
    return request<ApiResponse<OtaUploadResponse>>('/ota/online-prepare', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getLatestOtaRelease(config: OtaOnlinePrepareRequest) {
    return request<ApiResponse<OtaLatestReleaseResponse>>('/ota/latest-release', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getVowifiProfiles() {
    return request<ApiResponse<VowifiProfilesResponse>>('/vowifi/profiles', {
      timeoutMs: 10000,
    })
  }

  // ---- VoWiFi carrier profile database ----
  // These replace the compiled-in carrier constants. Carriers with no row fall
  // back to the built-ins, and finally to 3GPP-derived defaults.

  async listVowifiCarrierProfiles() {
    return request<ApiResponse<StoredCarrierProfile[]>>('/vowifi/carrier-profiles', {
      timeoutMs: 15000,
    })
  }

  async getCarrierCatalogStatus() {
    return request<ApiResponse<CarrierCatalogStatusResponse>>('/vowifi/carrier-catalog/status', {
      timeoutMs: 15000,
    })
  }

  async installCarrierCatalog(config: CarrierCatalogInstallRequest) {
    return request<ApiResponse<CarrierCatalogInstallResponse>>('/vowifi/carrier-catalog/install', {
      method: 'POST',
      body: JSON.stringify(config),
      timeoutMs: 240000,
    })
  }

  async saveVowifiCarrierProfile(record: CarrierProfileRecord) {
    return request<ApiResponse<StoredCarrierProfile>>(
      '/vowifi/carrier-profiles',
      { method: 'PUT', body: JSON.stringify(record) },
    )
  }

  async deleteVowifiCarrierProfile(profileId: string) {
    return request<ApiResponse<{ deleted: boolean }>>(
      `/vowifi/carrier-profiles/${encodeURIComponent(profileId)}`,
      { method: 'DELETE' },
    )
  }

  /** Show which profile a PLMN resolves to, and whether it is stored or derived. */
  async resolveVowifiCarrierProfile(plmn: string) {
    return request<ApiResponse<ResolvedCarrierProfile>>(
      `/vowifi/carrier-profiles/resolve?plmn=${encodeURIComponent(plmn)}`,
    )
  }

  async importVowifiCarrierProfiles(payload: CarrierProfileImportRequest) {
    return request<ApiResponse<CarrierProfileImportResult>>('/vowifi/carrier-profiles/import', {
      method: 'POST',
      body: JSON.stringify(payload),
      timeoutMs: 30000,
    })
  }

  async getExternalVowifiProfiles() {
    return request<ApiResponse<ExternalVowifiProfile[]>>('/vowifi/external-profiles')
  }

  async setExternalVowifiProfile(profile: ExternalVowifiProfile) {
    return request<ApiResponse<ExternalVowifiProfile[]>>('/vowifi/external-profiles', {
      method: 'POST',
      body: JSON.stringify(profile),
    })
  }

  async getE911Capability(lineId: string) {
    return request<ApiResponse<E911Capability>>(
      `/ims/lines/${encodeURIComponent(lineId)}/e911/capability`,
      { timeoutMs: 10000 },
    )
  }

  async getE911Status(lineId: string) {
    return request<ApiResponse<E911Status>>(
      `/ims/lines/${encodeURIComponent(lineId)}/e911/status`,
      { timeoutMs: 10000 },
    )
  }

  async queryE911(lineId: string) {
    return request<ApiResponse<E911Status>>(
      `/ims/lines/${encodeURIComponent(lineId)}/e911/query`,
      { method: 'POST', timeoutMs: 30000 },
    )
  }

  async createE911Operation(lineId: string) {
    return request<ApiResponse<E911Operation>>(
      `/ims/lines/${encodeURIComponent(lineId)}/e911/operations`,
      { method: 'POST', timeoutMs: 10000 },
    )
  }

  async applyOta(restartNow = false) {
    return request<ApiResponse<{ applied: boolean }>>('/ota/apply', {
      method: 'POST',
      body: JSON.stringify({ restart_now: restartNow }),
    })
  }

  async cancelOta() {
    return request<ApiResponse<Record<string, unknown>>>('/ota/cancel', {
      method: 'POST',
    })
  }

  async getAutomationConfig() {
    return request<ApiResponse<AutomationConfig>>('/automation/config')
  }

  async setAutomationConfig(config: AutomationConfig) {
    return request<ApiResponse<Record<string, unknown>>>('/automation/config', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async testAutomationTask(taskId: string) {
    return request<ApiResponse<Record<string, unknown>>>(`/automation/test/${encodeURIComponent(taskId)}`, {
      method: 'POST',
    })
  }

  async getAutomationLogs(params?: { type?: string; status?: string; line_id?: string; start_date?: string; end_date?: string; q?: string; limit?: number; offset?: number }) {
    const query = new URLSearchParams()
    if (params?.type) query.append('type', params.type)
    if (params?.status) query.append('status', params.status)
    if (params?.line_id) query.append('line_id', params.line_id)
    if (params?.q) query.append('q', params.q)
    if (params?.start_date) query.append('start_date', params.start_date)
    if (params?.end_date) query.append('end_date', params.end_date)
    if (params?.limit) query.append('limit', params.limit.toString())
    if (params?.offset) query.append('offset', params.offset.toString())
    const queryStr = query.toString() ? `?${query.toString()}` : ''
    return request<ApiResponse<AutomationLogsResponse>>(`/automation/logs${queryStr}`)
  }

  async clearAutomationLogs(filters?: { type?: string; status?: string; line_id?: string; start_date?: string; end_date?: string }) {
    return request<ApiResponse<{ deleted: number }>>('/automation/logs/clear', {
      method: 'POST',
      body: JSON.stringify(filters ?? {}),
    })
  }
}

export const api = new SimAdminCurrentAPI()

export * from './types'
