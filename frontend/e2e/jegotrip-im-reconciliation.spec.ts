import { test, expect } from '@playwright/test'
test('original IM reconciliation is an explicit current-account readonly sample and never clears unknown write or grants replay', async ({ page }) => {
  const reads: string[] = []; const errors: string[] = []; let accept = false
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { if (accept) { void dialog.accept() } else { void dialog.dismiss() } })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; const old = path.includes('0123456789abcdef0123456789abcdef')
    const target = old ? '9223372036854775807' : '7'
    if (request.method() === 'POST') {
      reads.push(path); expect(path.endsWith('/im-user-reads/1/reconcile')).toBe(true); expect(request.postDataJSON()).toEqual({ expected_revision: 1, consent: true })
      if (!old) { await route.abort(); return }
      await route.fulfill({ json: { status: 'ok', data: { receipt_id: 1, message_id: 2, target_index: target, local_state: 'uncertain', report: 'reported', reported_last_read: target, reported_last_received: '8', reported_latest: target, server_update_time: '4294967297', observed_at: '2026-01-01T00:00:00Z', token: 'private-token' } } }); return
    }
    expect(request.method()).toBe('GET'); expect(path.endsWith('/im-user-reads')).toBe(true)
    await route.fulfill({ json: { status: 'ok', data: [{ id: 1, message_id: 2, target_index: target, state: 'uncertain', updated_at: '2026-01-01T00:00:00Z' }] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-reconciliation.html')
  await expect(page.getByText(/索引 9223372036854775807/)).toBeVisible(); await expect(page.getByText('用户已读写入未核实；仅查询，不重放', { exact: true })).toBeVisible(); expect(reads).toEqual([])
  await page.getByRole('button', { name: '只读核对原已读位置' }).click(); expect(reads).toEqual([])
  accept = true; await page.getByRole('button', { name: '只读核对原已读位置' }).click()
  await expect(page.getByText('当前报告已读位置达到目标；不是原写入匹配回执，未知记录保持。', { exact: true })).toBeVisible()
  await expect(page.getByText('用户已读写入未核实；仅查询，不重放', { exact: true })).toBeVisible(); await expect(page.getByText('private-token', { exact: false })).toHaveCount(0)
  expect(reads).toHaveLength(1); await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-reconciliation-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '切换只读核对账号' }).click(); await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByText(/索引 7$/)).toBeVisible(); await expect(page.getByText('当前报告已读位置达到目标；不是原写入匹配回执，未知记录保持。', { exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: '只读核对原已读位置' }).click(); await expect(page.getByText('本次只读核对未完成；原未知状态保持，不重发、不重新登录。', { exact: true })).toBeVisible()
  await expect(page.getByText('用户已读写入未核实；仅查询，不重放', { exact: true })).toBeVisible(); expect(reads).toHaveLength(2); expect(reads.every((path) => path.endsWith('/reconcile'))).toBe(true); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-reconciliation-mobile.png', fullPage: true, animations: 'disabled' })
})
