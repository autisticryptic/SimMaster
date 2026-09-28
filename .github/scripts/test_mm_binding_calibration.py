"""Wiring guards; behavioural coverage lives in Rust and isolated D-Bus tests."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / 'backend/src'


class MmBindingCalibrationTests(unittest.TestCase):
    def test_inventory_checks_identity_before_presence_shortcut(self):
        main = (SRC / 'main.rs').read_text()
        self.assertLess(main.index('mm_calibration_ticket()'), main.index('if binding.present == was_present'))
        registry = (SRC / 'services/line_registry.rs').read_text()
        refresh = registry[registry.index('pub async fn refresh('):registry.index('fn observe_mm_inventory(')]
        self.assertLess(refresh.index('Self::observe_mm_inventory('), refresh.index('discover_readers().await'))
        self.assertLess(refresh.index('Self::observe_mm_inventory('), refresh.index('reconcile_ue_context('))
        self.assertNotIn('observe_mm_binding(binding)', refresh)
        self.assertIn('discovery_failed', refresh)
        self.assertIn('retained_mm_lines', refresh)

    def test_cleanup_requires_current_ticket_and_does_not_rewrite_cids(self):
        handlers = (SRC / 'api/handlers.rs').read_text()
        block = handlers[handlers.index('pub(crate) async fn recalibrate_line_mm_binding('):handlers.index('pub(crate) async fn suspend_line_runtime_for_hotplug(')]
        self.assertEqual(block.count('mm_calibration_ticket() != Some(ticket)'), 2)
        self.assertIn('bearer_operation_lock.try_lock()', block)
        self.assertIn('cellular_ims_connect_lock.try_lock()', block)
        self.assertIn('advance_guard().await', block)
        self.assertLess(block.index('discard_live_for_mm_binding_change'), block.index('finish_mm_calibration'))
        for forbidden in ('AT+', 'AT$', 'Enable', 'power_cycle', 'set_initial_eps', 'clear_budget'):
            self.assertNotIn(forbidden, block)

    def test_old_sim_cleanup_does_not_unregister_or_restore_at_profiles(self):
        live = (SRC / 'connectivity/modems/ims/cellular_ims/live.rs').read_text()
        block = live[live.index('pub async fn discard_live_for_mm_binding_change('):live.index('pub async fn disconnect_live_for_line(')]
        self.assertIn('listener.await', block)
        self.assertIn('cleanup_live_session_with_binding(live, false)', block)
        self.assertNotIn('unregister_live_session', block)
        self.assertIn('verify_mm_sim_binding(iccid, *slot)', live)
        self.assertIn('.update_current(generation, |state|', live)

    def test_pending_mm_failures_never_replay_selector_cleanup(self):
        live = (SRC / 'connectivity/modems/ims/cellular_ims/live.rs').read_text()
        block = live[live.index('async fn cleanup_pending_native_bearer('):live.index('async fn disable_pcscf_reporting(')]
        branch = block[:block.index('if let Some(native) = native_bearer.take()')]
        self.assertIn('!crate::hardware::cellular::backends::is_native_selector(modem_id)', branch)
        self.assertIn('cleanup_unverified_native_bearer(native_bearer).await', branch)
        self.assertIn('return;', branch)
        self.assertNotIn('disable_pcscf_reporting(', branch)
        lifecycle = (SRC / 'hardware/devices/qcm410/primary_ims_lifecycle.rs').read_text()
        self.assertIn('mm_single_sim_slot_zero_matches_inventory_logical_slot_one', lifecycle)

    def test_batch_and_live_tasks_keep_immutable_generation_scopes(self):
        handlers = (SRC / 'api/handlers.rs').read_text()
        batch = handlers[handlers.index('async fn run_line_cellular_ims_restore_round('):handlers.index('pub fn spawn_cellular_ims_auto_restore(')]
        self.assertIn('for_admission_generation(batch_generation)', batch)
        self.assertNotIn('let batch_generation = line.cellular_ims.generation()', batch)
        self.assertNotIn('line.cellular_ims\n            .update(', batch)
        self.assertIn('!runtime.task_is_current()', batch)
        live = (SRC / 'connectivity/modems/ims/cellular_ims/live.rs').read_text()
        self.assertIn('runtime.for_generation(generation)', live)
        self.assertIn('aka_runtime.task_is_current()', live)
        self.assertIn('mm_sim_changed_during_identity_read', live)

    def test_expected_sim_is_checked_before_mm_create(self):
        session = (SRC / 'hardware/devices/qcm410/primary_ims_session.rs').read_text()
        start = session[session.index('async fn start_inner('):session.index('pub async fn verify_sim_binding(')]
        self.assertLess(start.index('bus.verify_expected_sim(iccid, *slot)'), start.index('prepare_with('))
        strategy = (SRC / 'connectivity/modems/ims/cellular_ims/native_bearer.rs').read_text()
        self.assertIn('expected_mm_sim.map(|(iccid, slot)| (iccid.as_str(), *slot))', strategy)

    def test_uncertain_mm_creation_and_network_cleanup_keep_receipts(self):
        lifecycle = (SRC / 'hardware/devices/qcm410/primary_ims_lifecycle.rs').read_text()
        create = lifecycle[lifecycle.index('pub async fn create('):lifecycle.index('pub async fn status(')]
        self.assertNotIn('timed(', create)
        self.assertIn('qca410_primary_mm_create_unresolved', lifecycle)
        self.assertIn('retain_unverified_network(&record)?', lifecycle)
        session = (SRC / 'hardware/devices/qcm410/primary_ims_session.rs').read_text()
        self.assertLess(session.index('PendingCreate::new('), session.index('self.bus.create('))
        self.assertNotIn('let _ = self.bus.delete(&bearer)', session)

    def test_reporting_revalidates_after_acquiring_serial_permit(self):
        recovery = (SRC / 'hardware/devices/qcm410/primary_ims_recovery.rs').read_text()
        command = recovery[recovery.index('async fn command_checked'):recovery.index('async fn sim_identity(')]
        self.assertLess(command.index('serial::with_serial_for('), command.index('bus.ensure_sim_binding().await?'))
        self.assertLess(command.index('!allowed().await'), command.index('.call::<_, _, String>("Command"'))
        self.assertEqual(command.count('bus.ensure_sim_binding().await?'), 2)

    def test_regressions_are_scheduled_in_both_workflows(self):
        for name in ('build-release.yml', 'beta-validation.yml'):
            text = (ROOT / '.github/workflows' / name).read_text()
            for suite in ('cellular_ims::mm_binding::tests', 'cellular_ims::runtime::tests', 'primary_ims_lifecycle::ip_config_dbus_tests'):
                self.assertIn(suite, text)


if __name__ == '__main__':
    unittest.main()
