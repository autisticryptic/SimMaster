import type { AppLoginAttempt, AppLoginInput, JegotripLoginInput, JegotripLoginIntent, JegotripTrustQueryObservation, JegotripDeviceSyncObservation, JegotripStartupFailure } from '../api/current'

export type AppLoginMode = 'account' | 'phone'
export interface AppLoginDraft {
  mode: AppLoginMode
  username: string
  password: string
  country: string
  consent: boolean
  startReceiver: boolean
  intent?: JegotripLoginIntent
  deviceMode?: 'auto' | 'manual' | 'phone_app'
  deviceId?: string
}

export function emptyAppLoginDraft(): AppLoginDraft {
  return { mode: 'phone', username: '', password: '', country: '86', consent: false, startReceiver: true, intent: 'enable_hosting', deviceMode: 'auto', deviceId: '' }
}

export function canSubmitAppLogin(draft: AppLoginDraft): boolean {
  const user = draft.username.trim()
  const byteLength = (value: string) => new TextEncoder().encode(value).length
  if (!draft.consent || !user || byteLength(user) > 254 || /\p{Cc}/u.test(user)
    || !draft.password || byteLength(draft.password) > 1024 || /\p{Cc}/u.test(draft.password)) return false
  return draft.mode !== 'phone'
    || (/^\+?\d{1,4}$/.test(draft.country.trim()) && /^\d{1,20}$/.test(user))
}

export function appLoginInput(attemptId: string, draft: AppLoginDraft): AppLoginInput {
  if (!/^[a-f0-9]{32}$/.test(attemptId) || !canSubmitAppLogin(draft)) throw new Error('app_login_request_invalid')
  return {
    attempt_id: attemptId,
    username: draft.username.trim(),
    password: draft.password, // The native password path does NOT trim it.
    country_code: draft.mode === 'phone' ? draft.country.trim().replace(/^\+/, '') : null,
    enable_hosting: true,
    start_receiver: draft.startReceiver,
  }
}

export function validJegotripDeviceId(value: string): boolean {
  return /^[A-Za-z0-9_-]{8,64}$/.test(value.trim())
}

export function validPhoneAppUuid(value: string): boolean {
  return /^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$/.test(value.trim())
}

export function canSubmitJegotripLogin(draft: AppLoginDraft): boolean {
  const intent = draft.intent ?? 'enable_hosting'
  const deviceMode = draft.deviceMode ?? 'auto'
  const reusing = intent === 'reuse_hosting' || intent === 'enable_trusted_hosting'
  const localReceiver = intent === 'prepare_local_receiver'
  return canSubmitAppLogin(draft)
    && (deviceMode === 'auto' || (deviceMode === 'manual' && validJegotripDeviceId(draft.deviceId ?? ''))
      || (deviceMode === 'phone_app' && validPhoneAppUuid(draft.deviceId ?? '')))
    && (!reusing || deviceMode === 'phone_app')
    && (!localReceiver || deviceMode === 'phone_app')
    && (deviceMode !== 'phone_app' || reusing || localReceiver || intent === 'authenticate' || intent === 'check_device_trust')
    && (reusing || localReceiver || intent === 'enable_hosting' || intent === 'attach_hosting'
      || ((intent === 'authenticate' || intent === 'check_device_trust') && !draft.startReceiver))
}

export function jegotripLoginInput(attemptId: string, draft: AppLoginDraft): JegotripLoginInput {
  if (!/^[a-f0-9]{32}$/.test(attemptId) || !canSubmitJegotripLogin(draft)) throw new Error('app_login_request_invalid')
  return {
    attempt_id: attemptId, username: draft.username.trim(), password: draft.password,
    country_code: draft.mode === 'phone' ? draft.country.trim().replace(/^\+/, '') : null,
    intent: draft.intent ?? 'enable_hosting', consent: draft.consent, start_receiver: draft.startReceiver,
    ...(draft.deviceMode === 'manual' || draft.deviceMode === 'phone_app' ? { device_id: (draft.deviceId ?? '').trim() } : {}),
  }
}

