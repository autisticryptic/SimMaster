import assert from 'node:assert/strict'
import test from 'node:test'
import { airplaneControlLabel } from '../src/utils/lineControlLabels.ts'
import { esimProfileActive, esimSwitchBlockReason, switchEsimProfile, type EsimQuickSwitchApi } from '../src/utils/esimQuickSwitch.ts'
import type { EsimProfile } from '../src/api/contracts.ts'

const profile = (iccid: string, state: string): EsimProfile => ({ iccid, state, name: 'fixture', provider: 'test', class: 'operational', raw: null })
const oldProfile = profile('8900000000000000001', 'enabled')
const targetProfile = profile('8900000000000000002', 'disabled')

function switchFixture() {
  let enables = 0
  let reads = 0
  let progressReads = 0
  const api: EsimQuickSwitchApi = {
    profiles: (line) => {
      assert.equal(line, 'line-test')
      reads++
      return Promise.resolve(reads === 1 ? [oldProfile, targetProfile] : [profile(oldProfile.iccid, 'disabled'), profile(targetProfile.iccid, 'enabled')])
    },
    progress: () => {
      progressReads++
      return Promise.resolve({ running: progressReads === 2, steps: [] })
    },
    enable: (line, iccid) => {
      assert.equal(line, 'line-test'); assert.equal(iccid, targetProfile.iccid)
      enables++
      return Promise.resolve({ code: 0, status: 'success', action: 'enable', msg: 'task accepted' })
    },
    delay: () => Promise.resolve(),
  }
  return { api, counts: () => ({ enables, reads, progressReads }) }
}

void test('direct eSIM switch posts once and waits for authoritative active-profile readback', async () => {
  const fixture = switchFixture()
  const profiles = await switchEsimProfile(fixture.api, 'line-test', targetProfile.iccid, () => true, { maxPolls: 3 })
  assert.equal(fixture.counts().enables, 1)
  assert.equal(fixture.counts().progressReads, 3)
  assert.equal(fixture.counts().reads, 2)
  assert.equal(profiles.find((p) => p.iccid === targetProfile.iccid)?.state, 'enabled')
  assert.equal(targetProfile.state, 'disabled', 'do not optimistically mutate the initial list')
})

void test('active, protected and unknown eSIM profiles are not switchable', () => {
  assert.equal(esimProfileActive(profile('test', '1')), true)
  assert.equal(esimSwitchBlockReason(oldProfile, [oldProfile, targetProfile]), '已启用')
  assert.equal(esimSwitchBlockReason(targetProfile, [{ ...oldProfile, disable_allowed: false }, targetProfile]), '当前配置不允许停用')
  assert.equal(esimSwitchBlockReason(profile('test', 'unknown'), []), '状态不可切换')
})

void test('line change during eSIM preflight prevents enable request', async () => {
  const fixture = switchFixture()
  let current = true
  fixture.api.profiles = () => { current = false; return Promise.resolve([oldProfile, targetProfile]) }
  await assert.rejects(switchEsimProfile(fixture.api, 'line-test', targetProfile.iccid, () => current), /线路已变化/)
  assert.equal(fixture.counts().enables, 0)
})

void test('line change after submitting does not publish the old line result or repeat enable', async () => {
  const fixture = switchFixture()
  const enable = fixture.api.enable
  let current = true
  fixture.api.enable = (line, iccid) => { current = false; return enable(line, iccid) }
  await assert.rejects(switchEsimProfile(fixture.api, 'line-test', targetProfile.iccid, () => current), /线路已变化/)
  assert.equal(fixture.counts().enables, 1)
  assert.equal(fixture.counts().reads, 1)
})

void test('eSIM recovery failure is reported rather than claiming the profile was enabled', async () => {
  const fixture = switchFixture()
  let reads = 0
  fixture.api.progress = () => Promise.resolve(++reads === 1 ? { running: false, steps: [] } : { running: false, steps: [{ step: '切换', status: 'error', detail: 'policy denied' }] })
  await assert.rejects(switchEsimProfile(fixture.api, 'line-test', targetProfile.iccid, () => true), /policy denied/)
  assert.equal(fixture.counts().enables, 1)
})

void test('accepted but unconfirmed eSIM switch expires without resubmission', async () => {
  const fixture = switchFixture()
  fixture.api.profiles = () => Promise.resolve([oldProfile, targetProfile])
  await assert.rejects(switchEsimProfile(fixture.api, 'line-test', targetProfile.iccid, () => true, { maxPolls: 2 }), /尚未确认/)
  assert.equal(fixture.counts().enables, 1)
})

void test('busy eSIM maintenance and missing target stop before mutation', async () => {
  const fixture = switchFixture()
  fixture.api.progress = () => Promise.resolve({ running: true, steps: [] })
  await assert.rejects(switchEsimProfile(fixture.api, 'line-test', targetProfile.iccid, () => true), /正在切换或恢复/)
  assert.equal(fixture.counts().enables, 0)
  fixture.api.profiles = () => Promise.resolve([])
  await assert.rejects(switchEsimProfile(fixture.api, 'line-test', targetProfile.iccid, () => true), /未找到/)
  assert.equal(fixture.counts().enables, 0)
})

void test('flight-mode row stays compact and never reports unknown RF as off', () => {
  assert.equal(airplaneControlLabel(undefined, true, false, false), '状态未知')
  assert.equal(airplaneControlLabel(undefined, false, false, false), '设备离线')
  assert.equal(airplaneControlLabel(undefined, true, true, false), '切换中')
  const state = { airplane_mode_requested: true, airplane_mode_observed: null }
  assert.equal(airplaneControlLabel(state, true, false, false), '状态未知')
  assert.equal(airplaneControlLabel({ ...state, airplane_mode_observed: false }, true, false, false), '待生效')
  assert.equal(airplaneControlLabel({ ...state, airplane_mode_observed: true }, true, false, false), '已开启')
  assert.equal(airplaneControlLabel({ ...state, airplane_error: 'long backend diagnostic' }, true, false, false), '切换失败')
})
