import { test, expect, type Page } from '@playwright/test'
import type { EsimProfile } from '../src/api/contracts'

const EID = '89033023000000000000000000000123'
const profiles = (): EsimProfile[] => Array.from({ length: 6 }, (_, i) => ({ iccid: `890000000000000000${i}`, name: `Profile ${i}`, provider: 'Fixture carrier', state: i === 0 ? 'enabled' : 'disabled', class: 'operational', delete_allowed: i !== 2, imsi: '001010000000001', msisdn: '+12025550123', raw: null }))

async function setup(page: Page, euiccOverrides: Record<string, unknown> = {}) {
  const requests: Array<{ path: string; method: string; body: unknown }> = []
  const byLine: Record<string, EsimProfile[]> = { 'line-a': profiles(), 'line-b': [{ ...profiles()[0], name: 'Line B profile' }] }
  let releaseEnable: (() => void) | undefined
  let blockEnable = false
  // Deny all non-local requests and fully intercept every API, including unknown
  // ones, so this suite can never mutate host/device/IMS state.
  await page.route('**/*', async (route) => {
    const url = new URL(route.request().url())
    if (url.hostname !== '127.0.0.1') return route.abort()
    // Let Vite transform the local HTML fixture and inject its React preamble.
    if (!url.pathname.startsWith('/api/')) return route.continue()
    const method = route.request().method()
    const body: unknown = route.request().postDataJSON()
    requests.push({ path: url.pathname, method, body })
    const respond = (data: unknown) => route.fulfill({ json: { status: 'success', data } })
    if (url.pathname === '/api/esim/lpac/status') return respond({ usable: true })
    const lineId = url.pathname.match(/\/lines\/([^/]+)/)?.[1] || 'line-a'
    if (url.pathname.endsWith('/esim/euicc')) return respond({ eid: EID, manufacturer: lineId === 'line-a' ? 'Thales' : '', memory_available_kb: 192, raw: { EuiccConfiguredAddresses: { defaultDpAddress: 'default.example' } }, ...euiccOverrides })
    if (/restart.*status|restart-status/.test(url.pathname)) return respond({ running: false, steps: [] })
    if (url.pathname.endsWith('/esim/profiles') && method === 'GET') return respond({ profiles: byLine[lineId] })
    if (url.pathname.endsWith('/rename')) {
      const iccid = url.pathname.split('/').at(-2)
      byLine[lineId] = byLine[lineId].map((profile) => profile.iccid === iccid ? { ...profile, name: (body as { name: string }).name } : profile)
    } else if (url.pathname.endsWith('/enable')) {
      if (blockEnable) await new Promise<void>((resolve) => { releaseEnable = resolve })
      const iccid = url.pathname.split('/').at(-2)
      byLine[lineId] = byLine[lineId].map((profile) => ({ ...profile, state: profile.iccid === iccid ? 'enabled' : 'disabled' }))
    } else if (method === 'DELETE') {
      const iccid = url.pathname.split('/').at(-1)
      byLine[lineId] = byLine[lineId].filter((profile) => profile.iccid !== iccid)
    } else if (url.pathname.endsWith('/esim/profiles') && method === 'POST') {
      byLine[lineId].push({ ...profiles()[1], iccid: 'downloaded', name: 'Downloaded profile' })
    } else return route.fulfill({ status: 500, json: { message: `Unmocked API blocked: ${url.pathname}` } })
    return respond({ code: 0, status: 'success', action: 'fixture', msg: 'ok' })
  })
  await page.goto('/e2e/fixtures/esim.html')
  await expect(page.getByTestId('esim-profile-card')).toHaveCount(6)
  return { requests, blockSwitch: () => { blockEnable = true }, releaseSwitch: () => releaseEnable?.() }
}