const errors: Record<string, string> = {
  app_login_request_invalid: '请检查账号、密码、区号并确认授权。',
  app_login_deployment_missing: '后台尚未提供无忧行部署配置。',
  app_login_deployment_invalid: '后台部署配置无效；请检查原 App 与原生连接上下文。',
  app_login_unavailable: '后台登录功能尚未初始化或未取得数据库运行锁。',
  app_login_busy: '已有登录任务正在处理，请先查看任务结果。',
  app_login_attempt_exists: '该登录任务已存在，请查看原任务；不会重复提交开通。',
  app_login_attempt_limit: '登录任务记录已达上限，需要管理员检查。',
  app_login_account_limit: '无忧行账号摘要已达上限，需要管理员检查。',
  app_login_configuration_revision_conflict: '安装配置版本已变化，请重新读取后再保存。',
  app_login_migration_conflict: '新旧无忧行任务表同时有数据，迁移已保留原记录并停止，请管理员核对。',
  app_login_native_configuration_missing: '完整原生启动配置尚未就绪，未开启托管或启动无忧行；不会退回仅保存认证摘要。',
  app_login_verification_required: '服务要求额外安全验证，尚未开启接收；不会伪造验证通过。',
  app_login_existing_business_action_required: '服务要求处理既有业务状态；程序未自动关闭原托管或呼转。',
  app_login_migration_required: '该账号需要先完成原服务的账号迁移。',
  app_login_session_revoked: 'App 会话已被服务撤销，请确认其他设备登录情况。',
  app_login_service_maintenance: '无忧行服务当前维护中。',
  app_login_phone_binding_required: '账号没有返回可用的已绑定手机号码。',
  app_login_identity_mismatch: '返回的绑定号码与本次输入或该设备标识原绑定的号码不一致，已停止，未修改托管业务。',
  app_login_native_configuration_invalid: '原生接收配置不完整或不受支持，未提交业务开通。',
  app_login_account_creation_failed: '本地账号保存失败；请先核对任务中的远端业务结果。',
  app_login_receiver_start_failed: '账号已保存，但接收器启动失败；请查看独立账号状态。',
  app_login_receiver_device_check_missing: '本地接收准备需要已有设备查询端点；当前未配置，未请求电话资料或启动SDK。',
  app_login_version_discovery_failed: '未能从 App Store 或官方页面取得版本号；请手工填写后保存。',
  app_login_device_identity_conflict: '该标识已分配或保留给另一身份，或与本账号现有标识不一致。请勿复用他人标识；更换现有标识请使用下方设备标识管理。',
  app_login_device_identity_revision_conflict: '设备标识已被其他操作修改，请刷新后再保存。',
  app_login_device_identity_not_found: '设备标识已不存在，请刷新列表。',
  app_login_device_identity_limit: '设备标识或历史保留记录已达上限，请管理员核对；不会覆盖旧绑定。',
  app_login_device_identity_in_use: '有登录任务、接收器或未完成的清理正在使用该标识，请等待任务结束并停止接收器后再修改。',
  app_login_device_untrusted: '本次未取得该原 UUID 的明确服务器认可，或服务端拒绝本机（506）；生成或更换设备标识不等于通过一键验证。已停止，未自动申请新授信。',
  app_login_business_not_enabled: '所选接管操作不写业务开关，但服务器本次读回托管尚未开启。请在本人手机开启，或确认已授信后另选明确开启业务的操作；仍以服务端结果为准。',
  app_login_device_sync_context_missing: '已配置设备同步，但缺少本次 user-info.accountid 或受支持的本人手机报告资料；未猜测字段或申请新授信，请检查安装配置。',
  app_login_business_unconfirmed: '开通请求已被接受，但未确认业务状态；未启动接收器。',
  app_login_rate_limited: '服务请求过于频繁，请勿自动重复登录。',
  app_login_server_rejected: '无忧行服务拒绝了本次请求。',
  app_login_transport_failed: '连接未完成，请核对任务结果，避免重复开通。',
  app_login_response_invalid: '服务响应与已支持的协议不匹配，已停止。',
  app_login_storage_failed: '任务状态存储失败；不要据此重复开通，请先检查后台。',
  app_login_interrupted: '任务被中断；已发出的业务请求可能仍有效，不会自动重放。',
  app_login_timeout: '任务超时，请先核对业务结果，不会自动重放开通。',
  app_login_shutting_down: '后台正在退出，未接受新的登录。',
  app_account_runtime_owner_unconfirmed: '原运行归属或清理尚未确认，不能强制接管。',
  app_account_session_revoked: '无忧行服务器已撤销本次接收会话，可能有其他设备登录；已停止，不会自动重连争抢，请先核对本人手机。',
  app_account_revision_conflict: '账号配置版本已变化，请刷新后再操作。',
}
export function appLoginError(error: unknown): string {
  const text = error instanceof Error ? error.message : typeof error === 'string' ? error : ''
  for (const [code, message] of Object.entries(errors)) if (text.includes(code)) return message
  // Never echo an unrecognized service error, URL, password or token.
  return '请求结果尚未确认，请查看后台任务与账号状态，避免重复开通。'
}
export function appBusinessLabel(state: string): string {
  return ({ not_requested: '未提交开通', request_uncertain: '开通结果不确定', accepted_unconfirmed: '服务已接受，状态待确认',
    enabled: '业务已确认开启', already_enabled: '业务原已开启', rejected: '开通请求被拒绝' } as Record<string, string>)[state] ?? '业务状态未知'
}
export function appLoginPhase(phase: string): string {
  return ({ queued: '已入队', authenticating: '登录 App 账号', reading_bound_number: '核对绑定号码', fetching_credentials: '后台取得通话凭据',
    saving_app_account: '保存最近 App 认证摘要', checking_device_trust: '查询该 UUID 的服务器授信状态', syncing_device: '同步已明确配置的本人手机设备资料',
    checking_business: '查询托管业务', enabling_business: '提交开通请求', confirming_business: '确认业务状态', creating_account: '保存独立账号',
    starting_receiver: '提交接收器启动', completed: '登录配置任务完成', failed: '任务失败', interrupted: '任务中断' } as Record<string, string>)[phase] ?? '未知任务状态'
}
export function appLoginNeedsPolling(attempt: AppLoginAttempt): boolean { return !attempt.completed && !attempt.error }

