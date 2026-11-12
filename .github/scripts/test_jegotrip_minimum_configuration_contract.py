"""Static contracts for minimum-input guidance and private editor lifecycle.
These inspect source only; browsers/Rust/provider operations run nowhere here.
"""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'


def source(path):
    return (ROOT / path).read_text(encoding='utf-8')


class JegotripMinimumConfigurationContracts(unittest.TestCase):
    def test_configuration_guide_distinguishes_identification_and_authorization(self):
        doc = source('docs/JEGOTRIP_MINIMUM_CONFIGURATION.md')
        for term in ['phone_uuid', 'device.dat', 'authentication_code', 'enrollId', 'sso_token',
                     'App 会话', '电话 token', 'SIP 密码', 'ARC 返回 token', 'SDK `DeviceId` / `ClientId`']:
            self.assertIn(term, doc)
        for boundary in ['不是物理 IMEI', '没有证据支持', '原生代码、反射调用或服务器其他策略',
                         '不是一份可直接导入的完整部署模板', '正常 Android SDK getter']:
            self.assertIn(boundary, doc)
        self.assertIn('UUID 是原 App 授信/业务通路必须保持一致的材料', doc)
        self.assertIn('不证明服务器对所有接口/账号只检查 UUID', doc)
        self.assertIn('手机真实授信后', doc)
        self.assertIn('263527个 Java code item', doc)
        self.assertIn('未发现对这两个入口的 Java 调用', doc)

    def test_user_guidance_does_not_replace_native_configuration_with_app_uuid(self):
        ui = source('frontend/src/pages/sim/JegotripConfigurationDialog.tsx')
        for phrase in ['常规使用的最小输入', '不需要手机硬件 IMEI、型号或模拟器',
                       '不是一串能替代所有配置的永久代码', '不能混用', '不会被 App UUID 擅自替换',
                       '接收器与媒体仍须真实核验']:
            self.assertIn(phrase, ui)
        bind = (P / 'app_login/config.rs').read_text(encoding='utf-8').split('pub fn bind(', 1)[1].split('#[derive(Serialize)]', 1)[0]
        self.assertIn('device_id:self.sdk.device_id', bind)
        self.assertNotIn('device_id:self.http.device_id', bind)
        self.assertNotIn('hardware_imei', ui)

    def test_private_editor_is_opt_in_and_save_still_requires_an_initialized_revision(self):
        ui = source('frontend/src/pages/sim/JegotripConfigurationDialog.tsx')
        self.assertIn('const [advanced, setAdvanced] = useState(false)', ui)
        self.assertIn('setAdvanced(false)', ui)
        self.assertIn('{advanced && <Stack id="jegotrip-private-installation"', ui)
        self.assertIn('busy || !advanced || !view?.initialized || !text.trim()', ui)
        self.assertIn('api.saveJegotripConfiguration(view.revision, configuration)', ui)
        self.assertIn('aria-expanded={advanced}', ui)
        self.assertIn("setText(''); setError(null); setAdvanced(!advanced)", ui)

    def test_stale_file_reads_are_revoked_by_typing_hiding_saving_and_clearing(self):
        ui = source('frontend/src/pages/sim/JegotripConfigurationDialog.tsx')
        self.assertIn('const fileGeneration = useRef(0)', ui)
        self.assertEqual(ui.count('scope.active && selected === fileGeneration.current'), 2)
        for start, end in [('const save = async', 'const clear = async'), ('const clear = async', 'return <Dialog')]:
            body = ui.split(start, 1)[1].split(end, 1)[0]
            self.assertIn('fileGeneration.current += 1', body)
        self.assertIn("fileGeneration.current += 1; setText(''); setError(null); setAdvanced(!advanced)", ui)
        self.assertIn('fileGeneration.current += 1; setText(event.target.value)', ui)
        self.assertIn('file.size > 256 * 1024', ui)
        self.assertIn('new TextEncoder().encode(text).length > 256 * 1024', ui)

    def test_editor_preserves_secret_cleanup_and_never_persists_browser_data(self):
        ui = source('frontend/src/pages/sim/JegotripConfigurationDialog.tsx')
        for forbidden in ['localStorage', 'sessionStorage', 'indexedDB', 'console.log', 'fetch(', 'expose_to_backend']:
            self.assertNotIn(forbidden, ui)
        self.assertIn('scope.active = false', ui)
        self.assertIn("setBusy(false); setText('')", ui)
        self.assertIn('旧秘密值不会回显', ui)
        self.assertIn('数据库包含私有参数，不是加密保险箱', ui)
        self.assertIn('window.confirm', ui)

    def test_browser_cases_preserve_existing_checks_and_cover_deterministic_file_races(self):
        tests = source('frontend/e2e/jegotrip-device-identity.spec.ts')
        fixture = source('frontend/e2e/fixtures/jegotrip-device.tsx')
        self.assertIn('JegotripConfigurationDialog', fixture)
        self.assertEqual(tests.count("test('"), 4)
        for title in ['device UI persists the automatic identity and supports a manual CAS update after reopening',
                      'mobile device UI validates a manual value and never claims verification or hosting success',
                      'desktop phone backup UI parses original files locally and preserves explicit identity CAS and consent',
                      'mobile phone backup UI rejects protected metadata and cancels stale reads without uploading credentials']:
            self.assertIn("test('" + title + "'", tests)
        for marker in ['checkInstallationGuide', 'NEWER-TYPED-INPUT', 'STALE-FILE-INPUT', 'STALE-HIDDEN-INPUT',
                       "await expect(editor).not.toBeAttached()", 'expected_revision: 3',
                       'Buffer.alloc(256 * 1024 + 1, 32)', 'expect(model.otherWrites).toEqual([])',
                       'model.writes[1].expected_revision', 'expect(model.logins).toEqual([])']:
            self.assertIn(marker, tests)
        self.assertNotIn('waitForTimeout(', tests)
        self.assertIn("url.pathname.startsWith('/api/')", tests)

    def test_guidance_is_linked_and_keeps_the_real_receiver_acceptance_gap_visible(self):
        guide = source('docs/JEGOTRIP_MINIMUM_CONFIGURATION.md')
        delivery = source('docs/JEGOTRIP_NATIVE_HOSTING_DELIVERY.md')
        self.assertIn('(JEGOTRIP_MINIMUM_CONFIGURATION.md)', delivery)
        self.assertIn("'JEGOTRIP_MINIMUM_CONFIGURATION.md'", source('.github/scripts/test_documentation_layout.py'))
        for item in ['旧任务36', '旧任务16', '旧任务17', 'moreOfflineMessages', 'stop/Join',
                     '界面简化或已有1242项隔离回归都不能代替']:
            self.assertIn(item, guide)
    def test_phone_transfer_input_is_not_a_trust_boolean_or_sdk_identity_override(self):
        policy = source('frontend/src/policies/appLogin.ts')
        pane = source('frontend/src/pages/sim/AppAccountLoginPane.tsx')
        devices = source('frontend/src/pages/sim/JegotripDeviceIdentityPane.tsx')
        self.assertIn('validPhoneAppUuid', policy)
        self.assertIn("deviceMode === 'phone_app'", policy)
        self.assertIn('复用手机 App 已授信 UUID', pane)
        self.assertIn('录入手机 App 授信 UUID', devices)
        request = (P / 'app_login/mod.rs').read_text(encoding='utf-8').split('pub struct JegotripLoginRequest {', 1)[1].split('\n}', 1)[0]
        for forbidden in ['trust_granted', 'device_trust:', 'sso_token', 'hardware_imei', 'sdk_device_id']:
            self.assertNotIn(forbidden, request)

    def test_trust_check_is_explicit_no_native_no_enrollment_and_bound_to_the_original_identity(self):
        backend = (P / 'app_login/mod.rs').read_text(encoding='utf-8')
        self.assertIn('Self::EnableHosting|Self::AttachHosting', backend)
        self.assertIn('LoginIntent::Authenticate|LoginIntent::CheckDeviceTrust', backend)
        self.assertIn('intent.needs_native() && configuration.runtime.is_none()', backend)
        self.assertIn('if intent!=LoginIntent::CheckDeviceTrust {client.ensure_device_enrollment().await;}', backend)
        check = backend.split('if intent==LoginIntent::CheckDeviceTrust {', 1)[1].split('if self.database.with_conn', 1)[0]
        self.assertIn('DeviceTrustObservation::capture(&guard.snapshot', check)
        for forbidden in ['accounts.create', 'client.phone_credentials', 'client.enroll_device', 'client.enable_business', 'accounts.start']:
            self.assertNotIn(forbidden, check)
        self.assertLess(backend.index('self.confirm_device_identity(guard,&key,intent.reuses_trusted_device())'), backend.index('if intent==LoginIntent::CheckDeviceTrust {'))
        self.assertIn('device_identity::confirm_reuse(&tx,&guard.alias,&guard.snapshot,key,now())', backend)
        self.assertIn('if !intent.reuses_trusted_device() {', backend)

    def test_trust_observations_are_bounded_metadata_not_a_cached_authorization(self):
        model = (P / 'app_login/device_trust.rs').read_text(encoding='utf-8')
        store = (P / 'app_login/store.rs').read_text(encoding='utf-8')
        self.assertIn('MAX_OBSERVATION_BYTES: usize = 2048', model)
        self.assertIn('device_identity_revision', model)
        self.assertIn('stored_code(error.code)', model)
        self.assertIn('#[serde(deny_unknown_fields)]', model)
        self.assertIn("length(CAST(device_trust AS BLOB)) BETWEEN 1 AND 2048", store)
        self.assertIn('WHERE attempt_id=?3 AND owner=?4 AND completed=0', store)
        self.assertNotIn('pub token:', model)
        self.assertNotIn('pub device_id:', model)
        self.assertNotIn('pub trusted:bool', model)
        backend = (P / 'app_login/mod.rs').read_text(encoding='utf-8')
        self.assertNotIn('if observation.', backend)
        http = (P / 'app_login/http.rs').read_text(encoding='utf-8')
        self.assertIn('if number.to_string()!=text {return None;}', http)
        self.assertIn('noncanonical trust value must remain unknown', (P / 'app_login/tests.rs').read_text(encoding='utf-8'))

    def test_trust_api_and_browser_regressions_preserve_no_store_and_no_business_assertions(self):
        http = source('backend/src/api/app_voice_http_tests.rs')
        self.assertIn('device_trust_check_api_requires_auth_consent_and_no_receiver_without_a_native_template_or_secret_export', http)
        part = http.split('async fn device_trust_check_api_', 1)[1].split('async fn credential_metadata_', 1)[0]
        self.assertIn('StatusCode::UNAUTHORIZED', part)
        self.assertIn('no_store(&headers)', part)
        self.assertIn('tokio::join!(response,refusal)', part)
        self.assertNotIn('tokio::spawn', part)
        browser = source('frontend/e2e/jegotrip-device-identity.spec.ts')
        self.assertIn('checkPhoneTrustTransfer', browser)
        self.assertIn("intent: 'check_device_trust', consent: true, start_receiver: false", browser)
        self.assertIn("expect(model.logins).toEqual([])", browser)
        self.assertIn(".not.toBeChecked(); await expect(receiver).toBeDisabled()", browser)


if __name__ == '__main__':
    unittest.main()
