"""MM-only reattach guards; Rust fake-IO regressions execute on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
DEVICE = ROOT / "backend/src/hardware/devices/qcm410"


class MmPcscfRecoveryBoundaryTests(unittest.TestCase):
    def test_recovery_uses_the_original_owned_lease_and_unique_mm_bus(self):
        text = (DEVICE / "primary_ims_recovery.rs").read_text()
        self.assertRegex(text, r"leases\(\)\s*\.lock\(\)")
        self.assertIn("Arc::clone(&lease.bus)", text)
        self.assertIn("pcscf_binding_snapshot", text)
        self.assertIn("self.lease.is_done()", text)
        self.assertIn("sim_identity", text)
        self.assertIn("eps_settings", text)
        for forbidden in ("qmicli", "Command::new", 'Connection::system()', '"AT+CGACT=', '"AT+CGDCONT=', '"SetInitialEpsBearerSettings"'):
            self.assertNotIn(forbidden, text)

    def test_existing_pcscf_publication_is_not_weakened_by_the_recovery_hint(self):
        text = (DEVICE / "primary_ims_pcscf.rs").read_text()
        normal = text[text.index("pub(super) async fn discover_with"):text.index("pub(super) fn missing_reporting_context")]
        self.assertIn("profile_id.is_none_or(|profile| profile == u32::from(*cid))", normal)
        hint = text.split("pub(super) fn missing_reporting_context", 1)[1].split("#[cfg(test)]", 1)[0]
        for guard in ("context_rows", "expected.pcscf.is_empty()", "state.active.as_slice()", "expected.ipv6_prefix != Some(64)", "!row.candidates.is_empty()"):
            self.assertIn(guard, hint)

    def test_budget_is_exclusive_persistent_and_never_automatically_cleared(self):
        text = (DEVICE / "primary_ims_recovery.rs").read_text()
        self.assertIn('"/run/simadmin/mm-pcscf-recovery"', text)
        self.assertIn("create_new(true)", text)
        self.assertIn("file.sync_all()", text)
        production = text.split("#[cfg(test)]", 1)[0]
        self.assertNotIn("remove_file", production)
        self.assertNotIn("remove_dir", production)
        self.assertIn("run(Step::Enable).await", production)

    def test_handler_preserves_families_and_guards_calls_data_and_native(self):
        text = (ROOT / "backend/src/api/handlers.rs").read_text()
        recovery = text[text.index("async fn run_line_cellular_ims_restore_batch("):text.index("async fn run_line_cellular_ims_restore_round(")]
        for guard in ("!status.registered", 'Some("derived")', "RUNTIME_ALL_PCSCF_FAILED", "profile.airplane_mode_enabled", "profile.data_connection_enabled", "profile.vowifi.enabled", "active_native().is_none()", "list_calls_for_line", "calls.calls.is_empty()", "plan.budget_available()", "tokio::spawn", "cleanup_live_for_profile_switch", "transition_lock", "bearer_operation_lock"):
            self.assertIn(guard, recovery)
        self.assertEqual(recovery.count("run_line_cellular_ims_restore_round(app, line, source, generation).await"), 2)
        self.assertIn('line.cellular_ims.mm_binding_ready()', recovery)
        for forbidden in ("set_line_cellular_ims_ip_families", "Ipv6Only", "AT+", "46011", "sim06", "set_airplane_mode", "SetInitialEpsBearerSettings"):
            self.assertNotIn(forbidden, recovery)

    def test_radio_compensation_is_independent_of_ims_cancellation(self):
        text = (ROOT / "backend/src/api/handlers.rs").read_text()
        policy = text[text.index("fn mm_pcscf_radio_restore_policy_current("):text.index("fn mm_pcscf_recovery_stopped(")]
        self.assertIn("!profile.airplane_mode_enabled", policy)
        self.assertIn("binding.modem_path == modem", policy)
        self.assertIn("active_native().is_none()", policy)
        for forbidden in ("generation()", "profile.enabled", "cellular_ims_connection_enabled", "data_connection_enabled", "vowifi.enabled", "line_has_call"):
            self.assertNotIn(forbidden, policy)
        driver = (DEVICE / "primary_ims_recovery.rs").read_text()
        self.assertIn('"ListCalls"', driver)
        self.assertIn("mm_pcscf_calls_unavailable", driver)
        self.assertIn("restore_radio().await", driver)
        self.assertIn("state_wait_checks_cancellation_before_and_after_io", driver)
        self.assertIn("cancellation_after_disable_restores_radio_but_does_not_continue", driver)

    def test_actions_execute_recovery_and_admission_regressions(self):
        for name in ("build-release.yml", "beta-validation.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            self.assertIn("hardware::devices::qcm410::primary_ims_lifecycle::recovery::tests", text)
            self.assertIn("api::handlers::tests::cellular_ims_profile_batch", text)


if __name__ == "__main__":
    unittest.main()
