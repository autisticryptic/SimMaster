"""Static pre-lpac safety boundaries; behavioural flock tests live in Rust.

Run explicitly in Actions. This test does not build code or access hardware.
"""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "backend/src"
QCM = SRC / "hardware/devices/qcm410"


def code_only(text):
    # Preserve offsets while ignoring braces/tokens in Rust comments/strings.
    return re.sub(
        r'"(?:\\.|[^"\\])*"|//[^\n]*|/\*.*?\*/',
        lambda match: " " * len(match.group()),
        text,
        flags=re.S,
    )


def block(text, marker):
    start = text.index(marker) + len(marker)
    code = code_only(text)
    opening = code.index("{", start)
    depth = 1
    for index in range(opening + 1, len(code)):
        if code[index] == "{":
            depth += 1
        elif code[index] == "}":
            depth -= 1
            if depth == 0:
                return text[opening + 1:index]
    raise AssertionError(f"unclosed block: {marker}")


class ImsSwitchDrainBoundaryTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.handler = block(
            (SRC / "api/handlers.rs").read_text(encoding="utf-8"),
            "pub async fn enable_esim_profile_handler(",
        )
        cls.runtime = (QCM / "primary_ims_profile_runtime.rs").read_text(encoding="utf-8")

    def test_drain_error_returns_before_spawn_lpac_or_mm_mutation(self):
        handler = self.handler
        drain = block(handler, "let ims_switch_drain = if needs_ims_drain")
        failure = block(drain, "Err(reason) =>")
        self.assertIn("StatusCode::CONFLICT", failure)
        self.assertIn("return (", failure)
        self.assertIn("::error(reason)", failure)
        self.assertIn(".into_response();", failure)
        self.assertIn("Ok(guard) => Some(guard)", drain)
        self.assertNotIn("unwrap_or", drain)
        boundaries = [
            "begin_mm_switch()",
            "discard_live_for_mm_binding_change(",
            "qcm410::esim_switch_drain_guard()",
            "tokio::spawn(",
            ".enable_profile(",
            "power_cycle_sim_for_profile_switch_via_modem(",
        ]
        positions = [handler.index(token) for token in boundaries]
        self.assertEqual(positions, sorted(positions))
        self.assertEqual(handler.count(".enable_profile("), 1)
        self.assertEqual(handler.count("power_cycle_sim_for_profile_switch_via_modem("), 1)
        self.assertLess(positions[2], handler.index("own_numbers::clear("))

    def test_guard_is_moved_into_full_background_operation_scope(self):
        operation = block(self.handler, "async move")
        code = code_only(operation)
        for guard in ("let _mm_switch = mm_switch;", "let _ims_switch_drain = ims_switch_drain;"):
            self.assertIn(guard, code)
            prefix = code[:code.index(guard)]
            self.assertEqual(prefix.count("{"), prefix.count("}"), "guard must not live in a short inner scope")
            self.assertLess(code.index(guard), code.index(".enable_profile("))
            self.assertLess(code.index(guard), code.index("power_cycle_sim_for_profile_switch_via_modem("))
        for token in ("drop(ims_switch_drain", "drop(_ims_switch_drain", "forget(", "let _ = ims_switch_drain"):
            self.assertNotIn(token, code_only(self.handler))
        self.assertIn("#[must_use", self.runtime)
        self.assertIn("_lock: fs::File", block(self.runtime, "pub struct EsimSwitchDrainGuard"))

    def test_cleanup_lock_order_and_no_serial_permit_around_proof(self):
        cleanup = block(self.handler, "if mm_switch.is_some()")
        tokens = (
            "bearer_operation_lock.lock().await",
            "cellular_ims_connect_lock.lock().await",
            "advance_guard().await",
            "discard_live_for_mm_binding_change(",
        )
        positions = [cleanup.index(token) for token in tokens]
        self.assertEqual(positions, sorted(positions))
        before_spawn = self.handler.split("tokio::spawn(", 1)[0]
        self.assertNotIn("with_serial", code_only(before_spawn))
        self.assertNotIn("serial::", code_only(before_spawn))
        self.assertNotIn("esim_switch_drain_guard", cleanup)

    def test_qcm_mm_scope_does_not_depend_on_current_binding_readiness(self):
        scope = block(self.handler, "let needs_ims_drain =")
        for token in ("DeviceKind::Qcm410", 'line_kind != "reader"', "active_native().is_none()", "!crate::hardware::cellular::backends::is_native_selector"):
            self.assertIn(token, scope)
        for token in ("mm_switch", "binding.present", "mm_binding_ready", "modem_path.is_empty"):
            self.assertNotIn(token, code_only(scope))
        # Readers, other drivers and native backends retain the old path.
        self.assertIn("} else {\n        None\n    };", self.handler)

    def test_missing_admission_ticket_fails_closed_even_after_receipt_proof(self):
        failure = block(self.handler, "if needs_ims_drain && mm_switch.is_none()")
        self.assertIn("StatusCode::CONFLICT", failure)
        self.assertIn("return (", failure)
        self.assertIn("mm_ims_switch_admission_unverified", failure)
        self.assertLess(
            self.handler.index("qcm410::esim_switch_drain_guard()"),
            self.handler.index("if needs_ims_drain && mm_switch.is_none()"),
        )
        self.assertLess(
            self.handler.index("if needs_ims_drain && mm_switch.is_none()"),
            self.handler.index("own_numbers::clear("),
        )

    def test_global_mm_reset_is_not_disguised_as_multiline_drain(self):
        scope = block(self.handler, "\n    if needs_ims_drain")
        self.assertIn("app.line_registry.all().await", scope)
        self.assertIn('binding.line_kind != "reader"', scope)
        self.assertIn("!Arc::ptr_eq(&other, &line) || binding.slot_conflict", scope)
        self.assertNotIn("binding.present", code_only(scope))
        self.assertIn("mm_ims_switch_multiline_drain_unverified", scope)
        self.assertIn("StatusCode::CONFLICT", scope)
        self.assertIn("return (", scope)
        self.assertNotIn("discard_live", scope)
        proof = block(self.runtime, "fn esim_switch_drain_guard()")
        self.assertIn("PENDING.load(Ordering::Acquire)", proof)
        self.assertIn("!leases().lock().unwrap().is_empty()", proof)
        self.assertIn("Path::new(STATE_DIR)", proof)
        self.assertIn("context.strong_count() != 0", proof)
        self.assertEqual(proof.count("reject_other_receipts(&file)?"), 2)

    def test_receipt_existence_and_context_flock_are_fail_closed(self):
        proof = block(self.runtime, "fn esim_switch_drain_guard()")
        self.assertLess(proof.index("let lock = device_lock(&file)?"), proof.index("switch_drain_under_lock("))
        under_lock = block(self.runtime, "fn switch_drain_under_lock(")
        self.assertIn("fs::symlink_metadata(file)", under_lock)
        self.assertIn("ErrorKind::NotFound", under_lock)
        self.assertIn('Ok(_) => return Err("mm_ims_profile_runtime_receipt_pending".into())', under_lock)
        self.assertIn("Err(_) => return Err(RUNTIME_ERROR.into())", under_lock)
        self.assertLess(under_lock.index("symlink_metadata"), under_lock.index("no_other_work()?"))
        self.assertIn("Ok(EsimSwitchDrainGuard { _lock: lock })", under_lock)
        context = block(self.runtime, "struct Context")
        self.assertIn("_lock: fs::File", context)
        lock = block(self.runtime, "fn open_device_lock(")
        for token in ("O_NOFOLLOW", "O_CLOEXEC", "LOCK_EX | libc::LOCK_NB", "metadata.nlink() != 1"):
            self.assertIn(token, lock)

    def test_pending_marker_proof_never_parses_or_recovers_ownership(self):
        proof = block(self.runtime, "fn switch_bearer_work_absent(")
        self.assertIn("pending != 0 || live", proof)
        self.assertIn('Some("json" | "create")', proof)
        self.assertIn("fs::symlink_metadata(path)", proof)
        self.assertIn("directory.parent().ok_or(RUNTIME_ERROR)?", proof)
        self.assertIn("!metadata.file_type().is_symlink()", proof)
        self.assertIn("metadata.uid() == unsafe { libc::geteuid() }", proof)
        self.assertIn("metadata.mode() & 0o022 == 0", proof)
        self.assertLess(proof.index("fs::symlink_metadata(path)"), proof.index("fs::read_dir(directory)"))
        barrier = self.runtime.split("pub struct EsimSwitchDrainGuard", 1)[1].split("fn device_lock(", 1)[0]
        for token in ("read_receipt(", "recover_owned(", "identity_io(", "remove_file(", ".remove()", "release_with(", "AT+", "AT$"):
            self.assertNotIn(token, code_only(barrier))
        identity = block(self.runtime, "async fn identity_io_with(")
        self.assertIn("original.bus_id != old.bus_id || original.owner != old.owner", identity)
        self.assertIn("old.stable_sim_fingerprint.as_ref() != Some(&stable)", identity)

    def test_behavioral_tests_are_wired_and_cover_real_cleanup(self):
        self.assertRegex(
            self.runtime,
            r'#\[cfg\(test\)\]\s*#\[path = "primary_ims_switch_drain_tests.rs"\]\s*mod switch_drain_tests;',
        )
        lifecycle = (QCM / "primary_ims_lifecycle.rs").read_text(encoding="utf-8")
        self.assertRegex(
            lifecycle,
            r'#\[cfg\(target_os = "linux"\)\]\s*#\[path = "primary_ims_profile_lease.rs"\]\s*pub mod profile_lease;',
        )
        for workflow in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / workflow).read_text(encoding="utf-8")
            self.assertIn("hardware::devices::qcm410::primary_ims_lifecycle::profile_lease::runtime::switch_drain_tests", text)
            self.assertIn("unittest discover -s .github/scripts", text)
        tests = (QCM / "primary_ims_switch_drain_tests.rs").read_text(encoding="utf-8")
        cleanup = block(tests, "async fn ims_switch_drain_verified_cleanup_clears_receipt_but_retains_context_flock()")
        for token in ("RuntimeStore::new(", "release_with(&io, &store, receipt)", "store.receipt().unwrap().is_none()", "drop(context_lock)", "temp.drain(0, false).unwrap()"):
            self.assertIn(token, cleanup)
        self.assertNotIn("remove_file", code_only(cleanup))
        self.assertIn("async fn ims_switch_drain_failed_cleanup_retains_receipt_after_context_drop()", tests)
        store = block(self.runtime, "impl Store for RuntimeStore")
        remove = block(store, "fn remove(")
        self.assertLess(remove.index("self.disk.remove()"), remove.index("state.receipt = None"))
        self.assertIn("self.disk.save(receipt)", remove)
        self.assertIn("state.poisoned = true", remove)

    def test_inventory_reservation_covers_global_mutation_but_not_final_refresh(self):
        handler = self.handler
        self.assertLess(handler.index("reserve_esim_switch_inventory()"), handler.index("app.line_registry.all().await"))
        self.assertLess(handler.index("app.line_registry.all().await"), handler.index("begin_mm_switch()"))
        operation = block(handler, "async move")
        positions = [operation.index(token) for token in (
            "let mut switch_inventory = switch_inventory;",
            ".enable_profile(",
            "power_cycle_sim_for_profile_switch_via_modem(",
            "drop(switch_inventory.take());",
            "bg_app.line_registry.refresh().await",
        )]
        self.assertEqual(positions, sorted(positions))
        registry = (SRC / "services/line_registry.rs").read_text(encoding="utf-8")
        reserve = block(registry, "pub fn reserve_esim_switch_inventory(")
        self.assertIn("Arc::clone(&self.refresh_lock)", reserve)
        self.assertIn("try_lock_owned()", reserve)
        refresh = block(registry, "pub async fn refresh(")
        self.assertLess(refresh.index("self.refresh_lock.lock().await"), refresh.index("self.observations.discover().await"))
        self.assertIn("esim_switch_inventory_reservation_blocks_discovery_until_release", registry)
        self.assertIn("esim_switch_inventory_rejects_inflight_refresh_and_releases_on_cancel", registry)

    def test_forwarder_uses_barrier_not_point_in_time_profile_hint(self):
        lease = (QCM / "primary_ims_profile_lease.rs").read_text(encoding="utf-8")
        wrapper = block(lease, "pub fn esim_switch_drain_guard()")
        self.assertIn("runtime::esim_switch_drain_guard()", wrapper)
        self.assertNotIn("ensure_no_pending_profile", wrapper)
        driver = (QCM / "mod.rs").read_text(encoding="utf-8")
        self.assertIn("pub use primary_ims_lifecycle::profile_lease::esim_switch_drain_guard;", driver)


if __name__ == "__main__":
    unittest.main()
