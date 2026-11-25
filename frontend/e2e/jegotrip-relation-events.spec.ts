import { test, expect } from '@playwright/test'
const row = { id: 1, target_kind: 'group', updates: 1, removals: 2, base_time: '-9223372036854775808', update_time: '9223372036854775807', continuity: 'unseeded', full_roster_verified: false, received_at: '2026-01-01T00:00:00Z' }
test('original relation events are explicit finite read-only observations with signed64 gaps and Unknown WRITE unchanged', async ({ page }) => {
  const reads: string[] = []; const errors: string[] = []; page.on('pageerror', (e) => errors.push(e.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const url = new URL(request.url()); expect(request.method()).toBe('GET'); expect(request.postData()).toBeNull(); expect(url.pathname).toBe('/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/relation-events'); expect(url.search).toBe('?limit=128'); reads.push(url.pathname)
    await route.fulfill({ headers: { 'cache-control': 'no-store' }, json: { status: 'ok', data: ['unseeded', 'partial_chain', 'gap', 'stale'].map((continuity, n) => ({ ...row, id: n + 1, continuity })) } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/relation-events.html'); const button = page.getByRole('button', { name: '明确读取入站关系事件（只读）', exact: true })
  expect(reads).toEqual([]); await button.click(); await expect(page.getByText('事件 #3 · Group 关系 · 存在缺口（gap）：不自动推进基线或清除缺口', { exact: true })).toBeVisible()
  await expect(page.getByText('原 BaseTime -9223372036854775808 · UpdateTime 9223372036854775807（signed64 原值，不转换为墙上时钟或在线状态）', { exact: true })).toHaveCount(4)
  await expect(page.getByText('观测更新 1 项 · 观测删除 2 项 · 完整名册未核实（false）', { exact: true })).toHaveCount(4)
  await expect(page.getByText('出站关系 WRITE：Unknown（入站事件不能清除）', { exact: true })).toBeVisible(); await expect(page.getByRole('textbox')).toHaveCount(0); expect(reads).toHaveLength(1)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/relation-events-desktop.png', fullPage: true, animations: 'disabled' }); await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); expect(errors).toEqual([])
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/relation-events-mobile.png', fullPage: true, animations: 'disabled' })
})
test('relation event account ABA revision malformed private and empty responses never revive old state or retry WRITE', async ({ page }) => {
  const reads: string[] = []; const errors: string[] = []; const completed: number[] = []; let release: (() => void) | undefined; page.on('pageerror', (e) => errors.push(e.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const url = new URL(request.url()); expect(request.method()).toBe('GET'); expect(url.search).toBe('?limit=128'); reads.push(url.pathname); const index = reads.length
    if (index === 1 || index === 3) await new Promise<void>((resolve) => { release = resolve })
    if (index === 4) { await route.abort(); return }
    await route.fulfill({ json: { status: 'ok', data: index === 2 ? [{ ...row, uid: 'private-uid', full_roster_verified: true }] : index === 5 ? [] : [row] } }); completed.push(index)
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/relation-events.html'); const button = page.getByRole('button', { name: '明确读取入站关系事件（只读）', exact: true }); const result = page.getByText('事件 #1 · Group 关系 · 未建立基线（unseeded）：只有局部入站观测', { exact: true }); const error = page.getByText('入站关系事件未核实；不展示错误响应内容，不自动重试，不解决 Unknown-WRITE。', { exact: true })
  await button.click(); await expect.poll(() => reads.length).toBe(1); await page.getByRole('button', { name: '切换事件账号', exact: true }).click(); await expect(button).toBeEnabled(); await page.getByRole('button', { name: '切换事件账号', exact: true }).click(); release?.(); await expect.poll(() => completed.includes(1)).toBe(true); await expect(button).toBeEnabled(); await expect(result).toHaveCount(0); expect(reads).toHaveLength(1)
  await button.click(); await expect(error).toBeVisible(); await expect(page.getByText('private-uid', { exact: false })).toHaveCount(0); expect(reads).toHaveLength(2)
  await button.click(); await expect.poll(() => reads.length).toBe(3); await page.getByRole('button', { name: '替换事件配置版本', exact: true }).click(); release?.(); await expect.poll(() => completed.includes(3)).toBe(true); await expect(button).toBeEnabled(); await expect(result).toHaveCount(0); expect(reads).toHaveLength(3)
  await button.click(); await expect(error).toBeVisible(); expect(reads).toHaveLength(4); await page.screenshot({ path: 'test-results/jegotrip-device-ui/relation-events-generation-desktop.png', fullPage: true, animations: 'disabled' })
  await button.click(); await expect(page.getByText('本次未返回入站关系事件；不是没有关系、没有事件或先前写入未发生的证明。', { exact: true })).toBeVisible(); expect(reads).toHaveLength(5); await expect(page.getByText('出站关系 WRITE：Unknown（入站事件不能清除）', { exact: true })).toBeVisible(); await expect(page.getByRole('textbox')).toHaveCount(0)
  await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); expect(errors).toEqual([]); await page.screenshot({ path: 'test-results/jegotrip-device-ui/relation-events-generation-mobile.png', fullPage: true, animations: 'disabled' })
})
