import { test, expect } from '@playwright/test'
const privateFields = { uid: 'private-uid', token: 'private-token', endpoint: 'private-route', raw_relations: 'private-data', online: true, write_grant: true }
const relations = { operation: 'relations', target_kind: 'current_user', accepted: true, reported: false, original_time: '-9223372036854775808', first_relations: 1, second_relations: 2, names: 3, properties: 0, ...privateFields }
const properties = { operation: 'properties', target_kind: 'group', accepted: true, first_relations: 0, second_relations: 0, names: 0, properties: 2, ...privateFields }
test('original Group Buddy directory requires independent explicit READ consent and preserves full signed64 private result projection', async ({ page }) => {
  const reads: unknown[] = []; const errors: string[] = []; let consent = false
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { if (consent) { void dialog.accept() } else { void dialog.dismiss() } })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); expect(request.method()).toBe('POST'); expect(new URL(request.url()).pathname).toBe('/api/non-3gpp/accounts/app-account-0123456789abcdef0123456789abcdef/native-directory/read')
    const body: unknown = request.postDataJSON(); reads.push(body)
    expect(body).toEqual(reads.length === 1 ? { expected_revision: 1, operation: 'relations', since: '9223372036854775807', consent: true } : { expected_revision: 1, operation: 'properties', group_id: '1+2', since: '-1', consent: true })
    await route.fulfill({ headers: { 'cache-control': 'no-store' }, json: { status: 'ok', data: reads.length === 1 ? relations : properties } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/native-directory.html')
  const button = page.getByRole('button', { name: '明确读取原目录（只读）', exact: true }); const id = page.getByLabel('原群组 ID（关系查询可留空）'); const since = page.getByLabel('原目录 since（signed64 字符串）')
  await expect(button).toBeEnabled(); expect(reads).toEqual([]); await button.click(); expect(reads).toEqual([])
  await id.fill('15900000000'); await expect(button).toBeDisabled(); await id.fill('Org/1-2'); await expect(button).toBeDisabled(); await id.fill('')
  await since.fill('9223372036854775808'); await expect(button).toBeDisabled(); await since.fill('9223372036854775807'); consent = true; await button.click()
  await expect(page.getByText('原关系集合 A 1 项 · 集合 B 2 项 · 名称集合 3 项', { exact: true })).toBeVisible(); await expect(page.getByText('原 reported 字段：false · 原始时间值：-9223372036854775808（不推断墙上时钟或在线状态）', { exact: true })).toBeVisible()
  await expect(page.getByText('目标：当前原 SDK 用户 · 操作：关系目录', { exact: true })).toBeVisible(); await expect(page.getByText('private-', { exact: false })).toHaveCount(0); expect(reads).toHaveLength(1)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/native-directory-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('combobox', { name: '目录读取操作' }).click(); await page.getByRole('option', { name: '原群组属性观测', exact: true }).click(); await expect(button).toBeDisabled(); await expect(since).toHaveCount(0); await id.fill('1+2'); await button.click()
  await expect(page.getByText('原属性集合 2 项；私有属性值、关系标识和当前 SDK 凭据不从本视图导出。', { exact: true })).toBeVisible(); await expect(page.getByText('目标：本次原群组 · 操作：群组属性', { exact: true })).toBeVisible()
  await expect(page.getByText('private-', { exact: false })).toHaveCount(0); expect(reads).toHaveLength(2); await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); expect(errors).toEqual([])
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/native-directory-mobile.png', fullPage: true, animations: 'disabled' })
})
test('original directory discards old account revision and stopped-scope results without automatic queries or unknown WRITE replay', async ({ page }) => {
  const reads: { path: string; body: unknown }[] = []; const errors: string[] = []; let release: (() => void) | undefined
  page.on('pageerror', (error) => errors.push(error.message)); page.on('dialog', (dialog) => { void dialog.accept() })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; const body: unknown = request.postDataJSON(); expect(request.method()).toBe('POST'); expect(path.endsWith('/native-directory/read')).toBe(true); reads.push({ path, body })
    const index = reads.length; expect(body).toEqual({ expected_revision: index === 4 ? 2 : 1, operation: 'relations', since: '-1', consent: true })
    if (index === 1 || index === 3) await new Promise<void>((resolve) => { release = resolve })
    if (index === 4) { await route.abort(); return }
    await route.fulfill({ json: { status: 'ok', data: index === 2 ? { ...relations, original_time: 1 } : relations } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/native-directory.html')
  const button = page.getByRole('button', { name: '明确读取原目录（只读）', exact: true }); const success = page.getByText('原关系集合 A 1 项 · 集合 B 2 项 · 名称集合 3 项', { exact: true })
  await button.click(); await expect.poll(() => reads.length).toBe(1); await page.getByRole('button', { name: '切换目录账号', exact: true }).click(); release?.(); await expect(button).toBeEnabled(); await expect(success).toHaveCount(0); expect(reads).toHaveLength(1)
  await button.click(); await expect(page.getByText('原目录读取结果未核实；不会自动重试、重新登录、确认消息位置或重放未知 WRITE。', { exact: true })).toBeVisible(); expect(reads[1].path).toContain('app-account-fedcba9876543210fedcba9876543210'); expect(reads).toHaveLength(2)
  await button.click(); await expect.poll(() => reads.length).toBe(3); await page.getByRole('button', { name: '替换目录配置版本', exact: true }).click(); release?.(); await expect(button).toBeEnabled(); await expect(success).toHaveCount(0); expect(reads).toHaveLength(3)
  await button.click(); await expect(page.getByText('原目录读取结果未核实；不会自动重试、重新登录、确认消息位置或重放未知 WRITE。', { exact: true })).toBeVisible(); expect(reads).toHaveLength(4); await expect(page.getByText('private-', { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/native-directory-generation-desktop.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '退出原托管状态', exact: true }).click(); await expect(button).toBeDisabled(); await expect(page.getByText('当前原 SDK 未托管；不会自动启动、重新登录或建立其他连接。', { exact: true })).toBeVisible(); expect(reads).toHaveLength(4)
  await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); expect(errors).toEqual([]); await page.screenshot({ path: 'test-results/jegotrip-device-ui/native-directory-generation-mobile.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '关闭原目录策略', exact: true }).click(); await expect(button).toHaveCount(0); expect(reads).toHaveLength(4)
})
