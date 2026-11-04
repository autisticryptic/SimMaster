import { test, expect, type Page } from '@playwright/test'
import type { JegotripConfigurationView, JegotripDeviceIdentityInput, JegotripDeviceIdentityState, JegotripLoginInput } from '../src/api/current'

declare global {
  interface Window {
    jegotripFileReads?: { pending: Array<(value: string) => void> }
  }
}

async function mockApis(page: Page) {
  const state: JegotripDeviceIdentityState = { initialized: true, configured_mode: 'store', max_identities: 64, identities: [] }
  const writes: JegotripDeviceIdentityInput[] = []
  const logins: string[] = []
  const checks: JegotripLoginInput[] = []
  const configuration: JegotripConfigurationView = { initialized: true, configuration_source: 'database', revision: 3,
    http_configured: true, native_configured: false, accepting_authenticate: true, accepting_enable_hosting: false }
  const configWrites: Array<{ expected_revision: number; configuration: unknown }> = []
  const otherWrites: string[] = []
  // Match root API paths only; /src/api/current.ts is a JavaScript module.
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname
    if (path === '/api/jegotrip/login/setup') {
      await route.fulfill({ json: { status: 'ok', data: { implemented: true, accepting: false, accepting_authenticate: true, native_configured: false,
        configuration_source: 'unconfigured', configuration_revision: 0 } } }); return
    }
    if (path === '/api/jegotrip/app-version') {
      await route.fulfill({ json: { status: 'ok', data: { stored: { version: '8.9.8.0', source: 'manual', observed_at: 1000 }, stored_age_seconds: 0,
        store_ttl_seconds: 21600, configured_file: false, configured_mobile_page: false, configured_ios_lookup: true, configured_static: false, store_enabled: true } } }); return
    }
    if (path === '/api/jegotrip/push/status') {
      await route.fulfill({ json: { status: 'ok', data: {
        initialized: true, observed_at_unix_seconds: 1000,
        configuration: { runtime_template_present: true, sms_receiver_present: false, sip_request_receiver_selected: false,
          call_receiver_present: false, call_control_present: false, call_media_present: false, reconnect_configured: false },
        implementation: { online_native_sms: true, online_native_calls: true, browser_event_stream: true,
          platform_offline_wakeup: false, sms_notification_forwarding: true, call_notification_forwarding: false },
        native_accounts: [], stored_observations: { app_sms_records: 0, app_call_records: 0, retained_inbox_records: 0, pending_sms_notifications: 0 },
        warnings: ['sms_receiver_missing_or_unsupported', 'call_receiver_missing', 'call_control_missing', 'call_media_missing',
          'native_accounts_absent', 'no_armed_sms_receiver', 'no_armed_call_receiver', 'platform_offline_wakeup_unimplemented', 'call_notifications_unimplemented'],
        receiver_namespace_verification: 'requires_original_binding_evidence', external_delivery_verification: 'requires_external_sms_call_test',
      } } }); return
    }
    if (path === '/api/jegotrip/config' && request.method() === 'GET') {
      await route.fulfill({ json: { status: 'ok', data: configuration } }); return
    }
    if (path === '/api/jegotrip/config' && request.method() === 'PUT') {
      const input = request.postDataJSON() as { expected_revision: number; configuration: unknown }
      configWrites.push(input)
      if (input.expected_revision !== configuration.revision) {
        await route.fulfill({ status: 409, json: { status: 'error', message: 'app_login_configuration_conflict' } }); return
      }
      configuration.revision += 1
      await route.fulfill({ json: { status: 'ok', data: configuration } }); return
    }
    if (path === '/api/jegotrip/accounts') { await route.fulfill({ json: { status: 'ok', data: [] } }); return }
    if (path === '/api/jegotrip/device-identities' && request.method() === 'GET') {
      await route.fulfill({ json: { status: 'ok', data: state } }); return
    }
    if (path === '/api/jegotrip/device-identities' && request.method() === 'PUT') {
      const input = request.postDataJSON() as JegotripDeviceIdentityInput
      writes.push(input)
      if (input.id) {
        const row = state.identities.find((item) => item.id === input.id)
        if (!row || input.expected_revision !== row.revision) {
          await route.fulfill({ status: 409, json: { status: 'error', message: 'app_login_device_identity_revision_conflict' } }); return
        }
        row.device_id = input.device_id ?? 'D5C0199B-1644-4D33-8912-B8BD5AB49224'
        row.source = input.device_id ? 'manual' : 'generated'; row.revision += 1
      } else {
        state.identities.push({ id: '0123456789abcdef0123456789abcdef', device_id: input.device_id ?? 'D5C0199B-1644-4D33-8912-B8BD5AB49224',
          source: input.device_id ? 'manual' : 'generated', revision: 1, bound_account_id: null, masked_number: null,
          reserved: false, in_use: false, created_at: 1000, updated_at: 1000 })
      }
      await route.fulfill({ json: { status: 'ok', data: state } }); return
    }
    if (path === '/api/jegotrip/login/attempts' && request.method() === 'POST') {
      const input = request.postDataJSON() as JegotripLoginInput
      if (input.intent === 'check_device_trust') {
        checks.push(input)
        const row = state.identities[0]
        await route.fulfill({ json: { status: 'ok', data: { attempt_id: input.attempt_id, phase: 'completed', business_state: 'not_requested',
          completed: true, line_id: null, masked_number: '***0123', error: null, server_code: null, created_at: 1000, updated_at: 1001,
          intent: 'check_device_trust', account_id: '11111111111111111111111111111111', device_trust: {
            device_identity_id: row.id, device_identity_revision: row.revision, observed_at: 1001,
            security_verify: { state: 'observed', authentication_code: 1, error: null, server_code: null },
            existing_device: { state: 'observed', authentication_code: 1, error: null, server_code: null },
            hosting_enabled: false, business_query_error: null, business_query_server_code: null,
          } } } }); return
      }
      logins.push(path)
    }
    if (!['GET', 'HEAD'].includes(request.method())) otherWrites.push(path)
    await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } })
  })
  return { state, writes, logins, checks, configWrites, otherWrites }
}

