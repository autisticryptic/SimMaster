import { test, expect } from '@playwright/test'
test('recipient mark-read is explicit current baseline intent and unknown result blocks both direct and alias writes', async ({ page }) => {
  const writes: string[] = []; const errors: string[] = []; let consent = false; let known: object[] = []; let unknown = false
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { if (consent) { void dialog.accept() } else { void dialog.dismiss() } })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname
    if (request.method() === 'POST') {
      writes.push(path); expect(path.endsWith('/im-messages/1/mark-read/recipient')).toBe(true); const body = request.postDataJSON() as Record<string, unknown>
      expect(body).toEqual({ expected_revision: 1, nonce: expect.stringMatching(/^[0-9a-f]{32}$/), recipient: '[phone:+12345@fixture.invalid]', consent: true })
      if (writes.length === 2) { unknown = true; known.push({ id: 2, message_id: 1, target_index: '7', state: 'uncertain', updated_at: '2026-01-01T00:00:00Z' }); await route.abort(); return }
      known = [{ id: 1, message_id: 1, target_index: '7', state: 'confirmed', updated_at: '2026-01-01T00:00:00Z' }]
      await route.fulfill({ json: { status: 'ok', data: { ...known[0], uid: 'private-uid', token: 'private-token' } } }); return
    }
    expect(request.method()).toBe('GET')
    if (path.endsWith('/im-user-reads')) { await route.fulfill({ json: { status: 'ok', data: path.includes('fedcba') ? [] : known } }); return }
    expect(path.endsWith('/im-messages')).toBe(true)
    await route.fulfill({ json: { status: 'ok', data: [{ id: 1, box_kind: 'p2p', message_index: '7', text: '本条原消息正文', attachment_count: 0, received_at: '2026-01-01T00:00:00Z' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-recipient-read.html')
  const input = page.getByLabel('已读收件人原 URI（可选）'); const alias = page.getByRole('button', { name: '通过原收件人解析标记已读', exact: true }); const direct = page.getByRole('button', { name: '明确标记原消息已读', exact: true })
  await expect(input).toBeVisible(); await input.fill('7_9'); await expect(alias).toBeDisabled(); await input.fill('[phone:+12345@fixture.invalid]'); await alias.click(); expect(writes).toEqual([])
  consent = true; await alias.click(); await expect(page.getByText('原用户已读操作已确认', { exact: true })).toBeVisible(); await expect(page.getByText('private-', { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-recipient-read-desktop.png', fullPage: true, animations: 'disabled' })
  await alias.click(); await expect(page.getByText('用户已读写入未核实；仅查询原账号状态，不自动重放。', { exact: true })).toBeVisible(); expect(unknown).toBe(true); await expect(alias).toBeDisabled(); await expect(direct).toBeDisabled(); expect(writes).toHaveLength(2)
  await page.getByRole('button', { name: '切换收件人已读账号' }).click(); await expect(input).toHaveValue(''); await expect(page.getByText('用户已读写入未核实；仅查询原账号状态，不自动重放。', { exact: true })).toHaveCount(0)
  await page.setViewportSize({ width: 390, height: 844 }); await input.fill('scheme://fixture.invalid'); await expect(alias).toBeDisabled(); expect(errors).toEqual([]); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-recipient-read-mobile.png', fullPage: true, animations: 'disabled' })
})
