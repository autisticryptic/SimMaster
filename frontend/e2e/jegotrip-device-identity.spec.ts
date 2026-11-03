import { test, expect, type Page } from '@playwright/test'
import type { JegotripDeviceIdentityInput, JegotripDeviceIdentityState } from '../src/api/current'

async function mockApis(page: Page) {
  const state: JegotripDeviceIdentityState = { initialized: true, configured_mode: 'store', max_identities: 64, identities: [] }
  const writes: JegotripDeviceIdentityInput[] = []
  const logins: string[] = []
  // Match root API paths only; /src/api/current.ts is a JavaScript module.
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname
    if (path === '/api/jegotrip/login/setup') {
      await route.fulfill({ json: { status: 'ok', data: { implemented: true, accepting: false, accepting_authenticate: false, native_configured: false,
        configuration_source: 'unconfigured', configuration_revision: 0 } } }); return
    }
    if (path === '/api/jegotrip/app-version') {
      await route.fulfill({ json: { status: 'ok', data: { stored: { version: '8.9.8.0', source: 'manual', observed_at: 1000 }, stored_age_seconds: 0,
        store_ttl_seconds: 21600, configured_file: false, configured_mobile_page: false, configured_ios_lookup: true, configured_static: false, store_enabled: true } } }); return
    }
    if (path === '/api/jegotrip/accounts') { await route.fulfill({ json: { status: 'ok', data: [] } }); return }
    if (path === '/api/jegotrip/device-identities' && request.method() === 'GET') {
      await route.fulfill({ json: { status: 'ok', data: state } }); return
    }
    if (path === '/api/jegotrip/device-identities' && request.method() === 'PUT') {
      const input = request.postDataJSON() as JegotripDeviceIdentityInput
      writes.push(input)
      if (input.id) {
        const row = state.identities.find((item) => item.id === input.id)
        if (!row || input.expected_revision !== row.revision) {
          await route.fulfill({ status: 409, json: { status: 'error', message: 'app_login_device_identity_revision_conflict' } }); return
        }
        row.device_id = input.device_id ?? 'D5C0199B-1644-4D33-8912-B8BD5AB49224'
        row.source = input.device_id ? 'manual' : 'generated'; row.revision += 1
      } else {
        state.identities.push({ id: '0123456789abcdef0123456789abcdef', device_id: input.device_id ?? 'D5C0199B-1644-4D33-8912-B8BD5AB49224',
          source: input.device_id ? 'manual' : 'generated', revision: 1, bound_account_id: null, masked_number: null,
          reserved: false, in_use: false, created_at: 1000, updated_at: 1000 })
      }
      await route.fulfill({ json: { status: 'ok', data: state } }); return
    }
    if (path === '/api/jegotrip/login/attempts' && request.method() === 'POST') logins.push(path)
    await route.fulfill({ status: 503, json: { status: 'error', message: 'app_login_unavailable' } })
  })
  return { state, writes, logins }
}

test('device UI persists the automatic identity and supports a manual CAS update after reopening', async ({ page }) => {
  const model = await mockApis(page)
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  await expect(page.getByRole('combobox', { name: '本次登录的设备标识' })).toContainText('自动生成 / 复用已保存标识')
  await expect(page.getByRole('combobox', { name: '标识生成方式' })).toContainText('随机生成 UUID')
  await page.getByRole('button', { name: '生成并保存随机标识', exact: true }).click()
  await expect(page.getByText('D5C0199B-1644-4D33-8912-B8BD5AB49224', { exact: true })).toBeVisible()
  expect(model.writes[0]).toEqual({ id: null, expected_revision: 0, account_id: null, device_id: null })
  await page.getByRole('button', { name: '设置', exact: true }).click()
  await page.getByRole('textbox', { name: '设备标识', exact: true }).fill('OWN-MANUAL-DEVICE-01')
  page.once('dialog', (dialog) => { void dialog.accept() })
  await page.getByRole('button', { name: '保存新标识', exact: true }).click()
  await expect(page.getByText('OWN-MANUAL-DEVICE-01', { exact: true })).toBeVisible()
  expect(model.writes[1].expected_revision).toBe(1)
  expect(model.writes[1].device_id).toBe('OWN-MANUAL-DEVICE-01')
  await page.reload()
  await expect(page.getByText('OWN-MANUAL-DEVICE-01', { exact: true })).toBeVisible()
  await expect(page.getByText(/手工指定 · 版本 2/)).toBeVisible()
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/desktop.png', fullPage: true })
  expect(model.logins).toEqual([]) // Rendering/CRUD is not a provider login or a trust grant.
})

test('mobile device UI validates a manual value and never claims verification or hosting success', async ({ page }) => {
  const model = await mockApis(page)
  await page.setViewportSize({ width: 390, height: 844 })
  await page.goto('/e2e/fixtures/jegotrip-device.html')
  await page.getByRole('combobox', { name: '本次登录的设备标识' }).click()
  await page.getByRole('option', { name: '手工指定自己的标识', exact: true }).click()
  const input = page.getByRole('textbox', { name: '首次登录使用的设备标识', exact: true })
  await input.fill('invalid:device')
  await expect(input).toHaveAttribute('aria-invalid', 'true')
  await input.fill('OWN-MOBILE-DEVICE-01')
  await expect(input).toHaveAttribute('aria-invalid', 'false')
  await expect(page.getByText(/生成或更换标识不代表绕过一键验证/)).toBeVisible()
  await expect(page.getByRole('button', { name: '登录并启动无忧行', exact: true })).toBeDisabled()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/mobile.png', fullPage: true })
  expect(model.writes).toEqual([]); expect(model.logins).toEqual([])
})