async function checkInstallationGuide(page: Page, model: Awaited<ReturnType<typeof mockApis>>, label: 'desktop' | 'mobile') {
  await page.getByRole('button', { name: '无忧行安装配置', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '无忧行安装配置（独立数据库表）', exact: true })
  const expand = dialog.getByRole('button', { name: '展开高级安装配置', exact: true })
  const editor = dialog.getByRole('textbox', { name: '写入新安装配置（旧值不回显）', exact: true })
  const save = dialog.getByRole('button', { name: '保存到数据库', exact: true })
  await expect(dialog.getByText('常规使用的最小输入', { exact: true })).toBeVisible()
  await expect(dialog.getByText(/不需要手机硬件 IMEI、型号或模拟器/)).toBeVisible()
  await expect(dialog.getByText(/不是一串能替代所有配置的永久代码/)).toBeVisible()
  await expect(expand).toHaveAttribute('aria-expanded', 'false')
  await expect(editor).not.toBeAttached()
  await expect(save).toBeDisabled()
  await dialog.screenshot({ path: `test-results/jegotrip-device-ui/${label}-configuration.png`, animations: 'disabled' })
  await expand.click()
  await editor.fill('{"private":"TEST-ONLY-INPUT"}')
  await dialog.getByRole('button', { name: '收起高级安装配置并清空输入', exact: true }).click()
  await expect(editor).not.toBeAttached()
  await expand.click()
  await expect(editor).toHaveValue('')

  // Deterministic delayed File.text(): older reads must not repopulate secrets
  // after typing or collapsing/reopening. No arbitrary sleeps or real backend.
  await page.evaluate(() => {
    const original = (file: File) => Blob.prototype.text.call(file)
    const pending: Array<(value: string) => void> = []
    window.jegotripFileReads = { pending }
    File.prototype.text = function (this: File) {
      if (this.name.startsWith('delayed-')) return new Promise<string>((resolve) => { pending.push(resolve) })
      return original(this)
    }
  })
  const file = dialog.locator('input[type="file"]')
  await file.setInputFiles({ name: 'delayed-typed.json', mimeType: 'application/json', buffer: Buffer.from('{}') })
  await expect.poll(() => page.evaluate(() => window.jegotripFileReads?.pending.length)).toBe(1)
  await editor.fill('{"private":"NEWER-TYPED-INPUT"}')
  await page.evaluate(() => { window.jegotripFileReads?.pending.shift()?.('{"private":"STALE-FILE-INPUT"}') })
  await expect(editor).toHaveValue('{"private":"NEWER-TYPED-INPUT"}')
  await file.setInputFiles({ name: 'delayed-hidden.json', mimeType: 'application/json', buffer: Buffer.from('{}') })
  await expect.poll(() => page.evaluate(() => window.jegotripFileReads?.pending.length)).toBe(1)
  await dialog.getByRole('button', { name: '收起高级安装配置并清空输入', exact: true }).click()
  await expand.click()
  await page.evaluate(() => { window.jegotripFileReads?.pending.shift()?.('{"private":"STALE-HIDDEN-INPUT"}') })
  await expect(editor).toHaveValue('')
  await file.setInputFiles({ name: 'oversized.json', mimeType: 'application/json', buffer: Buffer.alloc(256 * 1024 + 1, 32) })
  await expect(dialog.getByText('文件超过 256 KiB。', { exact: true })).toBeVisible()
  await expect(save).toBeDisabled()
  expect(model.configWrites).toEqual([])

  // The mock proves UI/CAS transport only. This fragment is NOT a valid private
  // deployment and never grants native readiness or provider authorization.
  const payload = { version: 1, http: { device_identity_mode: 'store' }, runtime: null }
  await editor.fill(JSON.stringify(payload))
  await save.click()
  await expect(editor).toHaveValue('')
  expect(model.configWrites).toEqual([{ expected_revision: 3, configuration: payload }])
  await expect(save).toBeDisabled()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await dialog.screenshot({ path: `test-results/jegotrip-device-ui/${label}-configuration-advanced.png`, animations: 'disabled' })
  await dialog.getByRole('button', { name: '关闭', exact: true }).click()
  await page.getByRole('button', { name: '无忧行安装配置', exact: true }).click()
  await expect(expand).toHaveAttribute('aria-expanded', 'false')
  await expect(editor).not.toBeAttached()
  await dialog.getByRole('button', { name: '关闭', exact: true }).click()
  expect(model.otherWrites).toEqual([]); expect(model.logins).toEqual([])
}

