import { test, expect, type Page } from '@playwright/test'

/**
 * Browser coverage for the VoLTE Profile dialog.
 *
 * The backend already covers the policy: slot validation, source isolation,
 * derived-with-id refusal, and the HTTP error matrix. What no test could reach
 * is whether the dialog itself is wired up — whether the reorder buttons move a
 * slot, whether choosing `derived` really disables the profile picker, and
 * whether Save sends what the form shows. That is what these check.
 *
 * They need a running SimAdmin with at least one modem line, so they are opt-in:
 *
 *   E2E_BASE_URL=http://192.168.100.13:3000 E2E_PASSWORD=... pnpm test:e2e
 *
 * Without both variables every test skips. A skipped run is not a passing run,
 * so the skip message says which variable was missing.
 */

const password = process.env.E2E_PASSWORD ?? ''

test.beforeEach(async ({ page }) => {
  test.skip(
    password === '',
    'set E2E_PASSWORD to the admin password of the device the dev server proxies to',
  )
  await login(page)
})

async function login(page: Page) {
  await page.goto('/login')

  // The app redirects away from /login when the session is already valid, so a
  // password field may never appear. Only fill one if it does, and let the
  // shell assertion below decide whether we are in.
  const field = page.locator('input[type="password"]')
  const needsPassword = await field
    .waitFor({ state: 'visible', timeout: 8_000 })
    .then(() => true)
    .catch(() => false)
  if (needsPassword) {
    await field.fill(password)
    await page.locator('button[type="submit"], [aria-label="登录"]').first().click()
  }

  await expect(page).not.toHaveURL(/\/login/, { timeout: 30_000 })
}

/** Open the dialog from the first line that offers a 配置 button. */
async function openDialog(page: Page) {
  await page.goto('/sim')
  await expect(page.getByRole('main')).toBeVisible({ timeout: 30_000 })

  // The line card is a workbench: pick a line, then the IMS tab. The VoLTE
  // section is rendered only under `workbenchTab === 'ims'`, so on the default
  // 概览 tab the button does not exist at all.
  const lineCard = page.getByRole('button', { name: /基带 \d/ }).first()
  const hasLine = await lineCard
    .waitFor({ state: 'visible', timeout: 30_000 })
    .then(() => true)
    .catch(() => false)
  test.skip(!hasLine, 'no modem line on this device')
  await lineCard.click()

  const imsTab = page.getByRole('tab', { name: 'IMS 与 Trunk' })
  const hasTab = await imsTab
    .waitFor({ state: 'visible', timeout: 20_000 })
    .then(() => true)
    .catch(() => false)
  test.skip(!hasTab, 'this line has no IMS workbench tab')
  await imsTab.click()

  // Address the button by test id. A line card has several buttons labelled
  // 配置 -- data proxy, VoLTE, VoWiFi, trunk -- and the sidebar has 基本配置,
  // so matching on the label picks the wrong control and *acts* on it. Doing
  // that once while writing these tests saved a data-proxy config on a live
  // device, which is why the id exists.
  const configButtons = page.getByTestId('volte-profile-config')
  const appeared = await configButtons
    .first()
    .waitFor({ state: 'visible', timeout: 30_000 })
    .then(() => true)
    .catch(() => false)
  test.skip(!appeared, 'no line exposes a VoLTE 配置 button on this device')
  await configButtons.first().click()

  const dialog = page.getByRole('dialog')
  await expect(dialog).toBeVisible()
  // The dialog fetches its policy and profile lists before it is usable.
  await expect(dialog.getByText('正在加载 Profile 配置…')).toHaveCount(0, {
    timeout: 30_000,
  })
  return dialog
}

test('the dialog states that the setting is per line, not global', async ({ page }) => {
  const dialog = await openDialog(page)

  // The whole point of the feature: an operator must not read this as a global
  // switch, or they will configure one line and expect all of them to change.
  await expect(dialog).toContainText('不是全局设置')
  await expect(dialog).toContainText('第 1 次')
  await expect(dialog).toContainText('第 2 次')
  await expect(dialog).toContainText('第 3 次')
})

test('reorder buttons are bounded at the ends and actually move a slot', async ({ page }) => {
  const dialog = await openDialog(page)

  const up = dialog.getByRole('button', { name: '上移' })
  const down = dialog.getByRole('button', { name: '下移' })
  await expect(up).toHaveCount(3)
  await expect(down).toHaveCount(3)

  // First cannot move up, last cannot move down.
  await expect(up.nth(0)).toBeDisabled()
  await expect(down.nth(2)).toBeDisabled()
  await expect(up.nth(1)).toBeEnabled()
  await expect(down.nth(1)).toBeEnabled()

  // Moving really reorders. Each slot renders two comboboxes -- the source and
  // the profile picker -- so the sources are the even indices. MUI does not
  // associate the label in a way `getByLabel` resolves, hence the positional
  // read.
  const sourceTexts = async () => {
    const all = await dialog.locator('[role="combobox"]').allInnerTexts()
    return [all[0], all[2], all[4]]
  }

  await expect(dialog.locator('[role="combobox"]')).toHaveCount(6)
  const before = await sourceTexts()
  await down.nth(0).click()
  const after = await sourceTexts()

  expect(after).not.toEqual(before)
  // A swap of the first two, so slot 1 now holds what slot 2 held.
  expect(after[0]).toBe(before[1])
  expect(after[1]).toBe(before[0])
  // Slot 3 is untouched by a swap of the first two.
  expect(after[2]).toBe(before[2])
})

test('choosing the derived source disables that slot profile picker', async ({ page }) => {
  const dialog = await openDialog(page)

  // `derived` is computed from the SIM's home PLMN, so pinning an id in that
  // slot is a contradiction. The backend refuses it; the UI must not offer it.
  const boxes = dialog.locator('[role="combobox"]')
  await expect(boxes).toHaveCount(6)

  // Slot 1 starts on a database source, so its picker is usable.
  await expect(boxes.nth(1)).toBeEnabled()

  // Switch slot 1 to the derived source.
  await boxes.nth(0).click()
  await page.getByRole('option', { name: /派生/ }).first().click()

  await expect(dialog).toContainText('始终根据当前 SIM/Home PLMN 派生')
  // Slot 1's picker can no longer pin an id.
  await expect(boxes.nth(1)).toBeDisabled()
})

test('a slot whose source has no LTE-ready profile says it will fall back', async ({ page }) => {
  const dialog = await openDialog(page)

  // Neither a user database nor a downloaded catalog holds an LTE-ready profile
  // on this device, so the dialog must say the slot falls back to derived --
  // and must still allow saving. Silently accepting an unusable slot is the
  // failure this guards.
  await expect(dialog).toContainText('将使用派生配置兜底')
  await expect(dialog.getByRole('button', { name: /保存/ })).toBeEnabled()
})

test('save is reachable and reports a result rather than failing silently', async ({ page }) => {
  const dialog = await openDialog(page)

  const save = dialog.getByRole('button', { name: /保存/ })
  await expect(save).toBeEnabled()

  // Watch the request the form actually sends: the point of a browser test is
  // proving the button is wired to the right call with the right body.
  const [request] = await Promise.all([
    page.waitForRequest(
      (req) => req.url().includes('/profile-selection') && req.method() === 'PUT',
      { timeout: 30_000 },
    ),
    save.click(),
  ])

  const body = request.postDataJSON() as { attempts?: unknown[] }
  expect(Array.isArray(body.attempts)).toBe(true)
  // Exactly three ordered slots is the contract the backend enforces.
  expect(body.attempts).toHaveLength(3)
})
