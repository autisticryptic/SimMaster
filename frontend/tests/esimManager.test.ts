import assert from 'node:assert/strict'
import test from 'node:test'
import { readFileSync } from 'node:fs'
import { createEsimManager, type EsimManagerApi } from '../src/utils/esimManagerStore.ts'
import { CONFIRM_DELETE_PROFILE, euiccManufacturer, extractDefaultSmdp, maskedEid, parseLpaCode, profileDeleteBlockReason, redactEid, remainingStorage, validateDownload } from '../src/utils/esimPresentation.ts'
import type { EsimEuiccInfo, EsimProfile } from '../src/api/contracts.ts'

const eid = '89033023000000000000000000000123'
const euicc: EsimEuiccInfo = { eid, status: 'ready', manufacturer: 'Thales', raw: null, memory_available_kb: 192 }
const profile = (iccid: string, state = 'disabled'): EsimProfile => ({ iccid, state, name: `Profile ${iccid}`, provider: 'test', class: 'operational', raw: null })
const ok = { code: 0, status: 'success', action: 'test', msg: 'ok' }
function fixture(lineId = 'line-a') {
  let profiles = [profile('1', 'enabled'), profile('2')]
  const calls: string[] = []
  const scoped = (line: string, call: string) => { assert.equal(line, lineId); calls.push(call) }
  const api: EsimManagerApi = {
    status: () => { calls.push('status'); return Promise.resolve({ installed: true, usable: true, path: 'lpac', arch: 'test', glibc_version: '', asset_name: '', message: '' }) },
    euicc: (line) => { scoped(line, 'euicc'); return Promise.resolve(euicc) },
    profiles: (line) => { scoped(line, 'profiles'); return Promise.resolve(profiles) },
    progress: (line) => { scoped(line, 'progress'); return Promise.resolve({ running: false, steps: [] }) },
    enable: (line, iccid) => { scoped(line, 'enable'); profiles = profiles.map((item) => ({ ...item, state: item.iccid === iccid ? 'enabled' : 'disabled' })); return Promise.resolve(ok) },
    rename: (line, iccid, name) => { scoped(line, 'rename'); profiles = profiles.map((item) => item.iccid === iccid ? { ...item, name } : item); return Promise.resolve(ok) },
    delete: (line, iccid) => { scoped(line, 'delete'); profiles = profiles.filter((item) => item.iccid !== iccid); return Promise.resolve(ok) },
    download: (line) => { scoped(line, 'download'); return Promise.resolve(ok) },
    delay: () => Promise.resolve(),
  }
  const manager = createEsimManager(lineId, api)
  return { manager, api, calls, setProfiles: (next: EsimProfile[]) => { profiles = next } }
}

void test('shared line store serializes reads and reuses warm snapshots without global profile cache', async () => {
  const { manager, calls } = fixture()
  const first = manager.load()
  assert.equal(manager.load(), first)
  await first
  assert.deepEqual(calls, ['status', 'euicc', 'profiles'])
  await manager.load()
  assert.equal(calls.length, 3)
  await manager.load(true)
  assert.equal(calls.length, 6)
})

void test('manufacturer and remaining storage are truthful, never modem identifiers or EID prefixes', () => {
  assert.equal(euiccManufacturer(euicc), 'Thales')
  assert.equal(euiccManufacturer({ ...euicc, manufacturer: '' }), '未知 eUICC 厂商')
  assert.equal(euiccManufacturer({ ...euicc, manufacturer: 'Unknown EUM (Prefix: 89033023)' }), '未知 eUICC 厂商')
  assert.equal(euiccManufacturer(null), '未知 eUICC 厂商')
  assert.equal(remainingStorage(euicc), '192 KB', 'available capacity works without total')
  for (const available of [undefined, NaN, -1, Infinity]) assert.equal(remainingStorage({ ...euicc, memory_available_kb: available }), '未读取')
  assert.equal(remainingStorage({ ...euicc, memory_available_kb: 0 }), '0 KB')
  assert.equal(remainingStorage({ ...euicc, memory_available_kb: undefined, memory_total_kb: 512 }), '未读取', 'a total is not available storage')
  assert.equal(remainingStorage(null), '未读取')
  assert.equal(redactEid(`error ${eid}`, eid), 'error EID 已隐藏')
})

void test('EID label preserves only the first six and last four digits of complete 32-digit values', () => {
  assert.equal(maskedEid(eid), '890330••••0123')
  assert.equal(maskedEid(' 12345678901234567890123456789012 '), '123456••••9012')
  for (const length of [...Array.from({ length: 31 }, (_, i) => i + 1), 33, 64]) {
    assert.equal(maskedEid('1'.repeat(length)), '••••', `invalid length ${length} must be fully masked`)
  }
  for (const invalid of ['not-an-eid', '123456••••9012', `${eid.slice(0, -1)}X`, `${eid.slice(0, 6)} ${eid.slice(6)}`, '１'.repeat(32)]) {
    assert.equal(maskedEid(invalid), '••••')
  }
  for (const missing of [undefined, null, '', '   ']) assert.equal(maskedEid(missing), '未读取')
  // Error/detail redaction is deliberately stricter than the summary label.
  assert.equal(redactEid(`error ${eid}`), 'error EID 已隐藏')
  assert.equal(redactEid('error malformed-id', 'malformed-id'), 'error EID 已隐藏')
})