async function checkPhoneTrustTransfer(page: Page, model: Awaited<ReturnType<typeof mockApis>>, label: 'desktop' | 'mobile') {
  const value = 'D5C0199B-1644-4D33-8912-B8BD5AB49224'
  const originalWrites = model.writes.length
  if (model.state.identities.length) await page.getByRole('button', { name: '设置', exact: true }).click()
  await page.getByRole('button', { name: '录入手机 App 授信 UUID', exact: true }).click()
  const savedUuid = page.getByRole('textbox', { name: '待保存的手机 App UUID', exact: true })
  await savedUuid.fill('0123456789abcdef0123456789abcdef')
  await expect(savedUuid).toHaveAttribute('aria-invalid', 'true')
  const saveName = model.state.identities.length ? '保存新标识' : '保存手工标识'
  await expect(page.getByRole('button', { name: saveName, exact: true })).toBeDisabled()
  await savedUuid.fill(value)
  if (model.state.identities.length) page.once('dialog', (dialog) => { void dialog.accept() })
  await page.getByRole('button', { name: saveName, exact: true }).click()
  await expect(page.getByText(value, { exact: true })).toBeVisible()
  expect(model.writes.length).toBe(originalWrites + 1)
  expect(model.writes.at(-1)?.device_id).toBe(value)
  await page.getByRole('combobox', { name: /^本次登录的设备标识(?:\s|$)/ }).click()
  await page.getByRole('option', { name: '复用手机 App 已授信 UUID', exact: true }).click()
  await page.getByRole('textbox', { name: '手机 App 授信 UUID', exact: true }).fill(value)
  await page.getByRole('combobox', { name: /^托管操作(?:\s|$)/ }).click()
  await page.getByRole('option', { name: '只查询设备授信（不建接收账号、不改业务、不启动接收）', exact: true }).click()
  const receiver = page.getByRole('checkbox', { name: '业务确认后立即启动无忧行接收器（默认开启）', exact: true })
  await expect(receiver).not.toBeChecked(); await expect(receiver).toBeDisabled()
  await page.getByRole('textbox', { name: '已绑定手机号', exact: true }).fill('7700900123')
  await page.locator('input[type="password"]').fill('TEST-ONLY-PASSWORD')
  await page.getByRole('checkbox', { name: '我拥有此账号及号码，仅授权登录和查询设备授信/业务状态', exact: true }).check()
  await page.getByRole('button', { name: '查询设备授信', exact: true }).click()
  await expect(page.getByText('服务器设备授信查询（历史观察）', { exact: true })).toBeVisible()
  await expect(page.getByText(/已有设备查询：服务器本次返回 1/)).toBeVisible()
  await expect(page.getByText(/托管业务读回：该次读回未开启/)).toBeVisible()
  await expect(page.getByText(/本次不创建接收账号，不代表托管或收件成功/)).toBeVisible()
  await expect(page.locator('input[type="password"]')).toHaveValue('')
  expect(model.checks).toHaveLength(1)
  expect(model.checks[0]).toMatchObject({ intent: 'check_device_trust', consent: true, start_receiver: false, device_id: value })
  expect(Object.keys(model.checks[0]).sort()).toEqual(['attempt_id', 'consent', 'country_code', 'device_id', 'intent', 'password', 'start_receiver', 'username'])
  expect(model.logins).toEqual([]); expect(model.otherWrites).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: `test-results/jegotrip-device-ui/${label}-phone-trust.png`, fullPage: true })
}

