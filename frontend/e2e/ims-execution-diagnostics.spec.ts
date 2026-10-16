import { test, expect, type Page } from '@playwright/test'
import type { ImsRegistrationDiagnostics, NativeImsObservationStatus } from '../src/api/contracts'

function facts(profileId: string, wifi: boolean): ImsRegistrationDiagnostics {
  return {
    version: 1, observed: true, procedure: 'refresh', profile_id: profileId, profile_source: wifi ? 'carrier_catalog' : 'database',
    identity_source: wifi ? 'usim_derived' : 'isim', declared_identity_source: 'carrier_device_model', identity_rule_status: 'declared_only',
    domain_source: wifi ? 'derived' : 'isim', realm_source: 'sim_override', requested_family: 'ipv4v6',
    granted_family: wifi ? 'ipv4' : 'ipv6', signaling_family: wifi ? 'ipv4' : 'ipv6',
    pcscf_source: wifi ? 'access_configuration' : 'bearer_pco', transport: 'tcp', pcscf_port: 5060, signaling_port: 5098,
    endpoint_index: 1, endpoint_count: 2, socket_ready: true, tls_verification: 'not_applicable',
    offered_ipsec: [{ integrity: 'hmac_sha1_96', encryption: 'aes_cbc' }, { integrity: 'hmac_sha1_96', encryption: 'null' }],
    negotiated_ipsec: [{ integrity: 'hmac_sha1_96', encryption: 'null' }], negotiated_from_success: true,
    security_mode: 'ipsec', digest_algorithm: 'akav1_md5', rspauth_verified: false,
    failure: { scope: 'endpoint', reason_code: 'ims_register_authenticated_unexpected_status', sip_status: 503,
      auth_stage: 'authenticated', lease_negotiation_observed: false, procedure: 'refresh', endpoint_index: 1,
      response_retry_after_seconds: 300, response_retry_after_invalid: false },
    wait: { status: 'waiting', retry_after_seconds: 29, not_before: '2026-10-13T02:05:00Z' }, updated_at: '2026-10-13T02:00:00Z',
  }
}
async function installFixtures(page: Page, includeDiagnostics: boolean) {
  const mutations: string[] = [], errors: string[] = []
  page.on('pageerror', (error) => errors.push(error.message))
  const lines = ['a', 'b'].map((id, index) => ({
    modem: { line_id: `line-${id}`, line_kind: 'baseband', manufacturer: `Fixture ${id}`, model: 'mock-modem',
      slot_label: `测试槽${index + 1}`, slot_source: 'physdev', slot_stable: true, slot_conflict: false,
      display_order: index + 1, uim_slot: 1, present: true, sim_iccid: '', modem_path: '', qmi_device: null, state: 'registered' },
    profile: { enabled: true, cellular_ims_connection_enabled: true, data_connection_enabled: true,
      airplane_mode_enabled: true, roaming_allowed: true, vowifi: { enabled: true }, trunk: { enabled: false } },
    ...(includeDiagnostics && index === 0 ? { bearer_capabilities: { auth_none: 'supported', pap: 'supported', chap: 'unsupported', pap_or_chap: 'unknown',
      pco_request_policy: 'unknown', pco_pcscf_addresses: 'supported', pco_pcscf_domains: 'unsupported', dhcp_information: 'unknown',
      password: 'DO_NOT_RENDER_BEARER_CREDENTIAL' } } : {}),
    runtime: { phase: 'degraded', stage: 'register_refresh', registered: false, registration_mode: 'tcp_ipsec',
      recovery_state: 'retrying', manual_retry_available: false, connection_attempts: [], profile_attempt_results: [],
      retry_attempt: 1, retry_max: 3, modem_restart_attempt: 0, modem_restart_max: 0, reconnect_count: 0,
      register_refresh_count: 0, sent_count: 0, received_count: 0, duplicate_count: 0, profile_id: `cell-profile-${id}`,
      identity_source: 'at_cimi_isim_detected', isim_aid: 'DO_NOT_RENDER_APPLICATION_ID',
      ...(includeDiagnostics ? { diagnostics: { ...facts(`cell-profile-${id}`, false), raw_sip: 'DO_NOT_RENDER_SIP', authorization: 'DO_NOT_RENDER_AUTH' } } : {}) },
  }))
  const wifi = lines.map((line) => ({ line_id: line.modem.line_id, modem: line.modem,
    config: { enabled: true, proxy_mode: 'direct', proxy_endpoint: '',
      profile_selection: { attempts: [{ source: 'database', profile_id: 'saved-profile' }, { source: 'carrier_catalog', profile_id: null }, { source: 'derived', profile_id: null }] },
      auto_restore: { enabled: false } },
    runtime_phase: 'degraded', runtime_stage: 'ims_register', runtime_restore_in_progress: false,
    runtime_registered: false, runtime_readiness: { ike_ready: true, eap_ready: true, child_sa_ready: true, ims_registered: false },
    runtime_ims_transport_mode: 'tcp_ipsec', runtime_p_cscf: [], runtime_ike_algorithm_set: 'standard',
    runtime_ike_encryption: 'AES-CBC', runtime_ike_integrity: 'HMAC-SHA1-96', runtime_ike_prf: 'HMAC-SHA1',
    ...(includeDiagnostics ? { diagnostics: facts(`wifi-profile-${line.modem.line_id.at(-1)}`, true) } : {}) }))
  await page.route('**/*', async (route) => {
    const request = route.request(), url = new URL(request.url())
    if (url.hostname !== '127.0.0.1') return route.abort()
    if (!url.pathname.startsWith('/api/')) return route.continue()
    if (request.method() !== 'GET') {
      mutations.push(`${request.method()} ${url.pathname}`)
      return route.fulfill({ status: 403, json: { status: 'error', message: 'read-only mock' } })
    }
    if (url.pathname === '/api/cellular-ims/lines') return route.fulfill({ json: { status: 'ok', data: lines } })
    if (url.pathname === '/api/vowifi/lines') return route.fulfill({ json: { status: 'ok', data: wifi } })
    if (['/api/trunk/lines', '/api/modem/line-controls'].includes(url.pathname)) return route.fulfill({ json: { status: 'ok', data: [] } })
    return route.fulfill({ json: { status: 'ok', data: { events: [], messages: [], records: [] } } })
  })
  return { mutations, errors, wifi, lines }
}

