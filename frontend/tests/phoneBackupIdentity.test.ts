import assert from 'node:assert/strict'
import test from 'node:test'
import { PHONE_BACKUP_MAX_BYTES, phoneBackupError, phoneUuidFromBackupText } from '../src/policies/phoneBackupIdentity.ts'
import { emptyAppLoginDraft, canSubmitJegotripLogin, jegotripLoginInput } from '../src/policies/appLogin.ts'

const uuid = 'D5C0199B-1644-4D33-8912-B8BD5AB49224'
const exportText = JSON.stringify({ version: 1, source: 'android_root_export', app_device_uuid: uuid })

void test('raw App UUID and identity-only export preserve the original UUID case', () => {
  assert.equal(phoneUuidFromBackupText(`\uFEFF${uuid}\n`, 'device.dat'), uuid)
  assert.equal(phoneUuidFromBackupText(exportText, 'phone-identity.json'), uuid)
  assert.equal(phoneUuidFromBackupText(exportText.replace(uuid, uuid.toLowerCase()), 'phone-identity.json'), uuid.toLowerCase())
})
void test('protected SBA1 and v1 metadata are rejected without treating encoded data as a token or UUID', () => {
  for (const text of ['SBA1\u0000\u0002opaque-private-data', 'v1:::opaque-private-data']) {
    assert.throws(() => phoneUuidFromBackupText(text, 'setting_power.xml'), /jegotrip_backup_protected_container/)
  }
  assert.match(phoneBackupError(new Error('jegotrip_backup_protected_container')), /不是明文 App 配置/)
})
void test('ZIP files require the offline local reader and are never converted into an HTTP upload', () => {
  assert.throws(() => phoneUuidFromBackupText('PK\u0003\u0004private-data', 'backup.zip'), /jegotrip_backup_archive_requires_local_reader/)
  assert.match(phoneBackupError(new Error('jegotrip_backup_archive_requires_local_reader')), /整个备份不会上传/)
})
void test('identity exports reject authorization flags, cached tokens, SDK identifiers and duplicate JSON fields', () => {
  for (const field of ['trust_granted', 'authentication_code', 'enrollId', 'sso_token', 'password', 'sdk_device_id']) {
    const value = JSON.stringify({ version: 1, source: 'android_root_export', app_device_uuid: uuid, [field]: 'PRIVATE-NOT-EXPORTED' })
    assert.throws(() => phoneUuidFromBackupText(value, 'identity.json'), /jegotrip_backup_export_invalid/)
  }
  for (const value of [exportText.replace('"version":1', '"version":true'), exportText.replace('android_root_export', 'phone_safe_num'),
    exportText.slice(0, -1) + ',"app_device_uuid":"' + uuid + '"}', '[]', '{}', '{']) {
    assert.throws(() => phoneUuidFromBackupText(value, 'identity.json'), /jegotrip_backup_export_invalid/)
  }
})
void test('import bounds are UTF-8 bytes and unsupported identity shapes cannot masquerade as a UUID', () => {
  assert.throws(() => phoneUuidFromBackupText('é'.repeat(PHONE_BACKUP_MAX_BYTES / 2 + 1), 'device.dat'), /jegotrip_backup_identity_file_too_large/)
  for (const value of ['123456789012345', '0123456789abcdef0123456789abcdef', 'SDK-DEVICE-01', 'PRIVATE-TOKEN', '', `${uuid}\u0000`]) {
    assert.throws(() => phoneUuidFromBackupText(value, 'device.dat'), /jegotrip_backup_uuid_invalid/)
  }
  assert.throws(() => phoneUuidFromBackupText(uuid, 'sdk-uuid.txt'), /jegotrip_backup_format_unsupported/)
})
void test('DTD and entity declarations are rejected before attempting browser XML parsing', () => {
  assert.throws(() => phoneUuidFromBackupText('<!DOCTYPE map [<!ENTITY value "PRIVATE">]><map/>', 'setting_power.xml'), /jegotrip_backup_xml_invalid/)
})
void test('local backup parsing cannot grant consent or construct a device-trust or credential field', () => {
  const draft = { ...emptyAppLoginDraft(), username: '7700900123', password: 'TEST-ONLY-PASSWORD', deviceMode: 'phone_app' as const,
    deviceId: phoneUuidFromBackupText(exportText, 'identity.json'), intent: 'reuse_hosting' as const }
  assert.equal(canSubmitJegotripLogin(draft), false)
  const input = jegotripLoginInput('0123456789abcdef0123456789abcdef', { ...draft, consent: true })
  assert.equal(input.intent, 'reuse_hosting')
  assert.equal(input.device_id, uuid)
  assert.deepEqual(Object.keys(input).sort(), ['attempt_id', 'consent', 'country_code', 'device_id', 'intent', 'password', 'start_receiver', 'username'])
})
void test('unknown parsing and IO errors never reflect source filenames, account data or tokens', () => {
  const result = phoneBackupError(new Error('private-password token=PRIVATE-TOKEN https://private.invalid phone-PRIVATE.zip'))
  for (const privateValue of ['private-password', 'PRIVATE-TOKEN', 'private.invalid', 'phone-PRIVATE.zip']) assert.equal(result.includes(privateValue), false)
})
