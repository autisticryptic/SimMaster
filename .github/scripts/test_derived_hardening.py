"""Keep bounded proposal expansion and terminal-auth propagation in production."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]

class DerivedHardeningTests(unittest.TestCase):
    def test_terminal_auth_is_propagated_at_every_ike_ladder_level(self):
        text=(ROOT/'backend/src/connectivity/modems/ims/vowifi/live.rs').read_text()
        for begin,end in (
            ('async fn run_live_ike_until_depth(', 'async fn try_live_epdg_addresses'),
            ('async fn try_live_epdg_addresses', 'async fn run_live_ike_until_depth_for_stack'),
            ('async fn run_live_ike_until_depth_for_stack', 'fn live_ike_socket_spec'),
        ):
            body=text[text.index(begin):text.index(end)]
            self.assertIn('terminal_ike_auth_rejection(&error)',body)
        body=text[text.index('async fn run_live_ike_until_depth_for_stack'):text.index('fn live_ike_socket_spec')]
        self.assertEqual(body.count('terminal_ike_auth_rejection(&error)'),2)
        self.assertIn('const LIVE_IKE_MAX_PROPOSAL_GROUPS_PER_PASS: usize = 2;',text)
        self.assertIn('const LIVE_IKE_MAX_TRANSPORT_PATHS_PER_PASS: usize = 2;',text)

    def test_new_rust_suites_are_in_both_ci_workflows(self):
        for workflow in ('beta-validation.yml','build-release.yml'):
            text=(ROOT/'.github/workflows'/workflow).read_text()
            for suite in ('vowifi::ike_state::tests','vowifi::live::derived_hardening_tests'):
                self.assertIn(suite,text)

if __name__=='__main__':unittest.main()
