import { test, expect, type Page } from '@playwright/test'
import type { ImsRegistrationDiagnostics } from '../src/api/contracts'

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
    runtime: { phase: 'degraded', stage: 'register_refresh', registered: false, registration_mode: 'tcp_ipsec',
      recovery_state: 'retrying', manual_retry_available: false, connection_attempts: [], profile_attempt_results: [],
      retry_attempt: 1, retry_max: 3, modem_restart_attempt: 0, modem_restart_max: 0, reconnect_count: 0,
      register_refresh_count: 0, sent_count: 0, received_count: 0, duplicate_count: 0, profile_id: `cell-profile-${id}`,
      identity_source: 'at_cimi_isim_detected', isim_aid: 'DO_NOT_RENDER_APPLICATION_ID',
      ...(includeDiagnostics ? { diagnostics: { ...facts(`cell-profile-${id}`, false), raw_sip: 'DO_NOT_RENDER_SIP', authorization: 'DO_NOT_RENDER_AUTH' } } : {}) },
  }))
  const wifi = lines.map((line) => ({ modem: line.modem, config: { enabled: true, runtime_mode: 'live', proxy_endpoint: null },
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
  return { mutations, errors }
}

test('execution facts render inside the original workbench without mutating saved controls or crossing lines', async ({ page }, testInfo) => {
  const { mutations, errors } = await installFixtures(page, true)
  await page.goto('/e2e/fixtures/lines.html')
  await expect(page.getByTestId('selected-line')).toHaveText('line-a')
  await expect(page.getByTestId('original-basic-info')).toBeVisible()
  await expect(page.getByRole('checkbox', { name: '飞行模式' })).toBeChecked()
  await expect(page.getByRole('tab')).toHaveCount(7)
  await page.getByRole('tab', { name: 'IMS 与 Trunk', exact: true }).click()
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
  await expect(page.locator('body')).not.toContainText('cell-profile-a')
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
  await expect(page.locator('body')).not.toContainText('DO_NOT_RENDER')
  expect(mutations).toEqual([])
  expect(errors).toEqual([])
})