function nativeObservation(lineId: string): NativeImsObservationStatus {
  return { line_id: lineId, adapter_available: true, state: 'observed',
    sample: { schema: 'ec20_ce_fag_v0803', registration_reporting: false, service_reporting: true,
      registration: 'registered', sms: 'unknown_or_unreported', voice: 'limited', ut: 'available' },
    observed_at: '2026-10-13T02:00:00Z', age_seconds: 5, stale: false, last_error: null,
    sample_kind: 'sequential_not_atomic', purpose: 'modem_observation_not_host_ims_readiness' }
}

async function installNativeFixtures(page: Page) {
  const fixture = await installFixtures(page, false)
  fixture.lines.forEach((line) => {
    line.modem.modem_path = `native:${line.modem.line_id}`
    line.profile.airplane_mode_enabled = false
    line.profile.cellular_ims_connection_enabled = false
    line.profile.vowifi.enabled = false
  })
  fixture.wifi.forEach((line) => { line.config.enabled = false })
  const nativeRequests: Array<{ method: string; path: string; body: unknown }> = []
  const observations: Record<string, NativeImsObservationStatus> = {
    'line-a': nativeObservation('line-a'), 'line-b': nativeObservation('line-b'),
  }
  page.on('request', (request) => {
    const url = new URL(request.url())
    if (url.pathname.includes('/native/ims/')) nativeRequests.push({ method: request.method(), path: url.pathname,
      body: request.method() === 'POST' ? request.postDataJSON() : null })
  })
  await page.route('**/api/modem/lines/*/native/ims/*', (route) => {
    const lineId = new URL(route.request().url()).pathname.split('/')[4]
    return route.fulfill({ json: { status: 'ok', data: observations[lineId] } })
  })
  return { ...fixture, nativeRequests, observations }
}

async function openNativeDetails(page: Page) {
  await page.getByRole('tab', { name: 'IMS 与 Trunk', exact: true }).click()
  const panel = page.getByTestId('native-ims-observation')
  await panel.locator('summary').click()
  return panel
}

