import { test, expect } from '@playwright/test'

test('blocked hardware stays in the original selectable workbench with saved controls', async ({ page }, testInfo) => {
  const requests: Array<{ path: string; method: string }> = []
  page.on('pageerror', (error) => { void testInfo.attach('browser-error', { body: error.stack ?? error.message, contentType: 'text/plain' }) })
  const lines = ['a', 'b'].map((id, index) => ({
    modem: { line_id: `line-${id}`, line_kind: 'baseband', manufacturer: `Fixture ${id}`, model: 'QCM410',
      slot_label: `测试槽${index + 1}`, slot_source: 'physdev', slot_stable: true, slot_conflict: false,
      display_order: index + 1, uim_slot: 1, present: true, sim_iccid: '', modem_path: '', qmi_device: null, state: 'no_sim' },
    profile: { enabled: true, cellular_ims_connection_enabled: true, data_connection_enabled: true,
      airplane_mode_enabled: true, roaming_allowed: true, vowifi: { enabled: true }, trunk: { enabled: true } },
    runtime: { phase: 'blocked', stage: 'waiting_modem', registered: false, registration_mode: '',
      recovery_state: 'waiting_modem', last_error: 'ims_startup_recovery_pending', manual_retry_available: false,
      connection_attempts: [], profile_attempt_results: [], retry_attempt: 0, retry_max: 0, modem_restart_attempt: 0,
      modem_restart_max: 0, reconnect_count: 0, register_refresh_count: 0, sent_count: 0, received_count: 0, duplicate_count: 0 },
    read_only: true, blocked_reason: 'ims_startup_recovery_pending',
  }))
  await page.route('**/*', async (route) => {
    const request = route.request(), url = new URL(request.url())
    if (url.hostname !== '127.0.0.1') return route.abort()
    if (!url.pathname.startsWith('/api/')) return route.continue()
    requests.push({ path: url.pathname, method: request.method() })
    if (url.pathname === '/api/cellular-ims/lines') return route.fulfill({ json: { status: 'ok', data: lines } })
    return route.fulfill({ status: 503, json: { status: 'error', message: 'fixture blocks operational API' } })
  })
  await page.goto('/e2e/fixtures/lines.html')
  await expect(page.getByTestId('selected-line')).toHaveText('line-a')
  await expect(page.getByText('线路控制', { exact: true })).toBeVisible()
  await expect(page.getByTestId('original-basic-info')).toBeVisible()
  await expect(page.getByRole('tab')).toHaveCount(7)
  await expect(page.getByRole('checkbox', { name: '飞行模式' })).toBeChecked()
  await expect(page.getByRole('checkbox', { name: '飞行模式' })).toBeDisabled()
  await expect(page.getByRole('button', { name: /测试槽2/ })).toBeEnabled()
  await page.getByRole('button', { name: /测试槽2/ }).click()
  await expect(page.getByTestId('selected-line')).toHaveText('line-b')
  await page.getByRole('tab', { name: 'eSIM', exact: true }).click()
  await expect(page.getByText('原 eSIM 标签内容')).toBeVisible()
  await page.getByRole('tab', { name: '概览', exact: true }).click()
  await expect(page.getByTestId('original-basic-info')).toBeVisible()
  await expect(page.getByText('物理硬件已发现 · 仅供查看')).toHaveCount(0)
  await expect(page.getByRole('button', { name: '刷新硬件清单' })).toHaveCount(0)
  expect(requests.every((request) => request.path === '/api/cellular-ims/lines' && request.method === 'GET')).toBe(true)
  await page.screenshot({ path: testInfo.outputPath('original-workbench-blocked.png'), fullPage: true })
})
