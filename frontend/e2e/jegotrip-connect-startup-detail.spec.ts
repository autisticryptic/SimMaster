import { test, expect } from '@playwright/test'

test('connect timeout detail shows readonly DNS TCP steps on desktop mobile with no TLS readiness or raw peer values', async ({ page }) => {
  const writes: string[] = []; const errors: string[] = []; let mode: 'dns' | 'tcp' | 'invalid' = 'dns'; let reads = 0
  page.on('pageerror', (error) => errors.push(error.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname
    if (!['GET', 'HEAD'].includes(request.method())) { writes.push(path); await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } }); return }
    if (path === '/api/jegotrip/login/setup') { await route.fulfill({ json: { status: 'ok', data: { implemented: true, deployment_present: true, accepting: true, accepting_authenticate: true, native_configured: true, accepting_local_receiver: true, configuration_source: 'database', configuration_revision: 1 } } }); return }
    if (path === '/api/jegotrip/app-version') { await route.fulfill({ json: { status: 'ok', data: { stored: { version: '8.9.4.0', source: 'manual', observed_at: 1000 }, stored_age_seconds: 0, store_ttl_seconds: 21600, configured_file: false, configured_mobile_page: false, configured_ios_lookup: false, configured_static: true, store_enabled: true } } }); return }
    if (path === '/api/jegotrip/device-identities') { await route.fulfill({ json: { status: 'ok', data: { initialized: true, configured_mode: 'store', max_identities: 64, identities: [] } } }); return }
    if (path === '/api/jegotrip/accounts') { await route.fulfill({ json: { status: 'ok', data: [] } }); return }
    if (path === '/api/jegotrip/push/status') {
      reads += 1
      await route.fulfill({ json: { status: 'ok', data: { initialized: true, observed_at_unix_seconds: 1000,
        configuration: { runtime_template_present: true, sms_receiver_present: true, sip_request_receiver_selected: true, call_receiver_present: true, call_control_present: false, call_media_present: false, reconnect_configured: false },
        implementation: { online_native_sms: true, online_native_calls: true, browser_event_stream: true, platform_offline_wakeup: false, sms_notification_forwarding: true, call_notification_forwarding: false },
        native_accounts: [{ line_id: 'app-account-0123456789abcdef0123456789abcdef', provider_id: 'app_voice', revision: 1, phase: 'disconnected', io_cleanup_required: true, incoming_message_ready: false, incoming_call_ready: false, outbound_authorized: false,
          startup_failure: { phase: 'bootstrap', stage: 'router_connect', code: 'jegotrip_connector_timeout' },
          startup_connect_detail: mode === 'invalid' ? { phase: 'account', step: 'private-token <img src=x onerror=alert(1)>' } : { phase: 'bootstrap', step: mode, hostname: 'private-host.invalid', token: 'private-token' },
          startup_tls_detail: { phase: 'bootstrap', reason: 'name_mismatch' } }],
        stored_observations: { app_sms_records: 0, app_call_records: 0, retained_inbox_records: 0, pending_sms_notifications: 0 }, warnings: ['no_armed_sms_receiver', 'no_armed_call_receiver'] } } }); return
    }
    await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } })
  })
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  const dns = page.getByText(/连接超时步骤：DNS准备或解析时限到达；仅本次超时步骤，不授信、不延长时限、不重试或恢复就绪/)
  await expect(dns).toBeVisible(); await expect(page.getByText(/启动连接诊断：Bootstrap \/ Router TCP\/DNS：连接超时/)).toBeVisible()
  await expect(page.getByText(/disconnected · 短信未武装 · 来电未武装/)).toBeVisible(); await expect(page.getByText(/TLS类别：/)).toHaveCount(0)
  for (const [name, width, height] of [['desktop', 1280, 1000], ['mobile', 390, 844]] as const) {
    await page.setViewportSize({ width, height }); await expect(dns).toBeVisible(); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
    await page.screenshot({ path: `test-results/jegotrip-device-ui/startup-connect-detail-${name}.png`, fullPage: true, animations: 'disabled' })
  }
  mode = 'tcp'; const prior = reads; await page.getByRole('button', { name: '刷新推送状态', exact: true }).click()
  await expect(page.getByText(/连接超时步骤：TCP连接时限到达/)).toBeVisible(); expect(reads).toBe(prior + 1)
  mode = 'invalid'; await page.getByRole('button', { name: '刷新推送状态', exact: true }).click(); await expect(page.getByText(/连接超时步骤：/)).toHaveCount(0)
  await expect(page.getByText(/启动连接诊断：Bootstrap \/ Router TCP\/DNS：连接超时/)).toBeVisible()
  for (const value of ['private-token', 'private-host.invalid']) await expect(page.getByText(value, { exact: false })).toHaveCount(0)
  await expect(page.locator('img[src="x"]')).toHaveCount(0); expect(writes).toEqual([]); expect(errors).toEqual([])
})