// Uses the original workbench and its existing isolated fixture, never a live proxy.
test('native IMS observation is explicit, historical and independent of host readiness', async ({ page }, testInfo) => {
  const { nativeRequests, mutations, errors, observations } = await installNativeFixtures(page)
  await page.goto('/e2e/fixtures/lines.html')
  await expect(page.getByTestId('selected-line')).toHaveText('line-a')
  await expect(page.getByTestId('original-basic-info')).toBeVisible()
  await expect(page.getByRole('checkbox', { name: '飞行模式' })).not.toBeChecked()
  expect(nativeRequests).toEqual([])
  const panel = await openNativeDetails(page)
  await expect(panel).toContainText('并非原子快照')
  await expect(panel).toContainText('服务器仍会核验固件资格与实时 owner')
  expect(nativeRequests).toEqual([])
  const hostRegistration = page.getByText('注册状态', { exact: true }).locator('..')
  await expect(hostRegistration).toContainText('未注册')
  await panel.getByRole('button', { name: '只读采样一次', exact: true }).click()
  await expect(panel.getByText('已注册（仅基带报告）', { exact: true })).toBeVisible()
  await expect(hostRegistration).toContainText('未注册')
  await expect(panel.getByText('未知或未报告（0，不能据此判断不可用）', { exact: true })).toBeVisible()
  await expect(panel.getByText('最近样本 · UT（补充业务，非视频）', { exact: true })).toBeVisible()
  await expect(panel).toContainText('不证明持久 NV 读取成功')
  expect(nativeRequests).toEqual([{ method: 'POST', path: '/api/modem/lines/line-a/native/ims/probe', body: {} }])
  const sample = observations['line-a'].sample
  if (!sample) throw new Error('native fixture sample missing')
  observations['line-a'] = { ...observations['line-a'], stale: true, state: 'stale', sample: { ...sample, registration: 'unknown_or_unreported' } }
  await panel.getByRole('button', { name: '读取缓存（不访问设备）', exact: true }).click()
  await expect(panel.getByText('未知或未报告（0，不能据此判断未注册）', { exact: true })).toBeVisible()
  await expect(panel).toContainText('缓存已标记过期')
  await expect(hostRegistration).toContainText('未注册')
  expect(nativeRequests).toEqual([
    { method: 'POST', path: '/api/modem/lines/line-a/native/ims/probe', body: {} },
    { method: 'GET', path: '/api/modem/lines/line-a/native/ims/status', body: null },
  ])
  await page.screenshot({ path: testInfo.outputPath('original-workbench-native-ims-observation.png'), fullPage: true })
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})

for (const blocked of ['cellular intent', 'wifi intent', 'read-only projection', 'absent modem', 'airplane mode'] as const) {
  test(`native IMS explicit probe is disabled for ${blocked}`, async ({ page }) => {
    const { lines, wifi, nativeRequests, mutations, errors } = await installNativeFixtures(page)
    if (blocked === 'cellular intent') lines[0].profile.cellular_ims_connection_enabled = true
    if (blocked === 'wifi intent') { lines[0].profile.vowifi.enabled = true; wifi[0].config.enabled = true }
    if (blocked === 'read-only projection') Object.assign(lines[0], { read_only: true })
    if (blocked === 'absent modem') lines[0].modem.present = false
    if (blocked === 'airplane mode') lines[0].profile.airplane_mode_enabled = true
    await page.goto('/e2e/fixtures/lines.html')
    const panel = await openNativeDetails(page)
    await expect(panel.getByRole('button', { name: '只读采样一次', exact: true })).toBeDisabled()
    await expect(page.getByTestId('selected-line')).toHaveText('line-a')
    expect(nativeRequests).toEqual([])
    expect(mutations).toEqual([])
    expect(errors).toEqual([])
  })
}

test('native IMS ignores a late old-line probe response without auto-probing the new line', async ({ page }) => {
  const { nativeRequests, mutations, errors, observations } = await installNativeFixtures(page)
  const sample = observations['line-b'].sample
  if (!sample) throw new Error('native fixture sample missing')
  observations['line-b'] = { ...observations['line-b'], sample: { ...sample, registration: 'limited' } }
  let release!: () => void
  const held = new Promise<void>((resolve) => { release = resolve })
  await page.route('**/api/modem/lines/line-a/native/ims/probe', async (route) => {
    await held
    await route.fulfill({ json: { status: 'ok', data: nativeObservation('line-a') } })
  })
  await page.goto('/e2e/fixtures/lines.html')
  const oldPanel = await openNativeDetails(page)
  const sent = page.waitForRequest('**/api/modem/lines/line-a/native/ims/probe')
  await oldPanel.getByRole('button', { name: '只读采样一次', exact: true }).click()
  await sent
  await expect(oldPanel.getByRole('button', { name: '只读采样一次', exact: true })).toBeDisabled()
  await page.getByRole('button', { name: /测试槽2/ }).click()
  await expect(page.getByTestId('selected-line')).toHaveText('line-b')
  const panel = await openNativeDetails(page)
  await expect(panel).toContainText('尚未读取本线路的观测记录')
  expect(nativeRequests).toHaveLength(1)
  await panel.getByRole('button', { name: '读取缓存（不访问设备）', exact: true }).click()
  await expect(panel.getByText('受限（仅基带报告）', { exact: true })).toBeVisible()
  const response = page.waitForResponse('**/api/modem/lines/line-a/native/ims/probe')
  release()
  await (await response).finished()
  await page.evaluate(() => new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve()))))
  await expect(panel.getByText('受限（仅基带报告）', { exact: true })).toBeVisible()
  await expect(panel.getByText('已注册（仅基带报告）', { exact: true })).toHaveCount(0)
  expect(nativeRequests.filter(({ method }) => method === 'POST')).toEqual([
    { method: 'POST', path: '/api/modem/lines/line-a/native/ims/probe', body: {} },
  ])
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})