test('two summary rows, private EID copy, manufacturer, four small buttons and adaptive columns', async ({ page, context }, testInfo) => {
  await context.grantPermissions(['clipboard-read', 'clipboard-write'])
  // Obsolete backend fields must not restore the removed capacity editor.
  const fixture = await setup(page, { memory_total_customizable: true, memory_total_kb: 512 })
  await expect(page.getByText('Thales', { exact: true })).toBeVisible()
  await expect(page.getByText('完整管理', { exact: true })).toHaveCount(0)
  await expect(page.getByText('存储占用', { exact: true })).toHaveCount(0)
  await expect(page.getByText('192 KB', { exact: true })).toBeVisible()
  await expect(page.getByText(/自定义.*容量|总容量|512 KB/)).toHaveCount(0)
  const eidCard = page.getByTestId('esim-eid-card')
  const eidLabel = page.getByTestId('esim-eid')
  await expect(eidLabel).toHaveText('890330••••0123')
  expect(await eidLabel.evaluate((element) => element.closest('button, [role="button"]'))).toBeNull()
  await expect(eidCard.getByRole('button')).toHaveCount(1)
  await expect(eidCard.getByRole('button')).toHaveAccessibleName('复制完整 EID')
  await expect(eidCard.getByRole('button')).toHaveText('')
  expect(await page.locator('html').innerHTML()).not.toContain(EID)
  const summary = page.getByTestId('esim-summary')
  const children = await summary.locator(':scope > div').evaluateAll((items) => items.map((item) => { const rect = item.getBoundingClientRect(); return { x: rect.x, y: rect.y } }))
  expect(children[0].y).toBe(children[1].y)
  expect(children[2].y).toBe(children[3].y)
  expect(children[2].y).toBeGreaterThan(children[0].y)
  expect(children[0].x).toBe(children[2].x)
  await page.getByRole('button', { name: '复制完整 EID' }).click()
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(EID)
  expect(await page.locator('body').innerHTML()).not.toContain(EID)
  await page.getByTestId('esim-profile-card').first().getByRole('button', { name: '详情', exact: true }).click()
  await expect(page.getByRole('dialog')).toContainText('default.example')
  expect(await page.locator('body').innerHTML()).not.toContain(EID)
  await page.getByRole('button', { name: '关闭', exact: true }).click()
  for (const width of [360, 900, 1600]) {
    await page.setViewportSize({ width, height: 900 })
    expect(await eidCard.evaluate((card) => card.scrollWidth > card.clientWidth)).toBe(false)
    const cards = page.getByTestId('esim-profile-card')
    const first = cards.first()
    await expect(first.getByRole('button')).toHaveText(['详情', '重命名', '切换', '删除'])
    const metrics = await first.evaluate((card) => {
      const buttons = Array.from(card.querySelectorAll('button'))
      return { width: card.clientWidth, overflow: card.scrollWidth > card.clientWidth, buttons: buttons.map((button) => ({ x: button.getBoundingClientRect().x, y: button.getBoundingClientRect().y, small: button.classList.contains('MuiButton-sizeSmall'), overflow: button.scrollWidth > button.clientWidth })) }
    })
    expect(metrics.overflow).toBe(false)
    expect(metrics.buttons.every((button) => button.small && !button.overflow && button.y === metrics.buttons[0].y)).toBe(true)
    expect(metrics.buttons.map((button) => button.x)).toEqual([...metrics.buttons.map((button) => button.x)].sort((a, b) => a - b))
    if (width === 1600) {
      const columns = await page.getByTestId('esim-profiles').evaluate((grid) => getComputedStyle(grid).gridTemplateColumns.split(' ').length)
      expect(columns).toBeGreaterThan(2)
    }
    if (width !== 900) await page.screenshot({ path: testInfo.outputPath(`esim-${width}.png`), fullPage: true })
  }
  expect(fixture.requests.filter((request) => request.path.endsWith('/esim/euicc'))).toHaveLength(1)
  expect(fixture.requests.filter((request) => request.path.endsWith('/esim/profiles'))).toHaveLength(1)
  expect(fixture.requests.filter((request) => request.path.endsWith('/esim/config'))).toHaveLength(0)
  expect(fixture.requests.every((request) => request.method === 'GET')).toBe(true)
})

