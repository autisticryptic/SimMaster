import { test, expect } from '@playwright/test'
test('original IM history is separate from SMS and deletes once with uncertainty instead of replay or account crossover', async ({ page }) => {
  const writes: string[] = []; const errors: string[] = []; let deleted = false
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { void dialog.accept() })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname
    expect(path.includes('/im-messages')).toBe(true)
    if (request.method() !== 'GET') {
      writes.push(`${request.method()} ${path}`); expect(request.method()).toBe('DELETE'); expect(path.endsWith('/1')).toBe(true)
      if (path.includes('fedcba9876543210')) { await route.abort(); return }
      deleted = true; await route.fulfill({ json: { status: 'ok', data: true } }); return
    }
    const old = path.includes('0123456789abcdef0123456789abcdef')
    await route.fulfill({ json: { status: 'ok', data: old && deleted ? [] : [{ id: 1, box_kind: old ? 'p2p' : 'org', message_index: '9223372036854775807', text: old ? '原 IM 内容 <script>不可执行</script>' : '另一账号 IM', attachment_count: 1, received_at: '2026-01-01T00:00:00Z', body: 'private-raw', sender: 'private-sender', identity_key: 'private-key' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/app-im-messages.html')
  await expect(page.getByText(/原 IM 内容 <script>不可执行/)).toBeVisible()
  await expect(page.getByText(/不是运营商短信/)).toBeVisible(); await expect(page.getByText(/消息索引 9223372036854775807/)).toBeVisible()
  for (const hidden of ['private-raw', 'private-sender', 'private-key']) await expect(page.getByText(hidden, { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-history-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '删除本地 IM 文本' }).click(); await expect(page.getByText(/原 IM 内容/)).toHaveCount(0)
  await page.getByRole('button', { name: '切换原账号' }).click(); await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByText('另一账号 IM', { exact: true })).toBeVisible(); await expect(page.getByText(/原 IM 内容/)).toHaveCount(0)
  await page.getByRole('button', { name: '删除本地 IM 文本' }).click()
  await expect(page.getByText(/删除结果尚未核实/)).toBeVisible(); await expect(page.getByRole('button', { name: '删除本地 IM 文本' })).toBeDisabled()
  expect(writes).toHaveLength(2); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-history-mobile.png', fullPage: true, animations: 'disabled' })
})
