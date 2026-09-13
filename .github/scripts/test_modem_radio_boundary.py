from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "backend/src"


def handler(name):
    text = (SRC / "api/handlers.rs").read_text()
    # These are top-level functions; nested blocks cannot start at column zero.
    start = re.search(rf"^(?:pub(?:\(crate\))? )?async fn {name}\(", text, re.M)
    if start is None:
        raise AssertionError(f"missing live handler {name}")
    end = text.index("\n}", start.end()) + 2
    return text[start.start():end]


class ModemRadioBoundaryTests(unittest.TestCase):
    def test_radio_contract_and_planner_do_not_require_a_backend_or_io(self):
        for path in ("hardware/cellular/radio.rs", "services/orchestrator/radio_intent.rs"):
            with self.subTest(path=path):
                text = (SRC / path).read_text()
                for forbidden in ("use zbus::", "modem_manager::", "Connection::system(", "std::process::"):
                    self.assertFalse(forbidden in text, f"backend/IO leak: {forbidden}")

    def test_mm_airplane_calls_are_confined_to_the_adapter(self):
        allowed = {"hardware/cellular/mm_radio.rs", "hardware/cellular/modem_manager.rs"}
        for path in SRC.rglob("*.rs"):
            text = path.read_text()
            with self.subTest(path=str(path.relative_to(SRC))):
                self.assertFalse("get_airplane_mode_for_modem" in text, "legacy getter remains")
                if path.relative_to(SRC).as_posix() not in allowed:
                    self.assertFalse("set_airplane_mode_for_modem" in text, "setter bypasses adapter")

    def test_saved_data_starts_no_longer_accept_a_stale_profile(self):
        old_call = re.compile(r"start_line_data_runtime\s*\(\s*&?app\s*,\s*&?line\s*,")
        for path in SRC.rglob("*.rs"):
            with self.subTest(path=str(path.relative_to(SRC))):
                self.assertIsNone(old_call.search(path.read_text()))
        start = handler("start_line_data_runtime_locked")
        self.assertLess(start.index("get_line_profile("), start.index("data_start_admission("))
        self.assertLess(start.index("data_start_admission("), start.index("get_is_roaming_for_modem("))
        for name in ("start_line_data_runtime", "start_temporary_line_data_runtime"):
            text = handler(name)
            self.assertLess(text.index("bearer_operation_lock.lock()"), text.index("start_line_data_runtime_locked("))

    def test_intent_gates_precede_sampling_persistence_and_apply(self):
        for name in ("restore_line_runtime_intents", "apply_line_airplane_intent"):
            text = handler(name)
            self.assertLess(text.index("bearer_operation_lock.lock()"), text.index("line.binding()"))
            self.assertLess(text.index("line.binding()"), text.index("apply_line_airplane_mode_locked("))
        text = handler("apply_line_airplane_intent")
        self.assertLess(text.index("bearer_operation_lock.lock()"), text.index(".set_line_airplane_mode("))
        apply = handler("apply_line_airplane_mode_locked")
        self.assertLess(apply.index("stop_line_data_runtime_locked("), apply.index(".set_airplane_mode("))
        self.assertLess(apply.index("disconnect_live_for_line("), apply.index(".set_airplane_mode("))
        self.assertNotIn("connect_vowifi_on_line(", apply)

    def test_vowifi_preparation_does_not_implicitly_enable_cellular_rf(self):
        text = (SRC / "api/handlers.rs").read_text()
        self.assertFalse("ensure_line_radio_state_for_vowifi" in text, "old RF helper/caller remains")
        pause = handler("pause_cellular_data_for_vowifi")
        for forbidden in ("modem_manager::", "modem_radio", "set_airplane_mode", "set_modem_enabled"):
            self.assertNotIn(forbidden, pause)
        self.assertIn("stop_line_data_runtime(", pause)
        stop = handler("restore_cellular_and_reset_vowifi")
        for forbidden in ("modem_manager::", "modem_radio", "set_airplane_mode", "set_modem_enabled"):
            self.assertNotIn(forbidden, stop)

    def test_watchdog_and_ims_recheck_intent_inside_the_bearer_gate(self):
        watchdog = handler("reconcile_line_data_health")
        self.assertLess(watchdog.index("bearer_operation_lock.try_lock()"), watchdog.index("get_line_profile("))
        restore = handler("run_line_cellular_ims_restore_batch").split("let result = {", 1)[1]
        self.assertLess(restore.index("bearer_operation_lock.lock()"), restore.index("get_line_profile("))
        for gate in ("line.cellular_ims.generation()", "!profile.enabled", "profile.airplane_mode_enabled", "!profile.cellular_ims_connection_enabled"):
            self.assertLess(restore.index(gate), restore.index("prepare_line_data_slot_for_cellular_ims("))

    def test_automation_keeps_an_explicit_temporary_path_and_current_intent_cleanup(self):
        text = (SRC / "services/automation/tasks/consume_data.rs").read_text()
        self.assertIn("start_temporary_line_data_runtime(", text)
        self.assertIn("finish_temporary_line_data_runtime(", text)
        cleanup = handler("finish_temporary_line_data_runtime")
        self.assertLess(cleanup.index("bearer_operation_lock.lock()"), cleanup.index("get_line_profile("))
        self.assertIn("DataStartPurpose::SavedIntent", cleanup)

    def test_invalid_configuration_cannot_restart_modemmanager_first(self):
        text = (SRC / "main.rs").read_text()
        self.assertLess(text.index("ConfigManager::try_new("), text.index("ensure_modemmanager_debug_override();"))

    def test_new_regressions_are_selected_by_both_candidate_workflows(self):
        for name in ("build-release.yml", "beta-validation.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            for group in (
                "hardware::cellular::radio::tests",
                "hardware::cellular::mm_radio::tests",
                "services::orchestrator::radio_intent::tests",
                "services::automation::tasks::consume_data::tests",
            ):
                with self.subTest(workflow=name, group=group):
                    self.assertIn(group + " \\", text)
            self.assertIn("http_router_tests::radio_intent_", text)


if __name__ == "__main__":
    unittest.main()
