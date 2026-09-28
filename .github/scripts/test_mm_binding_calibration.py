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
        block = registry[registry.index('for (line, binding) in &existing_lines {'):]
        self.assertLess(block.index('observe_mm_binding(binding)'), block.index('reconcile_ue_context(line, binding)'))

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

    def test_regressions_are_scheduled_in_both_workflows(self):
        for name in ('build-release.yml', 'beta-validation.yml'):
            text = (ROOT / '.github/workflows' / name).read_text()
            for suite in ('cellular_ims::mm_binding::tests', 'cellular_ims::runtime::tests', 'primary_ims_lifecycle::ip_config_dbus_tests'):
                self.assertIn(suite, text)


if __name__ == '__main__':
    unittest.main()
