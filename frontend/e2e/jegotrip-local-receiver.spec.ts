import { test, expect, type Page } from '@playwright/test'
import type { JegotripLoginInput } from '../src/api/current'

const uuid = 'D5C0199B-1644-4D33-8912-B8BD5AB49224'

async function mockLocalReceiver(page: Page, lostRefusal = false) {
  const logins: JegotripLoginInput[] = []
  const writes: string[] = []
  let final: Record<string, unknown> | null = null
  let created = false
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname
    if (path === '/api/jegotrip/login/setup') {
      await route.fulfill({ json: { status: 'ok', data: { implemented: true, deployment_present: true, accepting: true,
        accepting_authenticate: true, native_configured: true, accepting_local_receiver: true, configuration_source: 'database', configuration_revision: 1 } } }); return
    }
    if (path === '/api/jegotrip/app-version') {
      await route.fulfill({ json: { status: 'ok', data: { stored: { version: '8.9.4.0', source: 'manual', observed_at: 1000 }, stored_age_seconds: 0,
        store_ttl_seconds: 21600, configured_file: false, configured_mobile_page: false, configured_ios_lookup: false, configured_static: true, store_enabled: true } } }); return
    }
    if (path === '/api/jegotrip/device-identities' && request.method() === 'GET') {
      await route.fulfill({ json: { status: 'ok', data: { initialized: true, configured_mode: 'store', max_identities: 64,
        identities: [{ id: '22222222222222222222222222222222', device_id: uuid, source: 'manual', revision: 4,
          bound_account_id: '11111111111111111111111111111111', masked_number: '***0123', reserved: false, in_use: false, created_at: 1000, updated_at: 1000 }] } } }); return
    }
    if (path === '/api/jegotrip/accounts') { await route.fulfill({ json: { status: 'ok', data: [] } }); return }
    if (path === '/api/jegotrip/push/status') {
      await route.fulfill({ json: { status: 'ok', data: { initialized: true, observed_at_unix_seconds: 1000,
        configuration: { runtime_template_present: true, sms_receiver_present: true, sip_request_receiver_selected: true,
          call_receiver_present: true, call_control_present: false, call_media_present: false, reconnect_configured: false },
        implementation: { online_native_sms: true, online_native_calls: true, browser_event_stream: true,
          platform_offline_wakeup: false, sms_notification_forwarding: true, call_notification_forwarding: false },
        native_accounts: created ? [{ line_id: 'app-account:0123456789abcdef0123456789abcdef', phase: 'stopped', incoming_message_ready: false, incoming_call_ready: false }] : [],
        stored_observations: { app_sms_records: 0, app_call_records: 0, retained_inbox_records: 0, pending_sms_notifications: 0 },
        warnings: ['call_control_missing', 'call_media_missing', 'no_armed_sms_receiver', 'no_armed_call_receiver'],
        receiver_namespace_verification: 'requires_original_binding_evidence', external_delivery_verification: 'requires_external_sms_call_test' } } }); return
    }
    if (path === '/api/jegotrip/login/attempts' && request.method() === 'POST') {
      const input = request.postDataJSON() as JegotripLoginInput
      logins.push(input)
      expect(input.intent).toBe('prepare_local_receiver')
      expect(input.device_id).toBe(uuid)
      created = !lostRefusal
      final = { attempt_id: input.attempt_id, intent: input.intent, phase: lostRefusal ? 'failed' : 'completed', business_state: 'not_requested', completed: true,
        line_id: created ? 'app-account:0123456789abcdef0123456789abcdef' : null, masked_number: '***0123',
        error: lostRefusal ? 'app_login_device_untrusted' : null, server_code: null, created_at: 1000, updated_at: 1001,
        account_id: '11111111111111111111111111111111', device_trust: {
          device_identity_id: '22222222222222222222222222222222', device_identity_revision: 4, observed_at: 1001, purpose: 'local_receiver',
          security_verify: { state: 'not_requested', authentication_code: null, error: null, server_code: null },
          existing_device: { state: 'observed', authentication_code: lostRefusal ? 2 : 1, error: null, server_code: null },
          call_status: { state: lostRefusal ? 'not_requested' : 'observed', authentication_code: lostRefusal ? null : 2, error: null, server_code: null },
          hosting_enabled: lostRefusal ? null : false, local_cs_forward: lostRefusal ? null : 1, business_query_error: null, business_query_server_code: null } }
      if (lostRefusal) {
        await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_transport_failed' } }); return
      }
      await route.fulfill({ status: 202, json: { status: 'ok', data: { ...final, phase: 'queued', completed: false, line_id: null, device_trust: null } } }); return
    }
    if (path.startsWith('/api/jegotrip/login/attempts/') && request.method() === 'GET' && final) {
      expect(path.endsWith(String(final.attempt_id))).toBe(true)
      await route.fulfill({ json: { status: 'ok', data: final } }); return
    }
    if (!['GET', 'HEAD'].includes(request.method())) writes.push(path)
    await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } })
  })
  return { logins, writes }
}

