"""Structural guards for derived-only production profile ownership."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "backend/src"
QCM = SRC / "hardware/devices/qcm410"


def part(text, start, end):
    return text.split(start, 1)[1].split(end, 1)[0]


class RuntimeProfileBoundaryTests(unittest.TestCase):
    def test_runtime_is_device_opt_in_and_ims_only_derived(self):
        live = (SRC / "connectivity/modems/ims/cellular_ims/live.rs").read_text()
        scope = part(live, "fn runtime_profile_preparation_allowed", "async fn prepare_profile_probe_admission")
        self.assertIn("DataSlotMode::UeNativeIms", scope)
        self.assertIn("is_standard_derived_profile", scope)
        connect = part(live, "let (runtime_profile_transport, _ordinary_profile_guard)", "let mut request = BearerRequest")
        self.assertIn("supports_owned_mm_profile_preparation", connect)
        self.assertIn("diagnostic_profile.is_none()", connect)
        self.assertIn("task.task_is_current() && task.generation() == generation", connect)
        self.assertLess(connect.index("runtime::recover("), connect.index("runtime::prepare("))
        self.assertIn("ordinary_profile_guard(", connect)
        self.assertIn("if profile_prepared_by_runtime", connect)

    def test_runtime_does_not_start_probe_or_shutdown_process(self):
        runtime = (QCM / "primary_ims_profile_runtime.rs").read_text()
        for forbidden in ("shutdown_owned(", "probe_owned_profile_once(", "maintain(", "remove_dir_all("):
            self.assertNotIn(forbidden, runtime)
        self.assertIn("CreationMethod::At", runtime)
        self.assertIn("establish_with_bus(", runtime)
        self.assertIn("context.bus.for_profile_attempt(Arc::clone(&current))", runtime)
        self.assertIn("Some(attempt_bus)", runtime)
        self.assertIn("RuntimePhase::BearerPending", runtime)
        self.assertIn("network", runtime)
        self.assertIn("verify_retired_network_readonly", runtime)

    def test_ordinary_guard_holds_same_flock_and_rejects_pending_profile(self):
        runtime = (QCM / "primary_ims_profile_runtime.rs").read_text()
        guard = part(runtime, "fn ordinary_profile_guard(", "fn device_lock(")
        self.assertIn("let lock = device_lock(&file)?", guard)
        self.assertIn("read_receipt(&file)?.is_some()", guard)
        self.assertIn("Ok(lock)", guard)
        self.assertIn("LOCK_EX | libc::LOCK_NB", runtime)

    def test_unverified_profile_recovery_blocks_global_namespace_sweep(self):
        main = (SRC / "main.rs").read_text()
        self.assertIn("if hardware::devices::recover_owned_ims_sessions().await {", main)
        driver = (QCM / "mod.rs").read_text()
        block = part(driver, "fn recover_owned_ims(&self)", "\n    }\n}")
        self.assertLess(block.index("runtime::recover("), block.index("primary_ims_lifecycle::recover_owned().await"))
        lease = (QCM / "primary_ims_profile_lease.rs").read_text()
        self.assertIn("mm_ims_profile_lease_runtime_ownership_pending", lease)
        self.assertIn("(1, false) | (2, true)", lease)

    def test_generation_reaches_create_connect_and_shutdown_is_ordered(self):
        lifecycle = (QCM / "primary_ims_lifecycle.rs").read_text()
        for start, end in (("pub async fn create(&self", "pub async fn status"), ("pub async fn connect(&self", "async fn disconnect")):
            block = part(lifecycle, start, end)
            self.assertLess(block.index(".await?"), block.index("self.authorize_setup_dispatch()?"))
            self.assertLess(block.index("self.authorize_setup_dispatch()?"), block.index(".call"))
        driver = (QCM / "mod.rs").read_text()
        block = part(driver, "fn shutdown_owned_ims(&self)", "fn recover_owned_ims(&self)")
        self.assertLess(block.index("primary_ims_lifecycle::shutdown_owned().await"), block.index("runtime::shutdown_profiles()"))
        main = (SRC / "main.rs").read_text()
        shutdown = part(main, "async fn wait_for_shutdown_signal(", "/// Assemble the HTTP router")
        self.assertLess(shutdown.index("cleanup_live_for_shutdown"), shutdown.index("hardware::devices::shutdown_owned_ims_sessions().await"))
        self.assertIn("let _ = ims_cleanup_wait.await", main)

    def test_runtime_outer_timeout_keeps_inner_late_result_coupled_to_profile(self):
        runtime = (QCM / "primary_ims_profile_runtime.rs").read_text()
        self.assertIn("tokio::time::timeout(Duration::from_secs(120), receiver)", runtime)
        session = (QCM / "primary_ims_session.rs").read_text()
        self.assertIn("has_profile_setup_guard()", session)
        receive = part(session, "async fn receive_setup_result<T>", "async fn deliver_setup")
        guarded = part(receive, "if profile_owns_timeout {", "} else {")
        self.assertIn("receiver.await", guarded)
        self.assertNotIn("timeout(", guarded)

    def test_both_ci_suites_execute_runtime_and_private_bus_regressions(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            for suffix in ("runtime::tests", "runtime::dbus_tests"):
                self.assertIn("hardware::devices::qcm410::primary_ims_lifecycle::profile_lease::" + suffix, text)


if __name__ == "__main__":
    unittest.main()
