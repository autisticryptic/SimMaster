import { test, expect } from '@playwright/test'
test('original received-position receipts show uncertainty without write replay and discard delayed old-account reads', async ({ page }) => {
  const writes: string[] = []; const errors: string[] = []; let oldReads = 0; let releaseOld: (() => void) | undefined
  page.on('pageerror', (error) => errors.push(error.message))
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname
    expect(path.endsWith('/im-receipts')).toBe(true)
    if (request.method() !== 'GET') writes.push(request.method())
    expect(request.method()).toBe('GET')
    const old = path.includes('0123456789abcdef0123456789abcdef')
    if (old && ++oldReads > 1) await new Promise<void>((resolve) => { releaseOld = resolve })
    await route.fulfill({ json: { status: 'ok', data: [{ id: old ? 1 : 2, box_kind: old ? 'p2p' : 'org', target_index: old ? '9223372036854775807' : '7', state: old ? 'uncertain' : 'confirmed', updated_at: '2026-01-01T00:00:00Z', attempt_nonce: 'private-nonce', identity_key: 'private-key', token: 'private-token' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/app-im-receipts.html')
  await expect(page.getByText(/接收位置写入尚未核实；不重放/)).toBeVisible()
  await expect(page.getByText(/接收索引 9223372036854775807/)).toBeVisible()
  await expect(page.getByText(/不是用户已读、渠道送达/)).toBeVisible()
  for (const privateValue of ['private-nonce', 'private-key', 'private-token']) await expect(page.getByText(privateValue, { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-receipts-desktop.png', fullPage: true, animations: 'disabled' })
  await expect.poll(() => releaseOld !== undefined).toBe(true)
  await page.getByRole('button', { name: '切换回执账号' }).click(); await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByText('原接收位置回执已确认', { exact: true })).toBeVisible()
  releaseOld?.()
  await expect(page.getByText(/接收索引 9223372036854775807/)).toHaveCount(0)
  await expect(page.getByText(/尚未核实；不重放/)).toHaveCount(0)
  expect(writes).toEqual([]); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-receipts-mobile.png', fullPage: true, animations: 'disabled' })
})
