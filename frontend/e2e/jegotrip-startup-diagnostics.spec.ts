import { test, expect } from '@playwright/test'

test('native startup diagnostics render redacted failure stages on desktop and mobile without login retry or ready claims', async ({ page }) => {
  const writes: string[] = []
  const errors: string[] = []
  let badInput = false
  let statusReads = 0
  page.on('pageerror', (error) => errors.push(error.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname
    if (!['GET', 'HEAD'].includes(request.method())) {
      writes.push(path)
      await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } }); return
    }
    if (path === '/api/jegotrip/login/setup') {
      await route.fulfill({ json: { status: 'ok', data: { implemented: true, deployment_present: true, accepting: true,
        accepting_authenticate: true, native_configured: true, accepting_local_receiver: true, configuration_source: 'database', configuration_revision: 1 } } }); return
    }
    if (path === '/api/jegotrip/app-version') {
      await route.fulfill({ json: { status: 'ok', data: { stored: { version: '8.9.4.0', source: 'manual', observed_at: 1000 }, stored_age_seconds: 0,
        store_ttl_seconds: 21600, configured_file: false, configured_mobile_page: false, configured_ios_lookup: false, configured_static: true, store_enabled: true } } }); return
    }
    if (path === '/api/jegotrip/device-identities') {
      await route.fulfill({ json: { status: 'ok', data: { initialized: true, configured_mode: 'store', max_identities: 64, identities: [] } } }); return
    }
    if (path === '/api/jegotrip/accounts') { await route.fulfill({ json: { status: 'ok', data: [] } }); return }
    if (path === '/api/jegotrip/push/status') {
      statusReads += 1
      await route.fulfill({ json: { status: 'ok', data: { initialized: true, observed_at_unix_seconds: 1000,
        configuration: { runtime_template_present: true, sms_receiver_present: true, sip_request_receiver_selected: true,
          call_receiver_present: true, call_control_present: false, call_media_present: false, reconnect_configured: false },
        implementation: { online_native_sms: true, online_native_calls: true, browser_event_stream: true,
          platform_offline_wakeup: false, sms_notification_forwarding: true, call_notification_forwarding: false },
        native_accounts: [{ line_id: 'app-account-0123456789abcdef0123456789abcdef', provider_id: 'jegotrip', revision: 1,
          phase: 'disconnected', io_cleanup_required: true, incoming_message_ready: false, incoming_call_ready: false,
          outbound_authorized: false, last_error: 'app_account_disconnected',
          startup_failure: badInput ? { phase: 'private-password', stage: 'https://private.invalid', code: 'token=secret' }
            : { phase: 'bootstrap', stage: 'router_tls', code: 'jegotrip_tls_handshake_failed' } }],
        stored_observations: { app_sms_records: 0, app_call_records: 0, retained_inbox_records: 0, pending_sms_notifications: 0 },
        warnings: ['no_armed_sms_receiver', 'no_armed_call_receiver'], receiver_namespace_verification: 'requires_original_binding_evidence',
        external_delivery_verification: 'requires_external_sms_call_test' } } }); return
    }
    await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } })
  })
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  const diagnostic = page.getByText(/启动连接诊断：Bootstrap \/ Router TLS：TLS握手或证书验证失败/)
  await expect(diagnostic).toBeVisible()
  await expect(page.getByText(/原生接收账号：1 · 在线短信接收器：0 · 在线来电接收器：0/)).toBeVisible()
  await expect(page.getByText(/disconnected · 短信未武装 · 来电未武装/)).toBeVisible()
  await expect(page.getByText('已武装，需收件测试', { exact: false })).toHaveCount(0)
  for (const [name, width, height] of [['desktop', 1280, 1000], ['mobile', 390, 844]] as const) {
    await page.setViewportSize({ width, height })
    await expect(diagnostic).toBeVisible()
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
    await page.screenshot({ path: `test-results/jegotrip-device-ui/startup-diagnostics-${name}.png`, fullPage: true, animations: 'disabled' })
  }
  const readsBeforeRefresh = statusReads
  badInput = true
  await page.getByRole('button', { name: '刷新推送状态', exact: true }).click()
  await expect(page.getByText(/启动连接诊断：来源未知 \/ 阶段未知：未识别的原生启动错误/)).toBeVisible()
  expect(statusReads).toBe(readsBeforeRefresh + 1)
  for (const privateValue of ['private-password', 'private.invalid', 'token=secret']) {
    await expect(page.getByText(privateValue, { exact: false })).toHaveCount(0)
  }
  expect(writes).toEqual([])
  expect(errors).toEqual([])
})
