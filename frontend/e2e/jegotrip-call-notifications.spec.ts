import { test, expect } from '@playwright/test'
test('call notification status stays original-account read-only and never claims answered media or retries unknown outcomes', async ({ page }) => {
  const writes: string[] = []; const errors: string[] = []; page.on('pageerror', (error) => errors.push(error.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); if (request.method() !== 'GET') writes.push(request.method())
    const path = new URL(request.url()).pathname; expect(path.endsWith('/call-notifications')).toBe(true)
    const old = path.includes('0123456789abcdef0123456789abcdef')
    await route.fulfill({ json: { status: 'ok', data: [{ id: old ? 1 : 2, call_history_id: old ? 91 : 92, state: old ? 'delivered' : 'uncertain', attempts: 1, expires_at: 1001 },
      { id: 3, call_history_id: 93, state: 'private-token', attempts: 1, expires_at: 1001 }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/app-call-notifications.html')
  await expect(page.getByText('原通话记录 91 · 渠道已确认送达；不代表来电已接听或媒体就绪', { exact: false })).toBeVisible()
  await expect(page.getByText('private-token', { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/call-notifications-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '切换原账号' }).click(); await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByText(/原通话记录 92.*结果不确定，不自动重发/)).toBeVisible()
  await expect(page.getByText(/原通话记录 91/)).toHaveCount(0)
  expect(writes).toEqual([]); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/call-notifications-mobile.png', fullPage: true, animations: 'disabled' })
})