void test('LPA/manual download validation retains confirmation code and IMEI fields', () => {
  assert.deepEqual(parseLpaCode(' LPA:1$smdp.io$matching-id$confirm '), { smdp: 'smdp.io', matching_id: 'matching-id', confirmation_code: 'confirm' })
  for (const invalid of ['not LPA', 'LPA:1$smdp.io$', 'LPA:1$smdp.io$id$cc$trailing']) assert.equal(parseLpaCode(invalid), null)
  assert.match(validateDownload({ smdp: ' ', matching_id: 'id' }) || '', /请填写/)
  assert.match(validateDownload({ smdp: 'smdp.io', matching_id: 'id', imei: 'abc' }) || '', /15/)
  assert.equal(validateDownload({ smdp: 'smdp.io', matching_id: 'id', imei: '123456789012345' }), null)
  assert.equal(extractDefaultSmdp({ EuiccConfiguredAddresses: { defaultDpAddress: ' dp.example ' } }), 'dp.example')
})

void test('delete requires exact confirmation and a fresh inactive unprotected profile', async () => {
  const f = fixture()
  await f.manager.load()
  assert.equal(await f.manager.delete('2', '确认'), false)
  assert.equal(f.calls.includes('delete'), false)
  assert.equal(await f.manager.delete('1', CONFIRM_DELETE_PROFILE), false)
  assert.match(f.manager.getSnapshot().error || '', /已启用/)
  f.setProfiles([profile('1'), { ...profile('2'), delete_allowed: false }])
  assert.equal(await f.manager.delete('2', CONFIRM_DELETE_PROFILE), false)
  assert.match(f.manager.getSnapshot().error || '', /策略/)
  f.setProfiles([profile('1'), profile('2')])
  assert.equal(await f.manager.delete('2', CONFIRM_DELETE_PROFILE), true)
  assert.deepEqual(f.manager.getSnapshot().profiles.map((item) => item.iccid), ['1'])
  assert.equal(profileDeleteBlockReason(profile('3', 'unknown')), '配置状态未知，请刷新后重试')
})

void test('rename trims input, rejects blank and honors lpac failure feedback', async () => {
  const f = fixture()
  await f.manager.load()
  assert.equal(await f.manager.rename('2', '   '), false)
  assert.equal(f.calls.includes('rename'), false)
  assert.equal(await f.manager.rename('2', '  renamed  '), true)
  assert.equal(f.manager.getSnapshot().profiles[1]?.name, 'renamed')
  f.api.rename = () => Promise.resolve({ ...ok, code: 1, msg: 'policy denied' })
  assert.equal(await f.manager.rename('2', 'wrong'), false)
  assert.match(f.manager.getSnapshot().error || '', /policy denied/)
  assert.equal(f.manager.getSnapshot().success, null)
})

void test('mutation lock survives unsubscribe/remount and blocks download/delete/rename/refresh during switching', async () => {
  const f = fixture()
  await f.manager.load()
  let finish!: (value: typeof ok) => void
  const originalEnable = f.api.enable
  f.api.enable = async (line, iccid) => { await new Promise<typeof ok>((resolve) => { finish = resolve }); return originalEnable(line, iccid) }
  const unsubscribe = f.manager.subscribe(() => {})
  const switching = f.manager.switch('2')
  while (!finish) await Promise.resolve()
  unsubscribe()
  assert.equal(f.manager.getSnapshot().operation, 'switch')
  const count = f.calls.length
  assert.equal(await f.manager.rename('2', 'other'), false)
  assert.equal(await f.manager.delete('2', CONFIRM_DELETE_PROFILE), false)
  assert.equal(await f.manager.download({ smdp: 'dp.io', matching_id: 'id' }), false)
  await f.manager.load(true)
  assert.equal(f.calls.length, count)
  finish(ok)
  assert.equal(await switching, true)
  assert.equal(f.calls.filter((call) => call === 'enable').length, 1)
  assert.equal(f.manager.getSnapshot().profiles.find((item) => item.iccid === '2')?.state, 'enabled')
})

void test('line A operations and cache never populate line B', async () => {
  const a = fixture('line-a')
  const b = fixture('line-b')
  await Promise.all([a.manager.load(), b.manager.load()])
  await a.manager.rename('2', 'only A')
  assert.equal(a.manager.getSnapshot().profiles[1]?.name, 'only A')
  assert.equal(b.manager.getSnapshot().profiles[1]?.name, 'Profile 2')
  assert.equal(b.calls.includes('rename'), false)
})

