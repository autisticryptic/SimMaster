import { test, expect } from '@playwright/test'
test('UID-local UI requires explicit current account intent and never labels local input as a remote reply or ready grant', async ({ page }) => {
  const requests: string[] = []; const errors: string[] = []; let consent = false
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { if (consent) { void dialog.accept() } else { void dialog.dismiss() } })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; expect(request.method()).toBe('POST'); expect(path.endsWith('/user-entry/lookup')).toBe(true)
    requests.push(path); expect(request.postDataJSON()).toEqual({ expected_revision: 1, recipient: '[uid:7_9@foreign.invalid]', consent: true })
    await route.fulfill({ json: { status: 'ok', data: { kind: 'uid', report: 'local_uid', observed_at: '2026-01-01T00:00:00Z', uid: 'private-uid', token: 'private-token' } } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/uid-local.html')
  const input = page.getByLabel('收件人原 URI'); const button = page.getByRole('button', { name: '只读解析收件人', exact: true })
  expect(requests).toEqual([]); await input.fill('[phone:+12345@fixture.invalid]'); await expect(button).toBeDisabled()
  await input.fill('[uid:7_9@foreign.invalid]'); await button.click(); expect(requests).toEqual([]); consent = true; await button.click()
  await expect(page.getByText('本次仅本地识别原 UID；未查询服务端，不是回执或授信。', { exact: true })).toBeVisible()
  await expect(page.getByText('本次原服务端报告已解析；不是写入回执或接收就绪。', { exact: true })).toHaveCount(0)
  await expect(page.getByText('private-', { exact: false })).toHaveCount(0); expect(requests).toHaveLength(1)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/uid-local-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '切换 UID 本地账号' }).click(); await expect(input).toHaveValue(''); await expect(button).toBeDisabled()
  await expect(page.getByText('本次仅本地识别原 UID；未查询服务端，不是回执或授信。', { exact: true })).toHaveCount(0)
  await page.setViewportSize({ width: 390, height: 844 }); await input.fill('7_09'); await expect(button).toBeDisabled(); await input.fill('[uid:7_9@foreign.invalid]'); await button.click()
  await expect(page.getByText('本次仅本地识别原 UID；未查询服务端，不是回执或授信。', { exact: true })).toBeVisible(); expect(requests).toHaveLength(2); expect(errors).toEqual([])
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/uid-local-mobile.png', fullPage: true, animations: 'disabled' })
})
