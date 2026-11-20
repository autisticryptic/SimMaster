import { test, expect } from '@playwright/test'

test('original Info content stays typed private and non-authorizing through mixed history deletion and account generations', async ({ page }) => {
  const writes: string[] = []; const errors: string[] = []; const dialogs: string[] = []; let deleted = false
  page.on('pageerror', (error) => errors.push(error.message))
  page.on('dialog', (dialog) => { dialogs.push(dialog.message()); void dialog.accept() })
  await page.addInitScript(() => { (window as Window & { infoSentinel?: number }).infoSentinel = 0 })
  const base = { box_kind: 'p2p', message_index: '9223372036854775807', attachment_count: 1, received_at: '2026-01-01T00:00:00Z' }
  const privateFields = { body: 'private-info-raw', sender: 'private-info-sender', identity_key: 'private-info-key', info_epoch: 'private-info-epoch', OnlineMsg: 'private-online-claim', token: 'private-info-token', endpoint: 'private-info-endpoint', grant: true, im_user_read: { mark_read: true }, incoming_message_ready: true }
  const infoText = '原 Info 内容 <script>window.infoSentinel=99</script>'
  const systemText = '系统结构化内容，不是在线确认'
  const oldRows: Record<string, unknown>[] = [
    { ...base, id: 1, text: '保留的原 Text 内容' },
    { ...base, ...privateFields, id: 2, kind: 'info', info_type: 'Notice.01', system_info: false, text: infoText },
    { ...base, ...privateFields, id: 3, box_kind: 'org', kind: 'info', info_type: 'SystemState', system_info: true, text: systemText },
    { ...base, id: 4, kind: 'info', info_type: 'bad/type', system_info: false, text: 'invalid-info-type' },
    { ...base, id: 5, kind: 'info', info_type: 'Notice', system_info: 'true', text: 'invalid-info-system' },
    { ...base, id: 6, info_type: 'Notice', system_info: false, text: 'invalid-info-kind' },
  ]
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname
    expect(path).toMatch(/^\/api\/non-3gpp\/accounts\/app-account-[0-9a-f]{32}\/im-messages(?:\/[0-9]+)?$/)
    const old = path.includes('0123456789abcdef0123456789abcdef')
    if (request.method() !== 'GET') {
      expect(request.method()).toBe('DELETE'); writes.push(`${request.method()} ${path}`)
      expect(path.endsWith(old ? '/2' : '/7')).toBe(true)
      if (!old) { await route.abort(); return }
      deleted = true; await route.fulfill({ headers: { 'cache-control': 'no-store' }, json: { status: 'ok', data: true } }); return
    }
    const rows = old ? oldRows.filter((row) => !deleted || row.id !== 2) : [{ ...base, ...privateFields, id: 7, kind: 'info', info_type: 'OtherAccount', system_info: false, text: '另一账号原 Info 内容' }]
    await route.fulfill({ headers: { 'cache-control': 'no-store' }, json: { status: 'ok', data: rows } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/app-im-messages.html')
  await expect(page.getByText(infoText, { exact: true })).toBeVisible(); await expect(page.getByText(systemText, { exact: true })).toBeVisible()
  await expect(page.getByText('保留的原 Text 内容', { exact: true })).toBeVisible()
  await expect(page.getByText(/原 Info · 类型 Notice\.01/)).toBeVisible(); await expect(page.getByText(/原系统 Info · 类型 SystemState/)).toBeVisible()
  await expect(page.getByText(/不是运营商短信/)).toBeVisible(); await expect(page.getByText(/不据此确认在线、渠道送达或执行命令/)).toHaveCount(2)
  for (const hidden of ['private-info-raw', 'private-info-sender', 'private-info-key', 'private-info-epoch', 'private-online-claim', 'private-info-token', 'private-info-endpoint', 'invalid-info-type', 'invalid-info-system', 'invalid-info-kind']) await expect(page.getByText(hidden, { exact: false })).toHaveCount(0)
  await expect(page.getByRole('button', { name: '明确标记原消息已读', exact: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: '通过原收件人解析标记已读', exact: true })).toHaveCount(0)
  expect(await page.evaluate(() => (window as Window & { infoSentinel?: number }).infoSentinel)).toBe(0); expect(writes).toEqual([]); expect(errors).toEqual([])
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-info-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '删除本地 Info 内容', exact: true }).first().click()
  await expect(page.getByText(infoText, { exact: true })).toHaveCount(0); await expect(page.getByText(systemText, { exact: true })).toBeVisible()
  await expect(page.getByText('保留的原 Text 内容', { exact: true })).toBeVisible(); expect(writes).toHaveLength(1)
  expect(dialogs[0]).toMatch(/删除本地 Info 内容与类型/)
  await page.getByRole('button', { name: '切换原账号', exact: true }).click(); await page.setViewportSize({ width: 390, height: 844 })
  await expect(page.getByText('另一账号原 Info 内容', { exact: true })).toBeVisible()
  for (const old of [infoText, systemText, '保留的原 Text 内容']) await expect(page.getByText(old, { exact: true })).toHaveCount(0)
  const currentDelete = page.getByRole('button', { name: '删除本地 Info 内容', exact: true })
  await currentDelete.click(); await expect(page.getByText(/删除结果尚未核实/)).toBeVisible(); await expect(currentDelete).toBeDisabled()
  await currentDelete.evaluate((button) => (button as HTMLButtonElement).click())
  expect(writes).toHaveLength(2); expect(writes[0]).toContain('0123456789abcdef0123456789abcdef/im-messages/2'); expect(writes[1]).toContain('fedcba9876543210fedcba9876543210/im-messages/7')
  expect(errors).toEqual([]); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  expect(await page.evaluate(() => (window as Window & { infoSentinel?: number }).infoSentinel)).toBe(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-info-mobile.png', fullPage: true, animations: 'disabled' })
})