async function selectLocal(page: Page) {
  await page.getByRole('combobox', { name: /^本次登录的设备标识(?:\s|$)/ }).click()
  await page.getByRole('option', { name: '复用手机 App 已授信 UUID', exact: true }).click()
  await page.getByRole('textbox', { name: '手机 App 授信 UUID', exact: true }).fill(uuid)
  await page.getByRole('combobox', { name: /^托管操作(?:\s|$)/ }).click()
  await page.getByRole('option', { name: '准备本地SDK接收器（不改呼转、不报告设备）', exact: true }).click()
  await page.getByRole('textbox', { name: '已绑定手机号', exact: true }).fill('7700900123')
  await page.getByRole('textbox', { name: '无忧行密码', exact: true }).fill('local-fixture-password')
}

test('desktop local receiver preparation preserves forwarding and only claims an unstarted prepared account', async ({ page }) => {
  await page.setViewportSize({ width: 1280, height: 1000 })
  const errors: string[] = []; page.on('pageerror', (error) => errors.push(error.message))
  const model = await mockLocalReceiver(page)
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  await selectLocal(page)
  const submit = page.getByRole('button', { name: '准备本地接收器', exact: true })
  await expect(submit).toBeDisabled()
  const consent = page.getByRole('checkbox', { name: /授权保持原UUID、查询已有设备/ })
  await expect(consent).not.toBeChecked()
  await page.getByRole('checkbox', { name: '准备后启动本地接收器（仍须SDK注册确认）', exact: true }).uncheck()
  await consent.check(); await expect(submit).toBeEnabled(); await submit.click()
  await expect(page.getByText(/本地接收账号已准备；SDK与收件状态请查看独立账号/)).toBeVisible()
  await expect(page.getByText(/接口本次返回 2（独立呼转业务观察，不是本地SDK注册结果）/)).toBeVisible()
  await expect(page.getByText('远端呼转业务读回：该次读回未开启', { exact: true })).toBeVisible()
  await expect(page.getByText('本地准备完成不是SDK或收件成功；未修改服务端呼转，也未发送设备同步。', { exact: true })).toBeVisible()
  await expect(page.getByText(/原生接收账号：1 · 在线短信接收器：0 · 在线来电接收器：0/)).toBeVisible()
  await expect(page.getByRole('textbox', { name: '无忧行密码', exact: true })).toHaveValue('')
  await expect(consent).not.toBeChecked()
  expect(model.logins).toHaveLength(1); expect(model.logins[0].start_receiver).toBe(false)
  for (const key of ['enable_hosting', 'device_sync_profile', 'sso_token', 'trust_granted']) expect(key in model.logins[0]).toBe(false)
  expect(model.writes).toEqual([]); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/local-receiver-desktop.png', fullPage: true, animations: 'disabled' })
})

test('mobile local receiver requires explicit consent and reads a lost refusal without resubmitting or changing business', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  const errors: string[] = []; page.on('pageerror', (error) => errors.push(error.message))
  const model = await mockLocalReceiver(page, true)
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  await page.getByRole('combobox', { name: /^托管操作(?:\s|$)/ }).click()
  await expect(page.getByRole('option', { name: '准备本地SDK接收器（不改呼转、不报告设备）', exact: true })).toHaveAttribute('aria-disabled', 'true')
  await page.keyboard.press('Escape')
  await selectLocal(page)
  const submit = page.getByRole('button', { name: '准备本地接收器', exact: true })
  const consent = page.getByRole('checkbox', { name: /授权保持原UUID、查询已有设备/ })
  await expect(submit).toBeDisabled(); await consent.check(); await submit.click()
  await expect(page.getByText(/本次未取得该原 UUID 的明确服务器认可/)).toBeVisible()
  await expect(page.getByText(/原生接收账号：0 · 在线短信接收器：0 · 在线来电接收器：0/)).toBeVisible()
  await expect(page.getByRole('textbox', { name: '无忧行密码', exact: true })).toHaveValue('')
  await expect(consent).not.toBeChecked(); await expect(submit).toBeDisabled()
  expect(model.logins).toHaveLength(1); expect(model.logins[0].start_receiver).toBe(true)
  expect(model.writes).toEqual([]); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/local-receiver-mobile.png', fullPage: true, animations: 'disabled' })
})
