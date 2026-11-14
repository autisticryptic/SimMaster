import { test, expect, type Page } from '@playwright/test'
import type { JegotripLoginInput } from '../src/api/current'
const uuid = 'D5C0199B-1644-4D33-8912-B8BD5AB49224'
async function mockCommunication(page: Page, lost = false) {
  const logins: JegotripLoginInput[] = []; const writes: string[] = []
  let final: Record<string, unknown> | undefined
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname
    if (path === '/api/jegotrip/login/setup') { await route.fulfill({ json: { status: 'ok', data: { implemented: true, deployment_present: true, accepting: false,
      accepting_authenticate: true, native_configured: false, configuration_source: 'database', configuration_revision: 1,
      communication_actions: ['query_profile', 'query_phone_service', 'query_display_status', 'query_forwarding_state', 'set_phone_service'] } } }); return }
    if (path === '/api/jegotrip/app-version') { await route.fulfill({ json: { status: 'ok', data: { stored: { version: '8.9.4.0', source: 'manual', observed_at: 1000 }, stored_age_seconds: 0,
      store_ttl_seconds: 21600, configured_file: false, configured_mobile_page: false, configured_ios_lookup: false, configured_static: true, store_enabled: true } } }); return }
    if (path === '/api/jegotrip/accounts') { await route.fulfill({ json: { status: 'ok', data: [] } }); return }
    if (path === '/api/jegotrip/device-identities' && request.method() === 'GET') { await route.fulfill({ json: { status: 'ok', data: { initialized: true, configured_mode: 'store', max_identities: 64, identities: [] } } }); return }
    if (path === '/api/jegotrip/push/status') { await route.fulfill({ json: { status: 'ok', data: { initialized: true, observed_at_unix_seconds: 1000,
      configuration: { runtime_template_present: false, sms_receiver_present: false, call_receiver_present: false, call_control_present: false, call_media_present: false },
      implementation: { online_native_sms: true, online_native_calls: true, browser_event_stream: true, platform_offline_wakeup: false, sms_notification_forwarding: true, call_notification_forwarding: false },
      native_accounts: [], stored_observations: { app_sms_records: 0, app_call_records: 0, retained_inbox_records: 0, pending_sms_notifications: 0 }, warnings: ['call_media_missing', 'no_armed_sms_receiver', 'no_armed_call_receiver'], external_delivery_verification: 'requires_external_sms_call_test' } } }); return }
    if (path === '/api/jegotrip/login/attempts' && request.method() === 'POST') {
      const input = request.postDataJSON() as JegotripLoginInput; logins.push(input)
      expect(input.intent).toBe('communication'); expect(input.device_id).toBe(uuid); expect(input.start_receiver).toBe(false); expect(input.expected_configuration_revision).toBe(1)
      expect(input.communication_operation).toEqual(lost ? { kind: 'set_phone_service', status: 3 } : { kind: 'query_profile' }); expect(input.write_consent).toBe(lost)
      final = { attempt_id: input.attempt_id, intent: 'communication', phase: lost ? 'failed' : 'completed', business_state: 'not_requested', completed: true,
        line_id: null, account_id: '11111111111111111111111111111111', masked_number: '***0123', error: lost ? 'app_login_transport_failed' : null, server_code: null, created_at: 1000, updated_at: 1001,
        communication: { action: input.communication_operation?.kind, state: lost ? 'request_uncertain' : 'observed', configuration_revision: 1, device_identity_id: '22222222222222222222222222222222', device_identity_revision: 4,
          observed_at: 1001, error: lost ? 'app_login_transport_failed' : null, server_code: null, status: lost ? null : { phone_server_status: null, cs_forward: null, jegom_status: null,
            profile: { allow_mo_voice: 1, allow_mt_voice: 0, mo_voice_limit: -1, mt_voice_limit: 900, forbidden_countries: [{ country_code: 'GB', region_id: '9223372036854775807' }], answer_forbidden_countries: [] } } } }
      if (lost) { await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_transport_failed' } }); return }
      await route.fulfill({ status: 202, json: { status: 'ok', data: { ...final, completed: false, phase: 'queued', communication: null } } }); return
    }
    if (path.startsWith('/api/jegotrip/login/attempts/') && final) { expect(path.endsWith(String(final.attempt_id))).toBe(true); await route.fulfill({ json: { status: 'ok', data: final } }); return }
    if (!['GET', 'HEAD'].includes(request.method())) writes.push(path)
    await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } })
  })
  return { logins, writes }
}
async function selectCommunication(page: Page, action: string) {
  await page.getByRole('combobox', { name: /^本次登录的设备标识(?:\s|$)/ }).click(); await page.getByRole('option', { name: '复用手机 App 已授信 UUID', exact: true }).click()
  await page.getByRole('textbox', { name: '手机 App 授信 UUID', exact: true }).fill(uuid)
  await page.getByRole('combobox', { name: /^托管操作(?:\s|$)/ }).click(); await page.getByRole('option', { name: '原App通信查询 / 明确业务操作（不启动SDK）', exact: true }).click()
  await page.getByRole('combobox', { name: /^本次通信操作(?:\s|$)/ }).click(); await page.getByRole('option', { name: action, exact: true }).click()
  await page.getByRole('textbox', { name: '国家/地区区号', exact: true }).fill('44')
  await page.getByRole('textbox', { name: '已绑定手机号', exact: true }).fill('7700900123'); await page.getByRole('textbox', { name: '无忧行密码', exact: true }).fill('communication-fixture-password')
}
test('desktop communication profile query works without native configuration and never claims receiver or outbound readiness', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1000 }); const model = await mockCommunication(page); const errors: string[] = []; page.on('pageerror', (error) => errors.push(error.message))
  await page.goto('/e2e/fixtures/jegotrip-device.html'); await selectCommunication(page, '查询通话配置')
  const submit = page.getByRole('button', { name: '执行本次通信操作', exact: true }); await expect(submit).toBeDisabled()
  await page.getByRole('checkbox', { name: /我拥有此原App UUID、账号及号码/ }).check(); await expect(submit).toBeEnabled()
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/communication-query-desktop-before.png', fullPage: true, animations: 'disabled' }); await submit.click()
  await expect(page.getByText(/查询通话配置：本次查询已返回；不代表设备授信、SDK或收件成功/)).toBeVisible()
  await expect(page.getByText('mo_voice_limit：-1（原配置观察，不授权外呼或接听）', { exact: true })).toBeVisible()
  await expect(page.getByText(/外呼限制国家：GB/)).toBeVisible(); await expect(page.getByRole('textbox', { name: '无忧行密码', exact: true })).toHaveValue('')
  expect(model.logins).toHaveLength(1); expect(model.writes).toEqual([]); expect(errors).toEqual([])
  for (const key of ['sso_token', 'device_sync_profile', 'enable_hosting', 'sip_password', 'phone_login_token']) expect(key in model.logins[0]).toBe(false)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/communication-query-desktop-after.png', fullPage: true, animations: 'disabled' })
})
test('mobile communication write requires two consents and recovers a lost response by reading the same uncertain task only', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 }); const model = await mockCommunication(page, true); const errors: string[] = []; page.on('pageerror', (error) => errors.push(error.message))
  await page.goto('/e2e/fixtures/jegotrip-device.html'); await selectCommunication(page, '设置电话服务原状态码'); await page.getByRole('spinbutton', { name: '电话服务原状态整数', exact: true }).fill('3')
  const submit = page.getByRole('button', { name: '执行本次通信操作', exact: true }); const owner = page.getByRole('checkbox', { name: /我拥有此原App UUID、账号及号码/ }); const writes = page.getByRole('checkbox', { name: /我明确批准本次远端业务写操作/ })
  await owner.check(); await expect(submit).toBeDisabled(); await writes.check(); await expect(submit).toBeEnabled()
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/communication-write-mobile-before.png', fullPage: true, animations: 'disabled' }); await submit.click()
  await expect(page.getByText(/设置电话服务原状态码：请求结果不确定，不自动重发/)).toBeVisible(); await expect(submit).toBeDisabled()
  await expect(owner).not.toBeChecked(); await expect(writes).not.toBeChecked(); await expect(page.getByRole('textbox', { name: '无忧行密码', exact: true })).toHaveValue('')
  expect(model.logins).toHaveLength(1); expect(model.writes).toEqual([]); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/communication-write-mobile-after.png', fullPage: true, animations: 'disabled' })
})
