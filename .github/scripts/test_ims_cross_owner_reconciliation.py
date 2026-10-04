"""No hardware/builds: wiring and authority boundaries for stale IMS recovery."""
from pathlib import Path
import unittest
ROOT = Path(__file__).resolve().parents[2]
QCM = ROOT / 'backend/src/hardware/devices/qcm410'


class CrossOwnerReconciliationTests(unittest.TestCase):
    def test_present_resources_need_explicit_plan_before_durable_intent(self):
        core = (QCM / 'primary_ims_profile_reconciliation.rs').read_text()
        fresh = core.split('        None => {', 1)[1].split('    // No retry loops', 1)[0]
        self.assertLess(fresh.index('expected_plan != Some(plan('), fresh.index('checkpoint('))
        self.assertIn('RECONCILE_MANUAL_REQUIRED', fresh)
        self.assertIn('identical fingerprints cannot rule', core)
        for phase in ('ReportingDispatched', 'DeleteDispatched'):
            self.assertIn(f'checkpoint(store, &source, &mut journal, Step::{phase})?', core)
        self.assertNotIn('release_with(', core)
        self.assertNotIn('receipt.before =', core)
        self.assertNotIn('loop {', core)

    def test_automatic_recovery_never_invents_a_plan(self):
        runtime = (QCM / 'primary_ims_profile_runtime.rs').read_text()
        auto = runtime.split('async fn recover_inner(', 1)[1]
        self.assertEqual(auto.count('reconciliation_io::run(io, file, &receipt, false, None)'), 3)
        self.assertNotIn('Some(&plan)', auto)
        self.assertIn('finish_archived()?', auto)
        gate = runtime.split('pub(crate) fn requires_pre_namespace_recovery()', 1)[1].split('async fn reboot_absence_io', 1)[0]
        self.assertIn('r.version == 2', gate)
        self.assertIn('reconciliation_io::has_pending', gate)

    def test_active_marker_blocks_both_ordinary_and_switch_admission(self):
        runtime = (QCM / 'primary_ims_profile_runtime.rs').read_text()
        self.assertGreaterEqual(runtime.count('reconciliation_io::ensure_no_pending('), 6)
        self.assertIn('Some("json" | "recovery")', runtime)
        lease = (QCM / 'primary_ims_profile_lease.rs').read_text()
        self.assertIn('runtime::ensure_no_pending_reconciliation(&store.file)?', lease)
        self.assertIn('"inspect-stale" | "reconcile-stale"', lease)
        cli = (ROOT / 'backend/src/main.rs').read_text()
        self.assertIn('"inspect-stale", "reconcile-stale"', cli)

    def test_provider_refuses_other_workers_namespaces_calls_or_modems(self):
        io = (QCM / 'primary_ims_profile_reconciliation_io.rs').read_text()
        for token in ('require_stopped()?', 'no_bearer_work()?', 'live_contexts()',
                      'retirement::old_owner_absent', 'ListCalls', 'count() != 1',
                      'verify_retired_network_readonly', 'read_dir("/run/netns")',
                      '"/proc/1/ns/net"', '"xfrm", kind, "list"', 'Some(&false)'):
            self.assertIn(token, io)
        for forbidden in ('"systemctl"', '"netns", "delete"', '"reboot"', 'kill('):
            self.assertNotIn(forbidden, io)
        self.assertIn('Step::AbsentVerified', io)
        self.assertIn('self.finish_archived()', io)
        self.assertIn('retirement::archive_source', io)
        self.assertIn('sync_all()', io)
        self.assertIn('meta.nlink() != 1', io)

    def test_both_actions_gates_run_new_suites_without_replacing_old_coverage(self):
        for name in ('beta-validation.yml', 'build-release.yml'):
            workflow = (ROOT / '.github/workflows' / name).read_text()
            for suite in ('reconciliation::tests', 'reconciliation_io::tests', 'switch_drain_tests'):
                self.assertIn('profile_lease::runtime::' + suite, workflow)
            self.assertIn('offline-registration-sim/run.py --history', workflow)
            self.assertIn('cellular_ims::live::refresh_tests', workflow)


if __name__ == '__main__':
    unittest.main()