test('native IMS rejects mismatched line and schema responses and suppresses unknown errors', async ({ page }) => {
  const { observations, nativeRequests, mutations, errors } = await installNativeFixtures(page)
  await page.goto('/e2e/fixtures/lines.html')
  const panel = await openNativeDetails(page)
  const readCache = panel.getByRole('button', { name: '读取缓存（不访问设备）', exact: true })
  observations['line-a'] = nativeObservation('line-b')
  await readCache.click()
  await expect(panel.getByRole('alert')).toContainText('响应不符合约定')
  await expect(panel.getByText('已注册（仅基带报告）', { exact: true })).toHaveCount(0)
  const malformed = nativeObservation('line-a')
  if (!malformed.sample) throw new Error('native fixture sample missing')
  observations['line-a'] = { ...malformed, sample: { ...malformed.sample, schema: 'SECRET_SCHEMA' } } as unknown as NativeImsObservationStatus
  await readCache.click()
  await expect(panel.getByRole('alert')).toContainText('响应不符合约定')
  await expect(panel.getByText('已注册（仅基带报告）', { exact: true })).toHaveCount(0)
  await expect(page.locator('body')).not.toContainText('SECRET_SCHEMA')
  await page.route('**/api/modem/lines/line-a/native/ims/status', (route) => route.fulfill({
    status: 502, json: { status: 'error', message: 'AT+SECRET_IDENTITY' },
  }))
  await readCache.click()
  await expect(panel.getByRole('alert')).toContainText('未识别错误')
  await expect(page.locator('body')).not.toContainText('SECRET')
  expect(nativeRequests.filter(({ method }) => method === 'POST')).toEqual([])
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})

test('execution facts render inside the original workbench without mutating saved controls or crossing lines', async ({ page }, testInfo) => {
  const { mutations, errors } = await installFixtures(page, true)
  await page.goto('/e2e/fixtures/lines.html')
  await expect(page.getByTestId('selected-line')).toHaveText('line-a')
  await expect(page.getByTestId('original-basic-info')).toBeVisible()
  await expect(page.getByRole('checkbox', { name: '飞行模式' })).toBeChecked()
  await expect(page.getByRole('tab')).toHaveCount(7)
  await page.getByRole('tab', { name: 'IMS 与 Trunk', exact: true }).click()
  const capabilities = page.getByTestId('ims-bearer-capabilities')
  await capabilities.locator('summary').click()
  await expect(capabilities).toContainText('适配器支持')
  await expect(capabilities).toContainText('适配器不支持')
  await expect(capabilities).toContainText('支持情况未知')
  await expect(capabilities).toContainText('不代表当前承载、P-CSCF 或 IMS 注册已就绪')
  const cellular = page.getByTestId('ims-diagnostics-cellular'), wifi = page.getByTestId('ims-diagnostics-wifi')
  await cellular.locator('summary').click()
  await wifi.locator('summary').click()
  await expect(cellular).toContainText('自定义数据库 · cell-profile-a')
  await expect(cellular).toContainText('ISIM 卡内身份')
  await expect(cellular).toContainText('IPv4v6 / IPv6 / IPv6')
  await expect(cellular).toContainText('SIP 503')
  await expect(cellular).toContainText('300 秒（实际门禁另列）')
  await expect(cellular).toContainText('等待 29 秒')
  await expect(cellular).toContainText('最近成功协商也不代表当前仍已注册')
  await expect(wifi).toContainText('只读运营商库 · wifi-profile-a')
  await expect(wifi).toContainText('USIM 标准派生')
  await expect(wifi).toContainText('IPv4v6 / IPv4 / IPv4')
  await expect(page.locator('body')).not.toContainText('DO_NOT_RENDER')
  await page.screenshot({ path: testInfo.outputPath('original-workbench-execution-facts.png'), fullPage: true })
  await page.getByRole('button', { name: /测试槽2/ }).click()
  await expect(page.getByTestId('selected-line')).toHaveText('line-b')
  await expect(page.getByTestId('original-basic-info')).toBeVisible()
  await expect(page.getByRole('checkbox', { name: '飞行模式' })).toBeChecked()
  await page.getByRole('tab', { name: 'IMS 与 Trunk', exact: true }).click()
  await page.getByTestId('ims-diagnostics-cellular').locator('summary').click()
  await expect(page.getByTestId('ims-diagnostics-cellular')).toContainText('cell-profile-b')
  await page.getByTestId('ims-bearer-capabilities').locator('summary').click()
  await expect(page.getByTestId('ims-bearer-capabilities').getByText('支持情况未知', { exact: true })).toHaveCount(8)
  await expect(page.getByTestId('ims-bearer-capabilities').getByText('适配器支持', { exact: true })).toHaveCount(0)
  await expect(page.locator('body')).not.toContainText('cell-profile-a')
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})