void test('failed or externally busy switches do not optimistically activate and require refresh', async () => {
  const f = fixture()
  await f.manager.load()
  f.api.enable = () => Promise.resolve({ ...ok, code: 1, msg: 'rejected' })
  assert.equal(await f.manager.switch('2'), false)
  assert.equal(f.manager.getSnapshot().profiles[1]?.state, 'disabled')
  assert.equal(f.manager.getSnapshot().needsRefresh, true)
  assert.equal(await f.manager.rename('2', 'blocked'), false)
  await f.manager.load(true)
  assert.equal(f.manager.getSnapshot().needsRefresh, false)
  f.api.progress = () => Promise.resolve({ running: true, steps: [] })
  assert.equal(await f.manager.delete('2', CONFIRM_DELETE_PROFILE), false)
  assert.equal(f.calls.includes('delete'), false)
})

void test('download trims all fields, translates refusal and never logs EID', async () => {
  const f = fixture()
  await f.manager.load()
  f.api.download = (line, form) => {
    assert.equal(line, 'line-a')
    assert.deepEqual(form, { smdp: 'dp.io', matching_id: 'id', confirmation_code: 'cc', imei: '123456789012345' })
    return Promise.resolve({ ...ok, code: 1, msg: 'MatchingID is refused' })
  }
  assert.equal(await f.manager.download({ smdp: ' dp.io ', matching_id: ' id ', confirmation_code: ' cc ', imei: '123456789012345' }), false)
  assert.match(f.manager.getSnapshot().error || '', /激活码已被使用或失效/)
  f.manager.reportError(`raw ${eid}`)
  assert.equal(f.manager.getSnapshot().error?.includes(eid), false)
})

void test('lost write responses block blind retries until authoritative refresh', async () => {
  for (const action of ['download', 'delete', 'rename'] as const) {
    const f = fixture()
    await f.manager.load()
    let writes = 0
    const fail = () => { writes++; return Promise.reject(new Error('connection lost after submit')) }
    f.api[action] = fail
    const submit = () => action === 'download'
      ? f.manager.download({ smdp: 'dp.io', matching_id: 'id' })
      : action === 'delete' ? f.manager.delete('2', CONFIRM_DELETE_PROFILE)
        : f.manager.rename('2', 'renamed')
    assert.equal(await submit(), false)
    assert.equal(f.manager.getSnapshot().needsRefresh, true)
    assert.equal(await submit(), false)
    assert.equal(writes, 1, `${action} must not be resubmitted blindly`)
    await f.manager.load(true)
    assert.equal(f.manager.getSnapshot().needsRefresh, false)
  }
})

void test('custom capacity mutation and its configuration wiring are removed', () => {
  const { manager } = fixture()
  assert.equal('saveCapacity' in manager, false)
  const store = readFileSync(new URL('../src/utils/esimManagerStore.ts', import.meta.url), 'utf8')
  const hook = readFileSync(new URL('../src/hooks/useEsimManager.ts', import.meta.url), 'utf8')
  const contracts = readFileSync(new URL('../src/api/contracts.ts', import.meta.url), 'utf8')
  assert.doesNotMatch(store, /saveCapacity|['"]capacity['"]|getConfig|setConfig|custom_memory_total/)
  assert.doesNotMatch(hook, /getEsimConfig|setEsimConfig/)
  assert.doesNotMatch(contracts, /custom_memory_total_kb|memory_total_customizable/)
})

void test('partial read failures lock mutations until a successful explicit refresh', async () => {
  const f = fixture()
  const original = f.api.profiles
  f.api.profiles = () => Promise.reject(new Error('read failed'))
  await f.manager.load()
  assert.equal(f.manager.getSnapshot().detected, true)
  assert.equal(f.manager.getSnapshot().needsRefresh, true)
  assert.equal(await f.manager.download({ smdp: 'dp.io', matching_id: 'id' }), false)
  f.api.profiles = original
  await f.manager.load(true)
  assert.equal(f.manager.getSnapshot().needsRefresh, false)
})

void test('workbench uses only inline shared manager, EID is masked with a separate copy icon and no capacity editor', () => {
  const workbench = readFileSync(new URL('../src/pages/SimCard.tsx', import.meta.url), 'utf8')
  const manager = readFileSync(new URL('../src/pages/EsimManager.tsx', import.meta.url), 'utf8')
  assert.doesNotMatch(workbench, /完整管理|managerOpen|getEsimProfiles|getEsimEuicc|switchEsimProfile/)
  assert.match(workbench, /<EsimProfileManager state=\{esimState\}/)
  assert.match(workbench, /euiccManufacturer\(euicc\)/)
  assert.match(manager, /export default function EsimManagerPage/)
  assert.match(manager, /repeat\(auto-fit, minmax\(min\(100%, 260px\), 1fr\)\)/)
  assert.doesNotMatch(manager, /euicc\.eid\.slice|title=\{.*eid|value=\{.*eid|showEid|Visibility/)
  assert.match(manager, /maskedEid\(euicc\?\.eid\)/)
  assert.match(manager, /<IconButton[^>]*aria-label="复制完整 EID"/)
  assert.match(manager, /copyPrivateText\(euicc.eid\)/)
  assert.doesNotMatch(manager, /capacityOpen|setCapacity|saveCapacity|自定义.*容量|memory_total/)
})
