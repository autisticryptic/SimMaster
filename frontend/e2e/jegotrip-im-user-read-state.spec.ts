import { test, expect } from '@playwright/test'
const firstLine = '0123456789abcdef0123456789abcdef'
const original = { id: 8, message_id: 1, target_index: '7', state: 'confirmed', updated_at: '2026-01-01T00:00:00Z' }
const pending = { ...original, id: 9, state: 'uncertain' }
test('user read waits for its original outcome baseline and old confirmation cannot clear a later lost write', async ({ page }) => {
  const writes: string[] = []; let histories = 0; let releaseBaseline: () => void = () => { throw new Error('Missing baseline barrier') }
  const baseline = new Promise<void>((resolve) => { releaseBaseline = resolve })
  page.on('dialog', (dialog) => { void dialog.accept() })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname
    if (request.method() === 'POST') { writes.push(path); expect(path.endsWith('/im-messages/1/mark-read')).toBe(true); await route.abort(); return }
    expect(request.method()).toBe('GET')
    if (path.endsWith('/im-user-reads')) {
      const number = ++histories; if (number === 1) await baseline
      await route.fulfill({ json: { status: 'ok', data: number === 1 ? [original] : [pending, original] } }); return
    }
    expect(path.endsWith('/im-messages')).toBe(true)
    await route.fulfill({ json: { status: 'ok', data: [{ id: 1, box_kind: 'p2p', message_index: '7', text: '等待本局用户已读原历史', attachment_count: 0, received_at: '2026-01-01T00:00:00Z' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/app-im-user-read.html')
  await expect(page.getByText('等待本局用户已读原历史', { exact: true })).toBeVisible()
  const button = page.getByRole('button', { name: '明确标记原消息已读' }); await expect(button).toBeDisabled(); expect(writes).toEqual([])
  releaseBaseline(); await expect(button).toBeEnabled(); await expect(page.getByText('原用户已读操作已确认', { exact: true })).toBeVisible()
  await button.click(); await expect(page.getByText('用户已读写入未核实；仅查询原账号状态，不自动重放。', { exact: true })).toBeVisible(); await expect(button).toBeDisabled()
  await expect.poll(() => histories).toBeGreaterThanOrEqual(2)
  await expect(page.getByText('用户已读写入未核实；仅查询，不重放', { exact: true })).toBeVisible()
  await expect(page.getByText('用户已读写入未核实；仅查询原账号状态，不自动重放。', { exact: true })).toBeVisible(); await expect(button).toBeDisabled(); expect(writes).toHaveLength(1)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-user-read-baseline-desktop.png', fullPage: true, animations: 'disabled' })
})
test('user read reload uncertainty stays disabled and an old-account late baseline cannot enable the new account', async ({ page }) => {
  const writes: string[] = []; let oldStarted = false; let oldFinished = false; let releaseOld: () => void = () => { throw new Error('Missing old-account barrier') }
  const old = new Promise<void>((resolve) => { releaseOld = resolve })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; const oldAccount = path.includes(firstLine)
    if (request.method() !== 'GET') { writes.push(path); await route.abort(); return }
    if (path.endsWith('/im-user-reads')) {
      if (oldAccount) { oldStarted = true; await old; await route.fulfill({ json: { status: 'ok', data: [{ ...original, id: 99 }] } }); oldFinished = true; return }
      await route.fulfill({ json: { status: 'ok', data: [pending] } }); return
    }
    expect(path.endsWith('/im-messages')).toBe(true)
    await route.fulfill({ json: { status: 'ok', data: [{ id: 1, box_kind: 'p2p', message_index: '7', text: oldAccount ? '旧账号等待用户已读基线' : '新账号持久未知用户已读', attachment_count: 0, received_at: '2026-01-01T00:00:00Z' }] } })
  })
  await page.setViewportSize({ width: 390, height: 844 }); await page.goto('/e2e/fixtures/app-im-user-read.html')
  await expect(page.getByText('旧账号等待用户已读基线', { exact: true })).toBeVisible(); await expect.poll(() => oldStarted).toBe(true)
  await expect(page.getByRole('button', { name: '明确标记原消息已读' })).toBeDisabled()
  await page.getByRole('button', { name: '切换用户已读账号' }).click()
  await expect(page.getByText('新账号持久未知用户已读', { exact: true })).toBeVisible()
  await expect(page.getByText('用户已读写入未核实；仅查询，不重放', { exact: true })).toBeVisible()
  releaseOld(); await expect.poll(() => oldFinished).toBe(true)
  await expect(page.getByRole('button', { name: '明确标记原消息已读' })).toBeDisabled(); await expect(page.getByText('原用户已读操作已确认', { exact: true })).toHaveCount(0); expect(writes).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-user-read-reload-mobile.png', fullPage: true, animations: 'disabled' })
})