test('original WiFi editor preserves cellular bearer intent and saved profile slots without replaying redacted credentials', async ({ page }) => {
  const { mutations, errors, wifi } = await installFixtures(page, true)
  const policy = { authentication: { method: 'pap', username: null, password: null }, pcscf_via_pco: 'required', pcscf_via_dhcp: 'disabled' }
  const saved = { ims_common: {}, ims_cellular: { apn: 'ims', profile_id: 'saved-cellular', bearer: policy, spoof_imsi: false },
    ims_vowifi: { spoof_imsi: false, custom_imsi: null }, services: { call_waiting: true }, emergency: {} }
  const writes: Array<{ path: string; body: Record<string, unknown> }> = []
  await page.route('**/api/vowifi/carrier-profiles', (route) => route.fulfill({ json: { status: 'ok', data: [] } }))
  await page.route('**/api/ims/lines/line-a/override', async (route) => {
    if (route.request().method() === 'PATCH') writes.push({ path: 'override', body: route.request().postDataJSON() as Record<string, unknown> })
    await route.fulfill({ json: { status: 'ok', data: { binding: { kind: 'plain', iccid_last4: '0001' }, override_: saved } } })
  })
  await page.route('**/api/vowifi/lines/line-a', async (route) => {
    if (route.request().method() === 'POST') writes.push({ path: 'wifi', body: route.request().postDataJSON() as Record<string, unknown> })
    await route.fulfill({ json: { status: 'ok', data: wifi[0] } })
  })
  await page.goto('/e2e/fixtures/lines.html')
  await page.getByRole('tab', { name: 'IMS 与 Trunk', exact: true }).click()
  await page.getByTestId('vowifi-profile-config').click()
  const dialog = page.getByRole('dialog')
  await expect(dialog).toContainText('WiFi Calling 配置')
  await expect(dialog.getByRole('button', { name: '保存配置' })).toBeEnabled()
  expect(writes).toEqual([])
  await dialog.getByRole('button', { name: '保存配置' }).click()
  await expect(dialog).toHaveCount(0)
  expect(writes.map(({ path }) => path)).toEqual(['override', 'wifi'])
  expect(writes[0].body.ims_cellular).toEqual({ apn: 'ims', profile_id: 'saved-cellular', spoof_imsi: false })
  expect(writes[0].body.services).toEqual({ call_waiting: true })
  expect(JSON.stringify(writes)).not.toContain('bearer')
  expect(JSON.stringify(writes)).not.toContain('password')
  expect(writes[1].body.profile_selection).toEqual(wifi[0].config.profile_selection)
  expect(writes[1].body.enabled).toBe(true)
  expect(saved.ims_cellular.bearer).toEqual(policy)
  await expect(page.getByTestId('selected-line')).toHaveText('line-a')
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})

test('older API responses keep the workbench and do not claim detected ISIM was selected', async ({ page }) => {
  const { mutations, errors } = await installFixtures(page, false)
  await page.goto('/e2e/fixtures/lines.html')
  await page.getByRole('tab', { name: 'IMS 与 Trunk', exact: true }).click()
  await expect(page.getByText('4G/5G 详情', { exact: true })).toBeVisible()
  await expect(page.getByText('已检测到应用，尚未确认选用', { exact: true })).toBeVisible()
  await expect(page.getByTestId('ims-diagnostics-cellular')).toHaveCount(0)
  await expect(page.getByTestId('ims-diagnostics-wifi')).toHaveCount(0)
  await expect(page.getByTestId('native-ims-observation')).toHaveCount(0)
  await page.getByTestId('ims-bearer-capabilities').locator('summary').click()
  await expect(page.getByTestId('ims-bearer-capabilities')).toContainText('支持情况未知')
  await expect(page.locator('body')).not.toContainText('DO_NOT_RENDER')
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})
