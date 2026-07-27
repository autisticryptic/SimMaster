import type {
  ApiResponse,
  ApnListResponse,
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
  DataConnectionRequest,
  DataConnectionStatus,
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
  EsimEuiccInfo,
  EsimLpacRepairRequest,
  EsimLpacRepairResponse,
  EsimLpacStatusResponse,
  EsimProfilesResponse,
  ExternalVowifiProfile,
  LoginRequest,
  LineRuntimeStatus,
  ManualRegisterRequest,
  TrunkProfileConfig,
  VowifiLineConfigResponse,
  LineVowifiConfig,
  StandaloneSimSlotConfig,
  TrunkProfileResponse,
  VolteControlResponse,
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
  RadioMode,
  RadioModeResponse,
  RoamingRequest,
  SecurityConfig,
  SetApnRequest,
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
  VowifiConfig,
  VowifiDiagnosticsResponse,
  VowifiEsimRestoreEntry,
  VowifiProfileMatchResponse,
  VowifiProfilesResponse,
  VowifiRuntimeEventsResponse,
  VowifiSmsDeliveriesResponse,
  VowifiSoakRunsResponse,
  VowifiStatusResponse,
  WebhookTestResponse,
  WorkMode,
  WorkModeRequest,
  WorkModeResponse,
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

/// Build the `?line_id=…` suffix for line-scoped cellular endpoints. Omitting
/// the id lets the backend act on the primary line, which is what the
/// single-line pages want.
function lineScopeQuery(lineId?: string) {
  const trimmed = lineId?.trim()
  return trimmed ? `?line_id=${encodeURIComponent(trimmed)}` : ''
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

  async getWorkMode() {
    return request<ApiResponse<WorkModeResponse>>('/work-mode')
  }

  async setWorkMode(mode: WorkMode) {
    const body: WorkModeRequest = { mode, confirm: true }
    return request<ApiResponse<WorkModeResponse>>('/work-mode', {
      method: 'POST',
      body: JSON.stringify(body),
      timeoutMs: 10000,
    })
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

  async getEsimEuicc() {
    return request<ApiResponse<EsimEuiccInfo>>('/esim/euicc', {
      timeoutMs: 30000,
    })
  }

  async getEsimProfiles() {
    return request<ApiResponse<EsimProfilesResponse>>('/esim/profiles', {
      timeoutMs: 30000,
    })
  }

  async getCachedEsimProfiles() {
    return request<ApiResponse<EsimProfilesResponse>>('/esim/profiles?cached=1', {
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

  async enableEsimProfile(iccid: string) {
    return request<ApiResponse<EsimCommandResponse>>(`/esim/profiles/${encodeURIComponent(iccid)}/enable`, {
      method: 'POST',
      body: JSON.stringify({}),
      timeoutMs: 10000,
    })
  }


  async renameEsimProfile(iccid: string, name: string) {
    return request<ApiResponse<EsimCommandResponse>>(`/esim/profiles/${encodeURIComponent(iccid)}/rename`, {
      method: 'POST',
      body: JSON.stringify({ name }),
      timeoutMs: 60000,
    })
  }

  async deleteEsimProfile(iccid: string) {
    return request<ApiResponse<EsimCommandResponse>>(`/esim/profiles/${encodeURIComponent(iccid)}`, {
      method: 'DELETE',
      timeoutMs: 60000,
    })
  }

  async downloadEsimProfile(requestData: EsimDownloadRequest) {
    return request<ApiResponse<EsimCommandResponse>>('/esim/profiles', {
      method: 'POST',
      body: JSON.stringify(requestData),
      timeoutMs: 180000, // 3 minutes timeout
    })
  }

  async getDeviceInfo() {
    return request<ApiResponse<DeviceInfo>>('/device')
  }

  async getSimInfo() {
    return request<ApiResponse<SimInfo>>('/sim')
  }

  async updateSimCache(data: UpdateSimCacheRequest) {
    return request<ApiResponse<void>>('/sim/cache', {
      method: 'POST',
      body: JSON.stringify(data),
    })
  }

  async getNetworkInfo() {
    return request<ApiResponse<NetworkInfo>>('/network')
  }

  async getCellsInfo(lineId?: string) {
    return request<ApiResponse<CellsResponse>>(`/cells${lineScopeQuery(lineId)}`)
  }

  async startCellMonitor() {
    return request<ApiResponse<Record<string, never>>>('/cell-monitor/start', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async stopCellMonitor() {
    return request<ApiResponse<Record<string, never>>>('/cell-monitor/stop', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getDataStatus() {
    return request<ApiResponse<DataConnectionStatus>>('/data')
  }

  async setDataStatus(active: boolean) {
    const body: DataConnectionRequest = { active }
    return request<ApiResponse<DataConnectionStatus>>('/data', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  }

  async restartBaseband() {
    return request<ApiResponse<BasebandRestartResponse>>('/baseband/restart', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getBasebandRestartStatus() {
    return request<ApiResponse<BasebandRestartResponse>>('/baseband/restart/status')
  }

  async restartService() {
    return request<ApiResponse<Record<string, never>>>('/service/restart', {
      method: 'POST',
      body: JSON.stringify({}),
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

  async getSignalStrength() {
    return request<ApiResponse<SignalStrengthResponse>>('/network/signal-strength')
  }

  async getCellLocationInfo() {
    return request<ApiResponse<CellLocationResponse>>('/location/cell-info')
  }

  async getOperators(lineId?: string) {
    return request<ApiResponse<OperatorListResponse>>(`/network/operators${lineScopeQuery(lineId)}`)
  }

  async scanOperators(lineId?: string) {
    return request<ApiResponse<OperatorListResponse>>(`/network/operators/scan${lineScopeQuery(lineId)}`)
  }

  async registerOperatorManual(mccmnc: string, lineId?: string) {
    const body: ManualRegisterRequest = { mccmnc, line_id: lineId }
    return request<ApiResponse<Record<string, never>>>('/network/register-manual', {
      method: 'POST',
      body: JSON.stringify(body),
    })
  }

  async registerOperatorAuto(lineId?: string) {
    return request<ApiResponse<Record<string, never>>>(`/network/register-auto${lineScopeQuery(lineId)}`, {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async getApnList(lineId?: string) {
    return request<ApiResponse<ApnListResponse>>(`/apn${lineScopeQuery(lineId)}`)
  }

  async setApn(config: SetApnRequest) {
    return request<ApiResponse<Record<string, unknown>>>('/apn', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getRadioMode(lineId?: string) {
    return request<ApiResponse<RadioModeResponse>>(`/radio-mode${lineScopeQuery(lineId)}`)
  }

  async setRadioMode(mode: RadioMode, lineId?: string) {
    return request<ApiResponse<Record<string, never>>>('/radio-mode', {
      method: 'POST',
      body: JSON.stringify({ mode, line_id: lineId }),
    })
  }

  async getBandLockStatus(lineId?: string) {
    return request<ApiResponse<BandLockStatus>>(`/band-lock${lineScopeQuery(lineId)}`)
  }

  async setBandLock(config: BandLockRequest, lineId?: string) {
    return request<ApiResponse<Record<string, never>>>('/band-lock', {
      method: 'POST',
      body: JSON.stringify({ ...config, line_id: lineId }),
    })
  }

  async getCellLockStatus(lineId?: string) {
    return request<ApiResponse<CellLockStatusResponse>>(`/cell-lock${lineScopeQuery(lineId)}`)
  }

  async setCellLock(config: CellLockRequest) {
    return request<ApiResponse<CellLockResult>>('/cell-lock', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async unlockAllCells(lineId?: string) {
    return request<ApiResponse<CellLockResult>>(`/cell-lock/unlock-all${lineScopeQuery(lineId)}`, {
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

  async getVolteControl() {
    return request<ApiResponse<VolteControlResponse>>('/volte/control')
  }

  async setVolteFeature(enabled: boolean) {
    return request<ApiResponse<VolteControlResponse>>('/volte/feature', {
      method: 'POST',
      body: JSON.stringify({ enabled }),
    })
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

  async getStandaloneSimSlots() {
    return request<ApiResponse<StandaloneSimSlotConfig[]>>('/sim/slots')
  }

  async setStandaloneSimSlots(slots: StandaloneSimSlotConfig[]) {
    return request<ApiResponse<StandaloneSimSlotConfig[]>>('/sim/slots', {
      method: 'POST',
      body: JSON.stringify(slots),
    })
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

  async sendSms(phoneNumber: string, content: string, lineId?: string) {
    return request<ApiResponse<{ path: string; transport?: string; line_id?: string }>>('/sms/send', {
      method: 'POST',
      body: JSON.stringify({ phone_number: phoneNumber, content, line_id: lineId || undefined }),
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

  async getSmsPathPolicy() {
    return request<ApiResponse<SmsPathPolicy>>('/sms/path-policy')
  }

  async setSmsPathPolicy(policy: SmsPathPolicy) {
    return request<ApiResponse<SmsPathPolicy>>('/sms/path-policy', {
      method: 'POST',
      body: JSON.stringify(policy),
    })
  }

  async clearAllSms() {
    return request<ApiResponse<Record<string, never>>>('/sms/clear', {
      method: 'POST',
    })
  }

  async deleteSmsMessage(id: number) {
    return request<ApiResponse<{ deleted: number }>>(`/sms/message/${id}`, {
      method: 'DELETE',
    })
  }

  async deleteSmsConversation(phoneNumber: string, channelId?: string) {
    const query = channelId ? `?channel_id=${encodeURIComponent(channelId)}` : ''
    return request<ApiResponse<{ deleted: number }>>(
      `/sms/conversation/${encodeURIComponent(phoneNumber)}${query}`,
      {
        method: 'DELETE',
      },
    )
  }

  async deleteSmsBatch(payload: { ids?: number[]; phone_numbers?: string[]; channel_id?: string }) {
    return request<ApiResponse<{ deleted: number }>>('/sms/batch-delete', {
      method: 'POST',
      body: JSON.stringify(payload),
    })
  }

  async getCalls() {
    return request<ApiResponse<CallListResponse>>('/calls')
  }

  async dialCall(phoneNumber: string) {
    return request<ApiResponse<{ path: string }>>('/call/dial', {
      method: 'POST',
      body: JSON.stringify({ phone_number: phoneNumber }),
    })
  }

  async hangupCall(path: string) {
    return request<ApiResponse<Record<string, never>>>('/call/hangup', {
      method: 'POST',
      body: JSON.stringify({ path }),
    })
  }

  async hangupAllCalls() {
    return request<ApiResponse<Record<string, never>>>('/call/hangup-all', {
      method: 'POST',
      body: JSON.stringify({}),
    })
  }

  async answerCall(path: string) {
    return request<ApiResponse<Record<string, never>>>('/call/answer', {
      method: 'POST',
      body: JSON.stringify({ path }),
    })
  }

  async getCallHistory(params?: { limit?: number; offset?: number }) {
    const query = new URLSearchParams()
    if (params?.limit) query.append('limit', params.limit.toString())
    if (params?.offset) query.append('offset', params.offset.toString())
    const queryStr = query.toString() ? `?${query.toString()}` : ''
    return request<ApiResponse<CallHistoryResponse>>(`/call/history${queryStr}`)
  }

  async deleteCallRecord(id: number) {
    return request<ApiResponse<Record<string, never>>>(`/call/history/${id}`, {
      method: 'DELETE',
    })
  }

  async clearCallHistory() {
    return request<ApiResponse<Record<string, never>>>('/call/history/clear', {
      method: 'POST',
    })
  }

  async getVoicePathPolicy() {
    return request<ApiResponse<VoicePathPolicy>>('/voice/path-policy')
  }

  async setVoicePathPolicy(policy: VoicePathPolicy) {
    return request<ApiResponse<VoicePathPolicy>>('/voice/path-policy', {
      method: 'POST',
      body: JSON.stringify(policy),
    })
  }

  async getWebCallCapabilities() {
    return request<ApiResponse<WebCallCapabilitiesResponse>>('/web-call/capabilities')
  }

  async getVolteVoiceStatus() {
    return request<ApiResponse<VolteVoiceStatusResponse>>('/volte/call/status')
  }

  async setVolteVoice(enabled: boolean) {
    return request<ApiResponse<VolteVoiceStatusResponse>>('/volte/voice', {
      method: 'POST',
      body: JSON.stringify({ enabled }),
    })
  }

  async getVilteStatus() {
    return request<ApiResponse<VilteStatusResponse>>('/vilte/control')
  }

  async setVilteFeature(enabled: boolean) {
    return request<ApiResponse<VilteStatusResponse>>('/vilte/control', {
      method: 'POST',
      body: JSON.stringify({ enabled }),
    })
  }

  async setVilteConfig(config: VilteConfig) {
    return request<ApiResponse<VilteStatusResponse>>('/vilte/config', {
      method: 'POST',
      body: JSON.stringify(config),
    })
  }

  async getCallSettings() {
    return request<ApiResponse<CallSettingsResponse>>('/call/settings')
  }

  async setCallWaiting(enabled: boolean) {
    return request<ApiResponse<Record<string, never>>>('/call/settings', {
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

  async getNotificationLogs(params?: { type?: string; status?: string; q?: string; start_date?: string; end_date?: string; limit?: number; offset?: number }) {
    const query = new URLSearchParams()
    if (params?.type) query.append('type', params.type)
    if (params?.status) query.append('status', params.status)
    if (params?.q) query.append('q', params.q)
    if (params?.start_date) query.append('start_date', params.start_date)
    if (params?.end_date) query.append('end_date', params.end_date)
    if (params?.limit) query.append('limit', params.limit.toString())
    if (params?.offset) query.append('offset', params.offset.toString())
    const queryStr = query.toString() ? `?${query.toString()}` : ''
    return request<ApiResponse<NotificationLogsResponse>>(`/notifications/logs${queryStr}`)
  }

  async clearNotificationLogs(filters?: { type?: string; status?: string; start_date?: string; end_date?: string }) {
    return request<ApiResponse<{ deleted: number }>>('/notifications/logs/clear', {
      method: 'POST',
      body: JSON.stringify(filters ?? {}),
    })
  }

  async getNotificationQueue(params?: { limit?: number }) {
    const query = new URLSearchParams()
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

  async saveVowifiCarrierProfile(record: CarrierProfileRecord) {
    return request<ApiResponse<{ profile_id: string; plmn: string; e911_expected: boolean }>>(
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

  async getVowifiProfile() {
    return request<ApiResponse<VowifiProfileMatchResponse>>('/vowifi/profile', {
      timeoutMs: 10000,
    })
  }

  async getVowifiStatus() {
    return request<ApiResponse<VowifiStatusResponse>>('/vowifi/status', {
      timeoutMs: 30000,
    })
  }

  async getVowifiControl() {
    return request<ApiResponse<VowifiConfig>>('/vowifi/control', {
      timeoutMs: 10000,
    })
  }

  async setVowifiFeature(enabled: boolean) {
    return request<ApiResponse<VowifiConfig>>('/vowifi/feature', {
      method: 'POST',
      body: JSON.stringify({ enabled }),
      timeoutMs: 10000,
    })
  }

  async setVowifiConnection(enabled: boolean) {
    return request<ApiResponse<VowifiStatusResponse>>('/vowifi/connection', {
      method: 'POST',
      body: JSON.stringify({ enabled }),
      timeoutMs: 120000,
    })
  }

  async connectVowifi() {
    return request<ApiResponse<VowifiStatusResponse>>('/vowifi/connect', {
      method: 'POST',
      timeoutMs: 120000,
    })
  }

  async getVowifiDiagnostics(options: { limit?: number; traceId?: string } = {}) {
    const query = new URLSearchParams()
    query.set('limit', String(options.limit ?? 50))
    const traceId = options.traceId?.trim()
    if (traceId) query.set('trace_id', traceId)
    const suffix = query.toString()
    return request<ApiResponse<VowifiDiagnosticsResponse>>(`/vowifi/diagnostics${suffix ? `?${suffix}` : ''}`, {
      timeoutMs: 30000,
    })
  }

  async getVowifiEvents(limit = 50, traceId?: string) {
    const query = new URLSearchParams()
    query.set('limit', String(limit))
    const filter = traceId?.trim()
    if (filter) query.set('trace_id', filter)
    return request<ApiResponse<VowifiRuntimeEventsResponse>>(`/vowifi/events?${query.toString()}`, {
      timeoutMs: 10000,
    })
  }

  async getVowifiSmsDeliveries(limit = 20) {
    return request<ApiResponse<VowifiSmsDeliveriesResponse>>(`/vowifi/sms/delivery?limit=${limit}`, {
      timeoutMs: 10000,
    })
  }

  async getVowifiSoakRuns(limit = 20) {
    return request<ApiResponse<VowifiSoakRunsResponse>>(`/vowifi/soak?limit=${limit}`, {
      timeoutMs: 10000,
    })
  }

  async getVowifiEsimRestore() {
    return request<ApiResponse<VowifiEsimRestoreEntry | null>>('/vowifi/esim-restore/status', {
      timeoutMs: 10000,
    })
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

  async getAutomationLogs(params?: { type?: string; status?: string; start_date?: string; end_date?: string; q?: string; limit?: number; offset?: number }) {
    const query = new URLSearchParams()
    if (params?.type) query.append('type', params.type)
    if (params?.status) query.append('status', params.status)
    if (params?.q) query.append('q', params.q)
    if (params?.start_date) query.append('start_date', params.start_date)
    if (params?.end_date) query.append('end_date', params.end_date)
    if (params?.limit) query.append('limit', params.limit.toString())
    if (params?.offset) query.append('offset', params.offset.toString())
    const queryStr = query.toString() ? `?${query.toString()}` : ''
    return request<ApiResponse<AutomationLogsResponse>>(`/automation/logs${queryStr}`)
  }

  async clearAutomationLogs(filters?: { type?: string; status?: string; start_date?: string; end_date?: string }) {
    return request<ApiResponse<{ deleted: number }>>('/automation/logs/clear', {
      method: 'POST',
      body: JSON.stringify(filters ?? {}),
    })
  }
}

export const api = new SimAdminCurrentAPI()

export * from './types'
