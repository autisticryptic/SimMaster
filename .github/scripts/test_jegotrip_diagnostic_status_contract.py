"""Static fences for diagnostic-only retention of the already-issued status.
Behavioral regressions run as Rust only in the two existing Actions workflows.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip/app_login'


class DiagnosticStatusContract(unittest.TestCase):
    def test_diagnostic_uses_one_existing_status_request_without_extra_side_effects(self):
        source = (P / 'mod.rs').read_text(encoding='utf-8')
        check = source.split('if intent==LoginIntent::CheckDeviceTrust {', 1)[1].split('if self.database.with_conn', 1)[0]
        self.assertEqual(re.findall(r'client\.(\w+)\(\)\.await', check),
                         ['security_verify_device', 'verify_trusted_device', 'call_status'])
        self.assertIn('let business=status.map(|s|s.hosting_enabled);', check)
        self.assertIn('DeviceTrustObservation::capture(&guard.snapshot,now(),security,existing,business)', check)
        self.assertIn('.with_diagnostic_status(status)', check)
        for forbidden in ['enroll_device', 'enable_business', 'phone_credentials', 'accounts.create', 'accounts.start', 'retry']:
            self.assertNotIn(forbidden, check)

    def test_status_enrichment_is_data_only_and_does_not_make_a_second_request(self):
        source = (P / 'device_trust.rs').read_text(encoding='utf-8')
        part = source.split('fn with_diagnostic_status(', 1)[1].split('fn diagnostic_status_valid(', 1)[0]
        self.assertIn('TrustQueryObservation::capture(status.map(', part)
        self.assertIn('self.local_cs_forward=status.ok()', part)
        for forbidden in ['.await', 'reqwest', 'client.', 'socket', 'device_sync=', 'std::fs']:
            self.assertNotIn(forbidden, part)
        self.assertIn('pub(super) const MAX_OBSERVATION_BYTES: usize = 2048;', source)
        self.assertIn('#[serde(deny_unknown_fields)]', source)

    def test_diagnostic_and_reuse_metadata_remain_separate_and_consistent(self):
        source = (P / 'device_trust.rs').read_text(encoding='utf-8')
        diagnostic = source.split('fn diagnostic_status_valid(', 1)[1].split('fn valid(&self)', 1)[0]
        self.assertIn('self.security_verify.state==TrustQueryState::NotRequested', diagnostic)
        self.assertIn('self.existing_device.state==TrustQueryState::NotRequested', diagnostic)
        self.assertIn('self.business_query_error==status.error', diagnostic)
        self.assertIn('self.business_query_server_code==status.server_code', diagnostic)
        self.assertIn('self.hosting_enabled.is_none() && self.local_cs_forward.is_none()', diagnostic)
        self.assertIn('None=>self.diagnostic_status_valid(status)', source)
        self.assertIn('Some(sync)=>self.security_verify.state==TrustQueryState::NotRequested', source)
        self.assertIn('&& self.existing_device.state==TrustQueryState::NotRequested && sync.valid()', source)

    def test_reuse_gate_does_not_consume_any_diagnostic_or_historical_status(self):
        reuse = (P / 'trusted_reuse.rs').read_text(encoding='utf-8')
        self.assertIn('if status.authentication_code!=Some(1)', reuse)
        self.assertIn('let initial=client.call_status().await;', reuse)
        self.assertIn('let initial=require_current_trust(initial?)?;', reuse)
        self.assertLess(reuse.index('require_current_trust(initial?)?'), reuse.index('client.phone_credentials(bound).await?'))
        for forbidden in ['with_diagnostic_status', 'store::get(', 'DeviceTrustObservation::decode', 'verify_trusted_device']:
            self.assertNotIn(forbidden, reuse)

    def test_existing_bounded_public_fields_are_used_without_new_credential_inputs(self):
        types = (ROOT / 'frontend/src/api/current.ts').read_text(encoding='utf-8')
        self.assertIn('call_status?: JegotripTrustQueryObservation | null', types)
        self.assertIn('local_cs_forward?: number | null', types)
        ui = (ROOT / 'frontend/src/pages/sim/AppAccountLoginPane.tsx').read_text(encoding='utf-8')
        self.assertIn('deviceTrustQueryLabel(attempt.device_trust.call_status)', ui)
        self.assertIn('不是 SDK 或媒体就绪证明', ui)
        request = (P / 'mod.rs').read_text(encoding='utf-8').split('pub struct JegotripLoginRequest {', 1)[1].split('\n}', 1)[0]
        for forbidden in ['call_status', 'trust_granted', 'authentication_code', 'sso_token', 'enrollId']:
            self.assertNotIn(forbidden, request)

    def test_twelve_actions_regressions_cover_no_replay_and_old_record_compatibility(self):
        tests = (P / 'diagnostic_status_tests.rs').read_text(encoding='utf-8')
        names = re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)', tests)
        self.assertEqual(len(names), 12)
        self.assertEqual(len(names), len(set(names)))
        self.assertNotIn('#[ignore]', tests)
        for required in ['stored_positive_diagnostic_cannot_replace_a_new_reuse_attempts_current_rejection',
                         'diagnostic_preserves_conflicting_current_status_without_promoting_existing_device_success',
                         'diagnostic_status_codec_preserves_legacy_records_without_backfilling_response_fields',
                         'diagnostic_status_storage_failure_cannot_create_a_native_account_or_repeat_the_query']:
            self.assertIn(required, names)
        self.assertIn('assert_eq!(f.factory.build_count(),0)', tests)
        self.assertIn('assert_eq!(device_rows(&f,"app_voice_runtime_leases"),0)', tests)
        self.assertIn('#[path = "diagnostic_status_tests.rs"]\nmod diagnostic_status;',
                      (P / 'tests.rs').read_text(encoding='utf-8'))


if __name__ == '__main__':
    unittest.main()