test('device UI persists the automatic identity and supports a manual CAS update after reopening', async ({ page }) => {
  const model = await mockApis(page)
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  await expect(page.getByRole('combobox', { name: '本次登录的设备标识' })).toContainText('自动生成 / 复用已保存标识')
  await expect(page.getByRole('combobox', { name: '标识生成方式' })).toContainText('随机生成 UUID')
  await page.getByRole('button', { name: '生成并保存随机标识', exact: true }).click()
  await expect(page.getByText('D5C0199B-1644-4D33-8912-B8BD5AB49224', { exact: true })).toBeVisible()
  expect(model.writes[0]).toEqual({ id: null, expected_revision: 0, account_id: null, device_id: null })
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await page.getByRole('textbox', { name: '设备标识', exact: true }).fill('OWN-MANUAL-DEVICE-01')
  page.once('dialog', (dialog) => { void dialog.accept() })
  await page.getByRole('button', { name: '保存新标识', exact: true }).click()
  await expect(page.getByText('OWN-MANUAL-DEVICE-01', { exact: true })).toBeVisible()
  expect(model.writes[1].expected_revision).toBe(1)
  expect(model.writes[1].device_id).toBe('OWN-MANUAL-DEVICE-01')
  await page.reload()
  await expect(page.getByText('OWN-MANUAL-DEVICE-01', { exact: true })).toBeVisible()
  await expect(page.getByText(/手工指定 · 版本 2/)).toBeVisible()
  await expect(page.getByText('短信 / 来电推送诊断', { exact: true })).toBeVisible()
  await expect(page.getByText(/原生接收账号：0 · 在线短信接收器：0 · 在线来电接收器：0/)).toBeVisible()
  await expect(page.getByText(/来电的第三方通知转发尚未接线/)).toBeVisible()
  await checkInstallationGuide(page, model, 'desktop')
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/desktop.png', fullPage: true })
  expect(model.logins).toEqual([]) // Rendering/CRUD is not a provider login or a trust grant.
  await checkPhoneTrustTransfer(page, model, 'desktop')
})

test('mobile device UI validates a manual value and never claims verification or hosting success', async ({ page }) => {
  const model = await mockApis(page)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  await page.getByRole('combobox', { name: '本次登录的设备标识' }).click()
  await page.getByRole('option', { name: '手工指定自己的标识', exact: true }).click()
  const input = page.getByRole('textbox', { name: '首次登录使用的设备标识', exact: true })
  await input.fill('invalid:device')
  await expect(input).toHaveAttribute('aria-invalid', 'true')
  await input.fill('OWN-MOBILE-DEVICE-01')
  await expect(input).toHaveAttribute('aria-invalid', 'false')
  await expect(page.getByText(/生成或更换标识不代表绕过一键验证/)).toBeVisible()
  await expect(page.getByRole('button', { name: '登录并启动无忧行', exact: true })).toBeDisabled()
  await expect(page.getByText(/未配置受支持的 Sip.Request 短信接收绑定/)).toBeVisible()
  const read = page.waitForRequest((request) => new URL(request.url()).pathname === '/api/jegotrip/push/status' && request.method() === 'GET')
  await page.getByRole('button', { name: '刷新推送状态', exact: true }).click()
  await read
  await expect(page.getByText(/尚无原生接收账号/)).toBeVisible()
  await checkInstallationGuide(page, model, 'mobile')
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/mobile.png', fullPage: true })
  expect(model.writes).toEqual([]); expect(model.logins).toEqual([])
  await checkPhoneTrustTransfer(page, model, 'mobile')
})
