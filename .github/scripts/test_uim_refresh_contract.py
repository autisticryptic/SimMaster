"""R9 QMI UIM refresh: static wiring only; framing/execution is Actions-only."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
WIFI = ROOT/'backend/src/connectivity/modems/ims/vowifi'
CELL = ROOT/'backend/src/connectivity/modems/ims/cellular_ims'

class UimRefreshContract(unittest.TestCase):
    def test_ids_and_enums_match_the_published_schema(self):
        text = (WIFI/'qmi_uim_refresh.rs').read_text(encoding='utf-8')
        for item in ['QMI_UIM_REFRESH_REGISTER: u16 = 0x002A',
                     'QMI_UIM_REFRESH_COMPLETE: u16 = 0x002C',
                     'QMI_UIM_REFRESH_INDICATION: u16 = 0x0033',
                     'STAGE_WAIT_FOR_OK: u8 = 0',
                     'STAGE_END_WITH_SUCCESS: u8 = 2',
                     'MAX_DENY']:
            self.assertIn(item, text)

    def test_vote_is_bounded_and_never_blocks_provisioning_forever(self):
        text = (WIFI/'qmi_uim_refresh.rs').read_text(encoding='utf-8')
        voter = text[text.index('impl Voter {'):text.index('fn handle_message(')]
        self.assertIn('if self.busy.load(Ordering::SeqCst)', voter)
        self.assertIn('if since.elapsed() < MAX_DENY', voter)
        self.assertIn('return false', voter)
        # An unrelated session/AID must never receive our vote.
        self.assertIn('fn event_matches(', text)
        self.assertIn('event.1 == session_type', text)

    def test_failure_and_unsupported_paths_are_not_treated_as_change(self):
        text = (WIFI/'qmi_uim_refresh.rs').read_text(encoding='utf-8')
        self.assertIn('RefreshEvent::Failed', text)
        self.assertIn('return None;', text)  # start() best-effort
        handle = text[text.index('fn handle_message('):text.index('/// Start a best-effort subscription.')]
        self.assertIn('STAGE_END_WITH_SUCCESS =>', handle)
        # Only a completed refresh produces Completed; the caller re-reads.
        self.assertNotIn('read_isim', handle)
        self.assertNotIn('read_bound', handle)

    def test_cellular_rechecks_material_and_defers_until_calls_drain(self):
        live = (CELL/'live.rs').read_text(encoding='utf-8')
        self.assertIn('session.identity_refresh.as_mut()', live)
        self.assertIn('verify_ims_registration_material(', live)
        self.assertIn('session.identity_stale = true', live)
        self.assertIn('session.reg_event.revoked || session.identity_stale', live)
        self.assertIn('refresh.set_busy(true)', live)
        self.assertIn('refresh.set_busy(false)', live)

    def test_actions_run_the_refresh_module(self):
        for name in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT/'.github/workflows'/name).read_text(encoding='utf-8')
            self.assertIn('connectivity::modems::ims::vowifi::qmi_uim::refresh::tests', text)
            self.assertIn('Empty regression filter', text)

if __name__ == '__main__':
    unittest.main()
