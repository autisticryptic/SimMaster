import { test, expect } from '@playwright/test'
test('manual recipient private UID lookup is explicit fixed P2P position intent and shares unknown serialization with default path', async ({ page }) => {
  let consent = false; let writes = 0; let rows: object[] = []; const errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { if (consent) { void dialog.accept() } else { void dialog.dismiss() } })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; const old = !path.includes('fedcba')
    if (request.method() === 'POST') {
      writes += 1
      const common = { expected_revision: 1, expected_index: '9223372036854775807', nonce: expect.stringMatching(/^[0-9a-f]{32}$/), consent: true }
      if (writes === 1) { expect(path.endsWith('/im-read-status-targets/2/set/recipient')).toBe(true); expect(request.postDataJSON()).toEqual({ ...common, recipient: '[phone:+12345@fixture.invalid]' }); rows = [{ id: 1, target_id: 2, box_kind: 'p2p', target_index: '9223372036854775807', state: 'confirmed' }]; await route.fulfill({ json: { status: 'ok', data: { ...rows[0], uid: 'private-resolved-uid', recipient_hash: 'private-hash' } } }); return }
      expect(path.endsWith('/im-read-status-targets/2/set')).toBe(true); expect(request.postDataJSON()).toEqual(common); rows.push({ id: 2, target_id: 2, box_kind: 'p2p', target_index: '9223372036854775807', state: 'uncertain' }); await route.abort(); return
    }
    expect(request.method()).toBe('GET')
    if (path.endsWith('/im-read-status-actions')) { expect(new URL(request.url()).searchParams.get('limit')).toBe('128'); await route.fulfill({ json: { status: 'ok', data: old ? rows : [] } }); return }
    expect(path.endsWith('/im-read-status-targets')).toBe(true); await route.fulfill({ json: { status: 'ok', data: [{ id: 2, box_kind: 'p2p', target_index: old ? '9223372036854775807' : '7' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-read-status-recipient.html')
  const input = page.getByRole('textbox', { name: '手动读位置收件人原 URI（可选）' }); const alias = page.getByRole('button', { name: '通过原收件人设置读位置', exact: true }); const original = page.getByRole('button', { name: '明确设置原读位置', exact: true })
  await expect(original).toBeEnabled(); await input.fill('[uid:1_2]'); await expect(alias).toBeDisabled(); expect(writes).toBe(0); await input.fill('[phone:+12345@fixture.invalid]'); await expect(alias).toBeEnabled(); await alias.click(); expect(writes).toBe(0); consent = true; await alias.click()
  await expect(page.getByText('位置 9223372036854775807 · 原手动读位置操作已确认（不是用户已读）', { exact: true })).toBeVisible(); await expect(page.getByText('private-', { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-recipient-desktop.png', fullPage: true, animations: 'disabled' })
  await original.click(); await expect(page.getByText('原手动读位置写入未核实；仅查询历史，不重放。', { exact: true })).toBeVisible(); await expect(alias).toBeDisabled(); await expect(original).toBeDisabled(); expect(writes).toBe(2)
  await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-recipient-mobile.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '切换原手动位置账号', exact: true }).click(); await expect(input).toHaveValue(''); await expect(alias).toBeDisabled(); await expect(original).toBeEnabled(); expect(errors).toEqual([]); expect(writes).toBe(2)
})
test('manual recipient stale span refusal before WRITE reloads the current baseline without creating permanent uncertainty', async ({ page }) => {
  let writes = 0; let current = '7'
  page.on('dialog', (dialog) => { void dialog.accept() })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname
    if (request.method() === 'POST') { writes += 1; expect(path.endsWith('/set/recipient')).toBe(true); if (writes === 1) { current = '8'; await route.fulfill({ status: 409, json: { status: 'error', message: 'app_account_im_read_status_recipient_target_stale' } }); return } expect(request.postDataJSON().expected_index).toBe('8'); await route.fulfill({ json: { status: 'ok', data: { id: 1, target_id: 2, box_kind: 'p2p', target_index: '8', state: 'confirmed' } } }); return }
    expect(request.method()).toBe('GET'); await route.fulfill({ json: { status: 'ok', data: path.endsWith('/im-read-status-actions') ? [] : [{ id: 2, box_kind: 'p2p', target_index: current }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-read-status-recipient.html'); await page.getByRole('textbox', { name: '手动读位置收件人原 URI（可选）' }).fill('[phone:+12345@fixture.invalid]')
  const alias = page.getByRole('button', { name: '通过原收件人设置读位置', exact: true }); await expect(alias).toBeEnabled(); await alias.click(); await expect(page.getByText('当前原目标或历史未核实；不生成新的写入。', { exact: true })).toBeVisible(); await expect(page.getByText('原手动读位置写入未核实；仅查询历史，不重放。', { exact: true })).toHaveCount(0); expect(writes).toBe(1)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-recipient-stale-desktop.png', fullPage: true, animations: 'disabled' })
  await expect(page.getByText(/原连续消费位置 8$/)).toBeVisible(); await expect(alias).toBeEnabled(); expect(writes).toBe(1); await alias.click(); await expect(page.getByText('位置 8 · 原手动读位置操作已确认（不是用户已读）', { exact: true })).toBeVisible(); expect(writes).toBe(2)
  await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-recipient-stale-mobile.png', fullPage: true, animations: 'disabled' })
})
