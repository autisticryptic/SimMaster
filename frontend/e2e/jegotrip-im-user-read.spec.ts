import { test, expect } from '@playwright/test'
test('explicit original user read is separate from received position and unknown writes block replay across account changes', async ({ page }) => {
  const writes: string[] = []; const errors: string[] = []; let confirmed = false
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { void dialog.accept() })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; const old = path.includes('0123456789abcdef0123456789abcdef')
    if (request.method() !== 'GET') {
      writes.push(path); expect(request.method()).toBe('POST'); expect(path.endsWith('/im-messages/1/mark-read')).toBe(true)
      const data = request.postDataJSON() as { expected_revision: unknown; nonce: unknown; consent: unknown }
      expect(Object.keys(data).sort()).toEqual(['consent', 'expected_revision', 'nonce']); expect(data.expected_revision).toBe(1); expect(data.consent).toBe(true); expect(data.nonce).toMatch(/^[0-9a-f]{32}$/)
      if (!old) { await route.abort(); return }
      confirmed = true; await route.fulfill({ json: { status: 'ok', data: { id: 10, message_id: 1, target_index: '9223372036854775807', state: 'confirmed', updated_at: '2026-01-01T00:00:00Z' } } }); return
    }
    if (path.endsWith('/im-user-reads')) { await route.fulfill({ json: { status: 'ok', data: old && confirmed ? [{ id: 10, message_id: 1, target_index: '9223372036854775807', state: 'confirmed', updated_at: '2026-01-01T00:00:00Z' }] : [] } }); return }
    expect(path.endsWith('/im-messages')).toBe(true)
    await route.fulfill({ json: { status: 'ok', data: [{ id: 1, box_kind: 'p2p', message_index: old ? '9223372036854775807' : '7', text: old ? '原用户已读文本 <script>不可执行</script>' : '另一账号用户已读文本', attachment_count: 0, received_at: '2026-01-01T00:00:00Z', token: 'private-token' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/app-im-user-read.html')
  await expect(page.getByText(/原用户已读文本/)).toBeVisible(); expect(writes).toEqual([])
  await page.getByRole('button', { name: '明确标记原消息已读' }).click()
  await expect(page.getByText('原用户已读操作已确认', { exact: true })).toBeVisible(); await expect(page.getByText(/不是接收位置回执/)).toBeVisible(); await expect(page.getByText('private-token', { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-user-read-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '切换用户已读账号' }).click(); await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByText('另一账号用户已读文本', { exact: true })).toBeVisible(); await expect(page.getByText('原用户已读操作已确认', { exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: '明确标记原消息已读' }).click()
  await expect(page.getByText(/用户已读写入未核实/)).toBeVisible(); await expect(page.getByRole('button', { name: '明确标记原消息已读' })).toBeDisabled()
  expect(writes).toHaveLength(2); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-user-read-mobile.png', fullPage: true, animations: 'disabled' })
})
