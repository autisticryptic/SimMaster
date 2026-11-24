import { test, expect } from '@playwright/test'
const row = { id: 2, nonce_tag: 'b'.repeat(64), target_kind: 'group', state: 'confirmed', additions: 1, updates: 2, removals: 3, base_time: '-9223372036854775808', update_time: '9223372036854775807', updated_at: '2026-01-01T00:00:00Z' }
const unknown = { ...row, id: 1, nonce_tag: 'a'.repeat(64), target_kind: 'buddy', state: 'uncertain', base_time: null, update_time: null }
test('relation modification history is explicit read-only with exact signed64 outcomes no write nonce or SDK grants', async ({ page }) => {
  const reads: string[] = []; const errors: string[] = []; page.on('pageerror', (error) => errors.push(error.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const url = new URL(request.url()); expect(request.method()).toBe('GET'); expect(url.pathname).toBe('/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/relation-writes'); expect(url.search).toBe('?limit=128'); expect(request.postData()).toBeNull(); reads.push(url.pathname)
    await route.fulfill({ headers: { 'cache-control': 'no-store' }, json: { status: 'ok', data: [row, unknown] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/relation-history.html'); const button = page.getByRole('button', { name: '明确读取关系修改记录（只读）', exact: true })
  expect(reads).toEqual([]); await button.click(); await expect(page.getByText('记录 #1 · Buddy 关系 · 关系修改结果未知；仅查询，不重放', { exact: true })).toBeVisible()
  await expect(page.getByText('原 BaseTime -9223372036854775808 · UpdateTime 9223372036854775807（signed64 原值，不推断墙上时钟或在线状态）', { exact: true })).toBeVisible(); await expect(page.getByText('新增 1 项 · 更新 2 项 · 删除 3 项', { exact: true })).toHaveCount(2)
  await expect(page.getByText('此视图没有关系写入按钮；完整修改交互与原服务端关系事件尚待独立验收。', { exact: true })).toBeVisible(); await expect(page.getByRole('textbox')).toHaveCount(0); expect(reads).toHaveLength(1)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/relation-history-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '停止夹具 SDK', exact: true }).click(); await expect(button).toBeEnabled(); await expect(page.getByText('SDK 观测：stopped（不是关系写权限）', { exact: true })).toBeVisible(); expect(reads).toHaveLength(1)
  await button.click(); await expect.poll(() => reads.length).toBe(2); await expect(button).toBeEnabled(); await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); expect(errors).toEqual([])
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/relation-history-mobile.png', fullPage: true, animations: 'disabled' })
})
test('relation history ignores old account and revision observations and rejects malformed or private projections without retries', async ({ page }) => {
  const reads: string[] = []; const errors: string[] = []; let release: (() => void) | undefined; page.on('pageerror', (error) => errors.push(error.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); expect(request.method()).toBe('GET'); expect(new URL(request.url()).search).toBe('?limit=128'); reads.push(new URL(request.url()).pathname); const index = reads.length
    if (index === 1 || index === 3) await new Promise<void>((resolve) => { release = resolve })
    if (index === 4) { await route.abort(); return }
    await route.fulfill({ json: { status: 'ok', data: index === 2 ? [{ ...row, uid: 'private-uid', token: 'private-token', ready: true }] : [row] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/relation-history.html'); const button = page.getByRole('button', { name: '明确读取关系修改记录（只读）', exact: true }); const result = page.getByText('记录 #2 · Group 关系 · 原关系修改操作已确认（不是在线或送达证明）', { exact: true })
  await button.click(); await expect.poll(() => reads.length).toBe(1); await page.getByRole('button', { name: '切换记录账号', exact: true }).click(); release?.(); await expect(button).toBeEnabled(); await expect(result).toHaveCount(0); expect(reads).toHaveLength(1)
  await button.click(); await expect(page.getByText('关系修改记录未核实；不会自动查询、重新登录、换 nonce 或重放未知 WRITE。', { exact: true })).toBeVisible(); expect(reads[1]).toContain('app-account-fedcba9876543210fedcba9876543210'); await expect(page.getByText('private-', { exact: false })).toHaveCount(0); expect(reads).toHaveLength(2)
  await button.click(); await expect.poll(() => reads.length).toBe(3); await page.getByRole('button', { name: '替换记录配置版本', exact: true }).click(); release?.(); await expect(button).toBeEnabled(); await expect(result).toHaveCount(0); expect(reads).toHaveLength(3)
  await button.click(); await expect(page.getByText('关系修改记录未核实；不会自动查询、重新登录、换 nonce 或重放未知 WRITE。', { exact: true })).toBeVisible(); expect(reads).toHaveLength(4); await expect(page.getByRole('textbox')).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/relation-history-generation-desktop.png', fullPage: true, animations: 'disabled' }); await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); expect(errors).toEqual([])
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/relation-history-generation-mobile.png', fullPage: true, animations: 'disabled' })
})
