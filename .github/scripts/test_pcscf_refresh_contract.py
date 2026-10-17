"""R6 source wiring; real provider/worker/REGISTER execution is Actions-only."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[2]
BASE=ROOT/'backend/src/connectivity/modems/ims/cellular_ims'

class PcscfRefreshContract(unittest.TestCase):
    def test_update_query_stays_on_retained_provider_without_at_fallback(self):
        text=(BASE/'native_bearer.rs').read_text(encoding='utf-8')
        block=text[text.index('pub async fn observe_pcscf_update('):text.index('pub(crate) fn retained_for_test(')]
        self.assertIn('self.handle.discover_pcscf().await',block)
        self.assertGreaterEqual(block.count('self.worker_binding_is_current()'),2)
        self.assertNotIn('exact_at_fallback',block)

    def test_reselection_moves_handles_without_reactivating_or_releasing_pdn(self):
        text=(BASE/'pcscf_refresh.rs').read_text(encoding='utf-8')
        self.assertIn('replacement.native_bearer = session.native_bearer.take()',text)
        self.assertIn('replacement.ims_profile_lease = session.ims_profile_lease.take()',text)
        self.assertIn('require_protection: session.channel.security_verify().is_some()',text)
        self.assertNotIn('establish_native_ims_bearer(',text)
        self.assertNotIn('cleanup_live_session(',text)
        self.assertIn('metadata.min_expires.is_none()',text)
        self.assertIn('session.selected_endpoint.source != PcscfSource::BearerPco || !session.voice_calls.is_empty()',text)
        self.assertIn('discovery.ipv6_complete',text)
        live=(BASE/'live.rs').read_text(encoding='utf-8')
        self.assertIn('pcscf_refresh::refresh_if_changed(session, runtime, line_id).await',live)
        self.assertIn('.with_required_security(inherited_protection ||',live)

    def test_fresh_pco_never_falls_through_to_another_source(self):
        text=(BASE/'pcscf.rs').read_text(encoding='utf-8')
        block=text[text.index('pub async fn rediscover_retained_pco_in_worker('):text.index('fn pcscf_srv_names(')]
        self.assertIn('select_source_with_discovery(None, settings, None, &[], domains, None)',block)
        self.assertIn('Selection::Explicit(PcscfSource::BearerPco, _)',block)
        self.assertNotIn('std::env',block)

    def test_actual_regressions_and_receipt_time_are_wired(self):
        for name in ['build-release.yml','beta-validation.yml']:
            text=(ROOT/'.github/workflows'/name).read_text(encoding='utf-8')
            self.assertIn('connectivity::modems::ims::cellular_ims::live::pcscf_refresh::tests',text)
            self.assertIn('connectivity::core::registration::tests',text)
        core=(ROOT/'backend/src/connectivity/core/register.rs').read_text(encoding='utf-8')
        self.assertIn('pub received_at: std::time::SystemTime',core)
        live=(BASE/'live.rs').read_text(encoding='utf-8')
        self.assertEqual(live.count('.with_received_at(registration.received_at)'),2)

if __name__=='__main__': unittest.main()
