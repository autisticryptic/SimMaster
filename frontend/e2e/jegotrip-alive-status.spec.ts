import { test, expect } from '@playwright/test'

test('independent Alive status distinguishes legacy registration and blocked transport without claiming receive readiness', async ({ page }) => {
  const writes: string[] = []
  const errors: string[] = []
  let state = 'registered'
  page.on('pageerror', (error) => errors.push(error.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname
    if (request.method() !== 'GET') { writes.push(path); await route.fulfill({ status: 503, json: { status: 'error' } }); return }
    if (path === '/api/jegotrip/login/setup') {
      await route.fulfill({ json: { status: 'ok', data: { implemented: true, deployment_present: true, accepting: true, accepting_authenticate: true, native_configured: true, accepting_local_receiver: true } } }); return
    }
    if (path === '/api/jegotrip/app-version') {
      await route.fulfill({ json: { status: 'ok', data: { stored: { version: '8.9.4.0', source: 'manual', observed_at: 1000 }, stored_age_seconds: 0, store_ttl_seconds: 21600,
        configured_file: false, configured_mobile_page: false, configured_ios_lookup: false, configured_static: true, store_enabled: true } } }); return
    }
    if (path === '/api/jegotrip/accounts') { await route.fulfill({ json: { status: 'ok', data: [] } }); return }
    if (path === '/api/jegotrip/device-identities') {
      await route.fulfill({ json: { status: 'ok', data: { initialized: true, configured_mode: 'store', max_identities: 64, identities: [] } } }); return
    }
    if (path === '/api/jegotrip/push/status') {
      await route.fulfill({ json: { status: 'ok', data: { initialized: true, observed_at_unix_seconds: 1000,
        configuration: { runtime_template_present: true, sms_receiver_present: true, sip_request_receiver_selected: true, call_receiver_present: true, call_control_present: false, call_media_present: false, reconnect_configured: false },
        implementation: { online_native_sms: true, online_native_calls: true, browser_event_stream: true, platform_offline_wakeup: false, sms_notification_forwarding: true, call_notification_forwarding: false },
        native_accounts: [{ line_id: 'app-account-0123456789abcdef0123456789abcdef', provider_id: 'app_voice', revision: 1, phase: 'hosting_pending', io_cleanup_required: true,
          incoming_message_ready: false, incoming_call_ready: false, outbound_authorized: false, account_alive_state: state }],
        stored_observations: { app_sms_records: 0, app_call_records: 0, retained_inbox_records: 0, pending_sms_notifications: 0 },
        warnings: state === 'blocked' ? ['account_alive_transport_not_approved'] : [], receiver_namespace_verification: 'requires_original_binding_evidence', external_delivery_verification: 'requires_external_sms_call_test' } } }); return
    }
    await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 })
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  await expect(page.getByText(/独立注册已确认（旧式stcp，非TLS）/)).toBeVisible()
  await expect(page.getByText(/不代表短信、来电或媒体就绪/)).toBeVisible()
  await expect(page.getByText(/原生接收账号：1 · 在线短信接收器：0 · 在线来电接收器：0/)).toBeVisible()
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/alive-registered-desktop.png', fullPage: true, animations: 'disabled' })
  state = 'blocked'
  await page.setViewportSize({ width: 390, height: 844 })
  await page.getByRole('button', { name: '刷新推送状态', exact: true }).click()
  await expect(page.getByText(/AccountAlive：未批准旧式stcp传输，未连接/)).toBeVisible()
  await expect(page.getByText(/独立Alive需要明确批准旧式非TLS传输；当前未连接/)).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/alive-blocked-mobile.png', fullPage: true, animations: 'disabled' })
  expect(writes).toEqual([])
  expect(errors).toEqual([])
})