export function deviceTrustQueryLabel(observation: JegotripTrustQueryObservation): string {
  if (observation.state === 'not_configured') return '查询端点未配置，状态未知'
  if (observation.state === 'not_requested') return '本次复用流程未请求此端点'
  if (observation.state === 'failed') return `查询失败：${appLoginError(observation.error)}`
  if (observation.state !== 'observed') return '状态未知'
  if (observation.authentication_code === 1) return '服务器本次返回 1（认可该设备）'
  if (observation.authentication_code === 2) return '服务器本次返回 2（未认可该设备）'
  return '服务器未返回已支持的明确授信结论'
}

export function callForwardingQueryLabel(observation: JegotripTrustQueryObservation): string {
  if (observation.state !== 'observed') return deviceTrustQueryLabel(observation)
  const code = observation.authentication_code
  if (typeof code !== 'number' || !Number.isInteger(code) || code < 0 || code > 999999) return '接口未返回规范状态码；不推断SDK状态'
  return `接口本次返回 ${code}（独立呼转业务观察，不是本地SDK注册结果）`
}

export function localReceiverTaskLabel(attempt: AppLoginAttempt): string {
  if (attempt.error) return appLoginError(attempt.error)
  if (!attempt.completed) return appLoginPhase(attempt.phase)
  return attempt.line_id ? '本地接收账号已准备；SDK与收件状态请查看独立账号' : '本地接收任务已结束，未建立接收账号'
}

export function accountAliveLabel(value: unknown): string {
  const labels: Record<string, string> = {
    absent: '未返回独立通道', disabled: '独立通道已禁用', unsupported: '未配置受支持的独立通道',
    starting: '独立通道准备中', registering: '等待独立注册回复', registered: '独立注册已确认（旧式stcp，非TLS）',
    retrying: '独立注册失败，按原退避等待', blocked: '未批准旧式stcp传输，未连接', unapproved: '返回的独立端点未获准，未连接',
    session_invalid: '本次原生会话已失效', stopped: '独立通道已停止',
  }
  const label = typeof value === 'string' && Object.prototype.hasOwnProperty.call(labels, value) ? labels[value] : '独立通道状态未知'
  return `${label}；不代表短信、来电或媒体就绪`
}

export function startupFailureLabel(failure: unknown): string {
  const fields = typeof failure === 'object' && failure !== null ? failure as Partial<JegotripStartupFailure> : {}
  const own = (values: Record<string, string>, key: unknown) => typeof key === 'string' && Object.prototype.hasOwnProperty.call(values, key) ? values[key] : undefined
  const phase = own({ bootstrap: 'Bootstrap', account: '账号连接' }, fields.phase) ?? '来源未知'
  const stage = own({ preparation: '准备参数', attempt: '启动流程（具体阶段未确认）', router_connect: 'Router TCP/DNS', router_tls: 'Router TLS', router_control: 'Router控制',
    router_io: 'Router收发', logical_channel: '逻辑通道', common_framing: 'Common封装' }, fields.stage) ?? '阶段未知'
  const reasons: Record<string, string> = {
    jegotrip_tls_handshake_failed: 'TLS握手或证书验证失败', jegotrip_tls_deadline_expired: 'TLS截止时间已到',
    jegotrip_tls_closed: 'TLS连接已关闭', jegotrip_connector_io_failed: '连接I/O失败', jegotrip_connector_timeout: '连接超时',
    jegotrip_router_control_rejected: 'Router拒绝连接', jegotrip_router_control_protocol_failed: 'Router控制响应无效',
    jegotrip_router_control_timeout: 'Router控制超时', jegotrip_router_io_failed: 'Router收发失败', jegotrip_router_timeout: 'Router超时',
    jegotrip_logical_channel_timeout: '逻辑通道超时', jegotrip_logical_channel_session_failed: '逻辑通道建立失败',
    jegotrip_common_startup_packet_failed: 'Common封装无效', jegotrip_common_startup_control_failed: 'Common控制失败',
    jegotrip_startup_scope_invalid: '原连接归属已失效', jegotrip_startup_configuration_invalid: '准备参数无效',
    jegotrip_startup_closed: '原连接已关闭', jegotrip_startup_cancelled: '原连接准备已取消', jegotrip_startup_timeout: '准备超时',
  }
  return `${phase} / ${stage}：${own(reasons, fields.code) ?? '未识别的原生启动错误'}（仅失败阶段观察，不恢复连接或就绪）`
}

export function deviceSyncLabel(observation: JegotripDeviceSyncObservation): string {
  if (observation.state === 'not_configured') return '未配置本人手机报告，未发送'
  if (observation.state === 'not_requested') return '本次未发送设备报告'
  if (observation.state === 'accepted') return '服务器已接受本次设备报告（不是授信或收件成功）'
  if (observation.state === 'rejected') return `报告被拒绝：${appLoginError(observation.error)}`
  return '设备报告结果不确定，不会自动重放'
}
