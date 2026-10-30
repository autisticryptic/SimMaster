import type { AppLoginAttempt, AppLoginInput } from '../api/current'

export type AppLoginMode = 'account' | 'phone'
export interface AppLoginDraft {
  mode: AppLoginMode
  username: string
  password: string
  country: string
  consent: boolean
  startReceiver: boolean
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

const errors: Record<string, string> = {
  app_login_request_invalid: '请检查账号、密码、区号并确认授权。',
  app_login_deployment_missing: '后台尚未提供无忧行部署配置。',
  app_login_deployment_invalid: '后台部署配置无效；请检查原 App 与原生连接上下文。',
  app_login_unavailable: '后台登录功能尚未初始化或未取得数据库运行锁。',
  app_login_busy: '已有登录任务正在处理，请先查看任务结果。',
  app_login_attempt_exists: '该登录任务已存在，请查看原任务；不会重复提交开通。',
  app_login_attempt_limit: '登录任务记录已达上限，需要管理员检查。',
  app_login_verification_required: '服务要求额外安全验证，尚未开启接收；不会伪造验证通过。',
  app_login_existing_business_action_required: '服务要求处理既有业务状态；程序未自动关闭原托管或呼转。',
  app_login_migration_required: '该账号需要先完成原服务的账号迁移。',
  app_login_session_revoked: 'App 会话已被服务撤销，请确认其他设备登录情况。',
  app_login_service_maintenance: '无忧行服务当前维护中。',
  app_login_phone_binding_required: '账号没有返回可用的已绑定手机号码。',
  app_login_identity_mismatch: '返回的绑定号码与本次手机号登录输入不一致，已停止。',
  app_login_native_configuration_invalid: '原生接收配置不完整或不受支持，未提交业务开通。',
  app_login_account_creation_failed: '本地账号保存失败；请先核对任务中的远端业务结果。',
  app_login_receiver_start_failed: '账号已保存，但接收器启动失败；请查看独立账号状态。',
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
    checking_business: '查询托管业务', enabling_business: '提交开通请求', confirming_business: '确认业务状态', creating_account: '保存独立账号',
    starting_receiver: '提交接收器启动', completed: '登录配置任务完成', failed: '任务失败', interrupted: '任务中断' } as Record<string, string>)[phase] ?? '未知任务状态'
}
export function appLoginNeedsPolling(attempt: AppLoginAttempt): boolean { return !attempt.completed && !attempt.error }
