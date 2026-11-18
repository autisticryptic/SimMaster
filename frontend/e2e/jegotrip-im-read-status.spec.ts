import { test, expect } from '@playwright/test'
test('manual Group read-position is explicit current baseline intent and never user-read grant or uncertain replay', async ({ page }) => {
  const writes: string[] = []; const errors: string[] = []; let consent = false; let rows: object[] = []
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { if (consent) { void dialog.accept() } else { void dialog.dismiss() } })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; const old = !path.includes('fedcba')
    const index = old ? '9223372036854775807' : '7'
    if (request.method() === 'POST') {
      writes.push(path); expect(path.endsWith('/im-read-status-targets/2/set')).toBe(true)
      expect(request.postDataJSON()).toEqual({ expected_revision: 1, expected_index: index, nonce: expect.stringMatching(/^[0-9a-f]{32}$/), consent: true })
      if (writes.length === 2) { rows.push({ id: 2, target_id: 2, box_kind: 'org', target_index: index, state: 'uncertain' }); await route.abort(); return }
      rows = [{ id: 1, target_id: 2, box_kind: 'org', target_index: index, state: 'confirmed' }]
      await route.fulfill({ json: { status: 'ok', data: { ...rows[0], uid: 'private-uid', span_hash: 'private-hash' } } }); return
    }
    expect(request.method()).toBe('GET')
    if (path.endsWith('/im-read-status-actions')) { await route.fulfill({ json: { status: 'ok', data: old ? rows : [] } }); return }
    expect(path.endsWith('/im-read-status-targets')).toBe(true); expect(new URL(request.url()).searchParams.get('expected_revision')).toBe('1')
    await route.fulfill({ json: { status: 'ok', data: [{ id: 2, box_kind: 'org', target_index: index }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-read-status.html')
  const button = page.getByRole('button', { name: '明确设置原读位置', exact: true }); await expect(page.getByText(/原连续消费位置 9223372036854775807/)).toBeVisible(); expect(writes).toEqual([])
  await button.click(); expect(writes).toEqual([]); consent = true; await button.click(); await expect(page.getByText('位置 9223372036854775807 · 原手动读位置操作已确认（不是用户已读）', { exact: true })).toBeVisible()
  await expect(page.getByText('private-', { exact: false })).toHaveCount(0); await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-desktop.png', fullPage: true, animations: 'disabled' })
  await button.click(); await expect(page.getByText('原手动读位置写入未核实；仅查询历史，不重放。', { exact: true })).toBeVisible(); await expect(button).toBeDisabled(); expect(writes).toHaveLength(2)
  await page.getByRole('button', { name: '切换原读位置账号' }).click(); await expect(page.getByText(/原连续消费位置 7$/)).toBeVisible(); await expect(page.getByText('原手动读位置写入未核实；仅查询历史，不重放。', { exact: true })).toHaveCount(0)
  await page.setViewportSize({ width: 390, height: 844 }); expect(errors).toEqual([]); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-mobile.png', fullPage: true, animations: 'disabled' })
})
test('manual full-history baseline blocks an older unknown beyond displayed rows and malformed history never mints a nonce', async ({ page }) => {
  let writes = 0; let malformed = false; let targetQueries = 0; let historyQueries = 0
  await page.addInitScript(() => { const original = crypto.randomUUID.bind(crypto); Object.defineProperty(crypto, 'randomUUID', { value: () => { document.documentElement.dataset.readStatusNonces = String(Number(document.documentElement.dataset.readStatusNonces || 0) + 1); return original() } }) })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const url = new URL(request.url()); expect(request.method()).toBe('GET')
    if (request.method() === 'POST') writes += 1
    if (url.pathname.endsWith('/im-read-status-actions')) { historyQueries += 1; expect(url.searchParams.get('limit')).toBe('128'); const rows = Array.from({ length: 20 }, (_, i) => ({ id: 20 - i, target_id: 2, box_kind: 'org', target_index: '7', state: i === 19 ? 'uncertain' : 'rejected' })); await route.fulfill({ json: { status: 'ok', data: malformed ? [{ accepted: true }] : rows } }); return }
    targetQueries += 1; await route.fulfill({ json: { status: 'ok', data: [{ id: 2, box_kind: 'org', target_index: '7' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-read-status.html')
  await expect(page.getByText('原手动读位置写入未核实；仅查询历史，不重放。', { exact: true })).toBeVisible(); await expect(page.getByRole('button', { name: '明确设置原读位置', exact: true })).toBeDisabled()
  expect(await page.evaluate(() => Number(document.documentElement.dataset.readStatusNonces || 0))).toBe(0); expect(writes).toBe(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-history-desktop.png', fullPage: true, animations: 'disabled' })
  const originalTargetQueries = targetQueries; const originalHistoryQueries = historyQueries; await page.getByRole('button', { name: '暂停读位置运行态', exact: true }).click(); await expect.poll(() => historyQueries).toBeGreaterThan(originalHistoryQueries); await expect(page.getByRole('button', { name: '明确设置原读位置', exact: true })).toHaveCount(0); await expect(page.getByText('原手动读位置写入未核实；仅查询历史，不重放。', { exact: true })).toBeVisible(); expect(targetQueries).toBe(originalTargetQueries)
  malformed = true; await expect(page.getByText('当前原目标或历史未核实；不生成新的写入。', { exact: true })).toBeVisible(); expect(await page.evaluate(() => Number(document.documentElement.dataset.readStatusNonces || 0))).toBe(0); expect(writes).toBe(0)
  await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-history-mobile.png', fullPage: true, animations: 'disabled' })
})
test('manual late account result and old terminal cannot clear later uncertainty and returning account needs a fresh generation baseline', async ({ page }) => {
  let releaseWrite: (() => void) | undefined; let releaseHistory: (() => void) | undefined; let writes = 0; let aHistory = 0; let bHistory = 0; let holdReturningHistory = false
  await page.addInitScript(() => { const original = crypto.randomUUID.bind(crypto); Object.defineProperty(crypto, 'randomUUID', { value: () => { document.documentElement.dataset.readStatusNonces = String(Number(document.documentElement.dataset.readStatusNonces || 0) + 1); return original() } }) })
  page.on('dialog', (dialog) => { void dialog.accept() })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; const isA = !path.includes('fedcba')
    if (request.method() === 'POST') {
      writes += 1
      if (isA) { await new Promise<void>((resolve) => { releaseWrite = resolve }); await route.fulfill({ json: { status: 'ok', data: { id: 1, target_id: 2, box_kind: 'org', target_index: '7', state: 'confirmed' } } }); return }
      await route.abort(); return
    }
    expect(request.method()).toBe('GET')
    if (path.endsWith('/im-read-status-actions')) {
      if (isA) { aHistory += 1; if (holdReturningHistory) await new Promise<void>((resolve) => { releaseHistory = resolve }); await route.fulfill({ json: { status: 'ok', data: aHistory === 1 ? [] : [{ id: 1, target_id: 2, box_kind: 'org', target_index: '7', state: 'confirmed' }] } }); return }
      bHistory += 1; await route.fulfill({ json: { status: 'ok', data: [{ id: 1, target_id: 2, box_kind: 'org', target_index: '7', state: 'rejected' }] } }); return
    }
    await route.fulfill({ json: { status: 'ok', data: [{ id: 2, box_kind: 'org', target_index: '7' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-read-status.html')
  const button = page.getByRole('button', { name: '明确设置原读位置', exact: true }); await expect(button).toBeEnabled(); await button.click(); await expect.poll(() => typeof releaseWrite).toBe('function')
  await page.getByRole('button', { name: '切换原读位置账号', exact: true }).click(); releaseWrite?.(); await expect(page.getByText('位置 7 · 原手动读位置操作被拒绝', { exact: true })).toBeVisible()
  await expect(page.getByText('位置 7 · 原手动读位置操作已确认（不是用户已读）', { exact: true })).toHaveCount(0); await button.click(); await expect(page.getByText('原手动读位置写入未核实；仅查询历史，不重放。', { exact: true })).toBeVisible()
  await expect.poll(() => bHistory).toBeGreaterThanOrEqual(2); await expect(button).toBeDisabled(); await expect(page.getByText('原手动读位置写入未核实；仅查询历史，不重放。', { exact: true })).toBeVisible()
  holdReturningHistory = true; await page.getByRole('button', { name: '切回原读位置账号', exact: true }).click(); await expect.poll(() => typeof releaseHistory).toBe('function'); await expect(button).toHaveCount(0)
  expect(await page.evaluate(() => Number(document.documentElement.dataset.readStatusNonces || 0))).toBe(2); expect(writes).toBe(2); releaseHistory?.(); holdReturningHistory = false; await expect(button).toBeEnabled(); await expect(page.getByText('位置 7 · 原手动读位置操作已确认（不是用户已读）', { exact: true })).toBeVisible()
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-generation-desktop.png', fullPage: true, animations: 'disabled' })
  await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); expect(writes).toBe(2)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-read-status-generation-mobile.png', fullPage: true, animations: 'disabled' })
})