for (const value of ['123456', 'not-a-valid-eid', `${EID.slice(0, -1)}X`]) {
  test(`malformed EID is fully masked but copies its actual value (${value.length} characters)`, async ({ page, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    await setup(page, { eid: value })
    await expect(page.getByTestId('esim-eid')).toHaveText('••••')
    const copy = page.getByTestId('esim-eid-card').getByRole('button')
    await expect(copy).toHaveCount(1)
    await expect(copy).toHaveAccessibleName('复制完整 EID')
    expect(await page.locator('html').innerHTML()).not.toContain(value)
    await copy.click()
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(value)
    expect(await page.locator('html').innerHTML()).not.toContain(value)
  })
}

test('missing EID disables copy and a legacy total never fabricates available storage', async ({ page }) => {
  const fixture = await setup(page, { eid: '', memory_available_kb: null, memory_total_kb: 512, memory_total_customizable: true })
  await expect(page.getByTestId('esim-eid')).toHaveText('未读取')
  await expect(page.getByTestId('esim-eid-card').getByRole('button')).toHaveCount(1)
  await expect(page.getByRole('button', { name: '复制完整 EID' })).toBeDisabled()
  const storage = page.getByTestId('esim-summary').locator(':scope > div').nth(1)
  await expect(storage).toHaveText('剩余存储未读取')
  await expect(storage.getByRole('button')).toHaveCount(0)
  await expect(page.getByText(/自定义.*容量|总容量|512 KB/)).toHaveCount(0)
  expect(fixture.requests.filter((request) => request.path.endsWith('/esim/config'))).toHaveLength(0)
})

test('cancelled profile and download dialogs never submit writes', async ({ page }) => {
  const fixture = await setup(page)
  const card = page.getByTestId('esim-profile-card').nth(1)
  for (const action of ['重命名', '切换', '删除']) {
    await card.getByRole('button', { name: action, exact: true }).click()
    await expect(page.getByRole('dialog')).toBeVisible()
    await page.getByRole('button', { name: '取消', exact: true }).click()
    await expect(page.getByRole('dialog')).toHaveCount(0)
  }
  await page.getByRole('button', { name: '下载', exact: true }).click()
  await page.getByLabel('LPA 激活码', { exact: true }).fill('LPA:1$dp.example$cancelled-id')
  await page.getByRole('button', { name: '取消', exact: true }).click()
  await expect(page.getByRole('dialog')).toHaveCount(0)
  expect(fixture.requests.every((request) => request.method === 'GET')).toBe(true)
})

test('cards expose individual details/rename/delete/download flows with validations', async ({ page }) => {
  const fixture = await setup(page)
  const cards = page.getByTestId('esim-profile-card')
  await expect(cards.nth(0).getByRole('button', { name: '删除', exact: true })).toBeDisabled()
  await expect(cards.nth(2).getByRole('button', { name: '删除', exact: true })).toBeDisabled()
  await cards.nth(1).getByRole('button', { name: '重命名', exact: true }).click()
  await page.getByLabel('Profile 名称').fill('   ')
  await expect(page.getByRole('button', { name: '保存', exact: true })).toBeDisabled()
  await page.getByLabel('Profile 名称').fill(' Renamed ')
  await page.getByRole('button', { name: '保存', exact: true }).click()
  await expect(cards.nth(1)).toContainText('Renamed')
  await cards.nth(1).getByRole('button', { name: '删除', exact: true }).click()
  await expect(page.getByRole('button', { name: '确认删除', exact: true })).toBeDisabled()
  await page.getByLabel('请输入 确认删除').fill('确认删除')
  await page.getByRole('button', { name: '确认删除', exact: true }).click()
  await expect(cards).toHaveCount(5)
  await page.getByRole('button', { name: '下载', exact: true }).click()
  await expect(page.getByRole('button', { name: '开始写卡' })).toBeDisabled()
  await page.getByLabel('LPA 激活码', { exact: true }).fill('LPA:1$dp.example$fixture-id$confirm')
  await expect(page.getByLabel('SM-DP+ 服务器地址')).toHaveValue('dp.example')
  await page.getByLabel('绑定 IMEI (选填)').fill('bad')
  await expect(page.getByRole('button', { name: '开始写卡' })).toBeDisabled()
  await page.getByLabel('绑定 IMEI (选填)').fill('123456789012345')
  await page.getByRole('button', { name: '开始写卡' }).click()
  await expect(cards).toHaveCount(6)
  expect(fixture.requests.find((request) => request.method === 'POST' && request.path.endsWith('/esim/profiles'))?.body).toEqual({ smdp: 'dp.example', matching_id: 'fixture-id', confirmation_code: 'confirm', imei: '123456789012345' })
  expect(await page.locator('body').innerHTML()).not.toContain(EID)
})

test('switch lock and cache survive line navigation without leaking another line state', async ({ page }) => {
  const fixture = await setup(page)
  fixture.blockSwitch()
  await page.getByTestId('esim-profile-card').nth(1).getByRole('button', { name: '切换', exact: true }).click()
  await page.getByRole('button', { name: '确认切换', exact: true }).click()
  await expect.poll(() => fixture.requests.filter((request) => request.path.endsWith('/enable')).length).toBe(1)
  await expect(page.getByRole('button', { name: '确认切换', exact: true })).toBeDisabled()
  // Escape cannot close a running operation. Navigation unmount is separately
  // exercised through the fixture buttons, as in switching workbench lines.
  await page.keyboard.press('Escape')
  await expect(page.getByRole('dialog')).toBeVisible()
  await page.getByRole('button', { name: '测试线路 B', includeHidden: true }).evaluate((button: HTMLButtonElement) => button.click())
  await expect(page.getByTestId('esim-profile-card')).toHaveCount(1)
  await expect(page.getByText('未知 eUICC 厂商')).toBeVisible()
  await expect(page.getByTestId('esim-profile-card')).toContainText('Line B profile')
  await page.getByRole('button', { name: '测试线路 A' }).click()
  await expect(page.getByTestId('esim-profile-card')).toHaveCount(6)
  await expect(page.getByRole('button', { name: '下载', exact: true })).toBeDisabled()
  fixture.releaseSwitch()
  await expect(page.getByTestId('esim-profile-card').nth(1)).toContainText('已启用')
  await expect(page.getByRole('button', { name: '下载', exact: true })).toBeEnabled()
  expect(fixture.requests.filter((request) => request.path.endsWith('/enable'))).toHaveLength(1)
  expect(fixture.requests.filter((request) => request.path.includes('/line-b/') && request.method !== 'GET')).toHaveLength(0)
})
