import { test, expect } from '@playwright/test'
import { createHash } from 'node:crypto'
const tag = (nonce: string) => createHash('sha256').update(`original-im-send-ui-nonce-v1\0${nonce}`).digest('hex')
test('original IM send requires current history and business confirmation and unknown nonce is never cleared by another terminal row', async ({ page }) => {
  let writes = 0; let consent = false; let history: object[] = []; let historyReads = 0
  page.on('dialog', (dialog) => { if (consent) { void dialog.accept() } else { void dialog.dismiss() } })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const path = new URL(request.url()).pathname; const old = !path.includes('fedcba')
    expect(path.endsWith('/im-outbox')).toBe(true)
    if (request.method() === 'POST') {
      writes += 1; const data = request.postDataJSON(); expect(data).toEqual({ expected_revision: 1, nonce: expect.stringMatching(/^[0-9a-f]{32}$/), recipient: '1_2', content: 'explicit original IM body', consent: true, write_consent: true })
      if (writes === 1) { const row = { id: 1, nonce_tag: tag(data.nonce), kind: 'text', state: 'confirmed', message_index: '9223372036854775807', original_time: '-1' }; history = [row]; await route.fulfill({ json: { status: 'ok', data: { ...row, uri: 'private-uri', token: 'private-token' } } }); return }
      // An unrelated newer terminal for the same kind must not resolve this
      // request's lost response; its opaque nonce tag is different.
      history = [{ id: 2, nonce_tag: 'f'.repeat(64), kind: 'text', state: 'rejected', message_index: null, original_time: null }, ...history]; await route.abort(); return
    }
    historyReads += 1; expect(request.method()).toBe('GET'); expect(new URL(request.url()).searchParams.get('limit')).toBe('128'); await route.fulfill({ json: { status: 'ok', data: old ? history : [] } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-send.html')
  await page.getByRole('textbox', { name: '原 IM 收件人（UID / GroupID / 原 URI）' }).fill('1_2'); await page.getByRole('textbox', { name: '原 IM 内容' }).fill('explicit original IM body')
  const button = page.getByRole('button', { name: '明确执行原 IM 外发', exact: true }); await expect(button).toBeEnabled(); expect(writes).toBe(0); await button.click(); expect(writes).toBe(0); consent = true; await button.click(); await expect(page.getByText(/原消息 ID 9223372036854775807/)).toBeVisible(); await expect(page.getByText('private-', { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-send-desktop.png', fullPage: true, animations: 'disabled' })
  await button.click(); await expect(page.getByText('原 IM 外发尚未核实；仅查询历史，不自动重放。', { exact: true })).toBeVisible(); await expect(button).toBeDisabled(); const reads = historyReads; await expect.poll(() => historyReads).toBeGreaterThan(reads); await expect(button).toBeDisabled(); expect(writes).toBe(2)
  await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true); await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-send-mobile.png', fullPage: true, animations: 'disabled' })
  await page.getByRole('button', { name: '切换原 IM 外发账号' }).click(); await expect(page.getByRole('textbox', { name: '原 IM 收件人（UID / GroupID / 原 URI）' })).toHaveValue(''); await expect(button).toBeDisabled(); await expect(page.getByText('原 IM 外发尚未核实；仅查询历史，不自动重放。', { exact: true })).toHaveCount(0); expect(writes).toBe(2)
})
test('original IM send exact no-intent failure rechecks whole history but admitted timeout cannot clear unknown from empty history', async ({ page }) => {
  let writes = 0; let reads = 0; let lastNonce = ''; let history: object[] = []
  page.on('dialog', (dialog) => { void dialog.accept() })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); const url = new URL(request.url()); expect(url.pathname.endsWith('/im-outbox')).toBe(true)
    if (request.method() === 'GET') { reads += 1; expect(url.searchParams.get('limit')).toBe('128'); await route.fulfill({ json: { status: 'ok', data: history } }); return }
    expect(request.method()).toBe('POST'); writes += 1; const data = request.postDataJSON(); expect(data.consent).toBe(true); expect(data.write_consent).toBe(true)
    expect(data.nonce).toMatch(/^[0-9a-f]{32}$/); expect(data.nonce).not.toBe(lastNonce); lastNonce = data.nonce
    if (writes === 1) { await route.fulfill({ status: 429, json: { status: 'error', message: 'app_account_im_send_capacity', im_send_admission: 'not_admitted', nonce_tag: tag(data.nonce) } }); return }
    if (writes === 2) { const row = { id: 1, nonce_tag: tag(data.nonce), kind: 'text', state: 'confirmed', message_index: '4294967297', original_time: '-1' }; history = [row]; await route.fulfill({ json: { status: 'ok', data: row } }); return }
    history = []; await route.fulfill({ status: 503, json: { status: 'error', message: 'app_account_im_send_timeout', im_send_admission: 'admitted_or_unknown', nonce_tag: tag(data.nonce) } })
  })
  await page.setViewportSize({ width: 1280, height: 1000 }); await page.goto('/e2e/fixtures/im-send.html')
  await page.getByRole('textbox', { name: '原 IM 收件人（UID / GroupID / 原 URI）' }).fill('1_2'); await page.getByRole('textbox', { name: '原 IM 内容' }).fill('explicit original IM body')
  const button = page.getByRole('button', { name: '明确执行原 IM 外发', exact: true }); await expect(button).toBeEnabled(); await button.click()
  await expect(page.getByText('原外发历史未核实，不生成新的业务意图。', { exact: true })).toBeVisible(); await expect(button).toBeDisabled()
  await expect(page.getByText('原 IM 外发尚未核实；仅查询历史，不自动重放。', { exact: true })).toHaveCount(0); expect(writes).toBe(1)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-send-admission-desktop.png', fullPage: true, animations: 'disabled' })
  const before = reads; await expect.poll(() => reads).toBeGreaterThan(before); await expect(button).toBeEnabled(); expect(writes).toBe(1)
  await button.click(); await expect(page.getByText(/原消息 ID 4294967297/)).toBeVisible(); await expect(button).toBeEnabled(); expect(writes).toBe(2)
  await button.click(); await expect(page.getByText('原 IM 外发尚未核实；仅查询历史，不自动重放。', { exact: true })).toBeVisible(); await expect(button).toBeDisabled()
  const readAfter = reads; await expect.poll(() => reads).toBeGreaterThan(readAfter); await expect(button).toBeDisabled(); expect(writes).toBe(3)
  await page.setViewportSize({ width: 390, height: 844 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-send-admission-mobile.png', fullPage: true, animations: 'disabled' })
})
test('original notification metadata editor keeps digest separate from body and no metadata change can replay an unknown business send', async ({ page }) => {
  let writes = 0; let history: object[] = []
  page.on('dialog', (dialog) => { void dialog.accept() })
  await page.route((url) => url.pathname.startsWith('/api/'), async (route) => {
    const request = route.request(); expect(new URL(request.url()).pathname.endsWith('/im-outbox')).toBe(true)
    if (request.method() === 'GET') { expect(new URL(request.url()).searchParams.get('limit')).toBe('128'); await route.fulfill({ json: { status: 'ok', data: history } }); return }
    writes += 1; const data = request.postDataJSON(); const bytes = (text: string) => Array.from(new TextEncoder().encode(text))
    expect(data).toEqual({ expected_revision: 1, nonce: expect.stringMatching(/^[0-9a-f]{32}$/), recipient: '1_2', content: 'actual body', notification_digest: 'display summary', parameters: [{ key: bytes('userData'), value: bytes('opaque-user-data') }, { key: bytes('OnlineMsg'), value: bytes('01') }, { key: bytes('imdnId'), value: bytes('opaque-imdn') }], consent: true, write_consent: true })
    if (writes === 1) { const row = { id: 1, nonce_tag: tag(data.nonce), kind: 'text', state: 'confirmed', message_index: '4294967297', original_time: '-1' }; history = [row]; await route.fulfill({ json: { status: 'ok', data: { ...row, private_notification: 'never-render-raw' } } }); return }
    await route.abort()
  })
  await page.setViewportSize({ width: 1280, height: 1100 }); await page.goto('/e2e/fixtures/im-send.html')
  await page.getByRole('textbox', { name: '原 IM 收件人（UID / GroupID / 原 URI）' }).fill('1_2'); await page.getByRole('textbox', { name: '原 IM 内容' }).fill('actual body')
  await page.getByRole('button', { name: '原通知元数据（可选）', exact: true }).click()
  await page.getByRole('textbox', { name: 'Text通知摘要（原MtcImDigestKey DATA）' }).fill('display summary')
  await page.getByRole('textbox', { name: '原userData字符串' }).fill('opaque-user-data'); await page.getByRole('textbox', { name: '原OnlineMsg字符串（不代表在线状态）' }).fill('01'); await page.getByRole('textbox', { name: '原imdnId字符串（不是回执授信）' }).fill('opaque-imdn')
  const button = page.getByRole('button', { name: '明确执行原 IM 外发', exact: true }); await expect(button).toBeEnabled(); await button.click(); await expect(page.getByText(/原消息 ID 4294967297/)).toBeVisible(); await expect(page.getByText('never-render-raw', { exact: false })).toHaveCount(0)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-send-options-desktop.png', fullPage: true, animations: 'disabled' })
  await button.click(); await expect(page.getByText('原 IM 外发尚未核实；仅查询历史，不自动重放。', { exact: true })).toBeVisible(); await expect(button).toBeDisabled(); expect(writes).toBe(2)
  await page.getByRole('textbox', { name: 'Text通知摘要（原MtcImDigestKey DATA）' }).fill('changed-summary'); await expect(button).toBeDisabled(); expect(writes).toBe(2)
  await page.setViewportSize({ width: 390, height: 1000 }); expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1)).toBe(true)
  await page.screenshot({ path: 'test-results/jegotrip-device-ui/im-send-options-mobile.png', fullPage: true, animations: 'disabled' })
})
