from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "backend/src"


class NativeBackendBoundaryTests(unittest.TestCase):
    def test_native_implementation_never_executes_mm_or_service_takeover(self):
        forbidden = re.compile(r'Command::new\("(?:mmcli|ModemManager|systemctl|nmcli)"\)')
        for source in (SRC / "hardware/cellular/backends").glob("*.rs"):
            with self.subTest(module=source.name):
                self.assertIsNone(forbidden.search(source.read_text()))

    def test_main_conditionally_runs_mm_and_nm_startup(self):
        text = (SRC / "main.rs").read_text()
        self.assertRegex(text, r"if using_mm\s*\{[^}]*ensure_mm_handover_clear[^}]*ensure_modemmanager_debug_override\(\)")
        self.assertRegex(text, r"if using_mm\s*\{\s*let nm_result = ensure_nm_modem_profile")
        self.assertIn("backends::initialize(", text)

    def test_native_selection_is_opt_in_and_default_mm_is_not_persisted_as_a_new_key(self):
        config = (SRC / "hardware/cellular/backends/config.rs").read_text()
        self.assertRegex(config, r"#\[default\]\s*Modemmanager")
        self.assertIn("native_backend_requires_explicit_unvalidated_opt_in", config)
        main_config = (SRC / "platform/config.rs").read_text()
        self.assertIn("BackendConfig::is_default", main_config)

    def test_unknown_native_targets_do_not_fall_back_to_mm(self):
        text = (SRC / "hardware/cellular/control.rs").read_text()
        start = text.index("fn route(")
        end = text.index("\n}", start)
        route = text[start:end]
        self.assertIn("return fleet.device(selector).map(Some)", route)
        self.assertIn("native_backend_not_selected", route)
        self.assertNotIn("mm::", route)

    def test_sim_auth_and_packet_control_share_the_device_lease(self):
        text = (SRC / "hardware/cellular/backends/sim.rs").read_text()
        self.assertIn("blocking_lock_owned()", text)
        self.assertIn("verify_owner(path)", text)
        sim = (SRC / "connectivity/modems/ims/vowifi/qmi_uim.rs").read_text()
        self.assertGreaterEqual(sim.count("SimLease::for_endpoint("), 5)

    def test_no_mm_getter_is_hidden_in_native_sms_polling(self):
        text = (SRC / "services/messaging/sms_listener.rs").read_text()
        native = text.index("if let Some(fleet) = crate::hardware::cellular::backends::active_native()")
        legacy = text.index('info!("Starting SMS listener (ModemManager mode)")')
        self.assertLess(native, legacy)
        self.assertNotIn("Proxy::new", text[native:legacy])
        self.assertIn("BackendMode::Modemmanager", (SRC / "hardware/cellular/backends/config.rs").read_text())

    def test_migrated_at_consumers_use_the_control_facade(self):
        for path in ("hardware/cellular/cgcontrdp.rs", "connectivity/modems/ims/cellular_ims/pcscf.rs"):
            text = (SRC / path).read_text()
            self.assertFalse('Command::new("mmcli")' in text, path)
            self.assertIn("control::at_command(", text)

    def test_native_sms_initialization_is_admitted_before_touching_storage(self):
        text = (SRC / "services/messaging/sms_listener.rs").read_text()
        gate = text[text.index("async fn maybe_scan_sms_paths("):text.index("async fn scan_all_modems_or_rebind(")]
        self.assertLess(gate.index("modem_sms_scan_allowed("), gate.index("initialize_sms().await"))
        self.assertLess(gate.index("sms_reception_enabled"), gate.index("initialize_sms().await"))
        cleanup = text[text.index("fn schedule_sms_delete("):text.index("struct SmsIngestContext")]
        self.assertLess(cleanup.index("get_line_profile(&line_id).enabled"), cleanup.index("delete_message("))
        config = (SRC / "hardware/cellular/backends/config.rs").read_text()
        self.assertRegex(config, r"#\[serde\(default\)\]\s*pub sms_reception_enabled: bool")

    def test_all_native_regression_groups_are_run_in_ci_without_hardware(self):
        for workflow in ("build-release.yml", "beta-validation.yml"):
            text = (ROOT / ".github/workflows" / workflow).read_text()
            for group in ("config", "io", "qmi_proxy", "protocol", "native", "messages", "sim", "management", "bearer"):
                self.assertIn(f"hardware::cellular::backends::{group}::tests \\", text)
            self.assertIn("hardware::cellular::modem_manager::roaming_observation_tests \\", text)
            self.assertIn("services::messaging::sms_listener::tests \\", text)
            self.assertIn("http_router_tests::backend_selection_", text)
            for group in ("services::ue_worker::tests", "services::ue_netcfg::tests",
                          "connectivity::modems::ims::cellular_ims::identity::tests",
                          "connectivity::modems::ims::cellular_ims::bearer::tests"):
                self.assertIn(group, text)

    def test_native_data_uses_the_original_worker_binding_and_checks_route_outcome(self):
        text = (SRC / "hardware/cellular/backends/bearer.rs").read_text()
        start = text[text.index("impl CellularDataTransport for NativeDataTransport"):text.index("#[cfg(test)]\nmod tests")]
        self.assertIn("session.worker_binding()", start)
        self.assertIn("configure_data_bearer_network_in_worker", start)
        self.assertIn("apply_data_routes(binding", start)
        helper = text[text.index("async fn apply_data_routes("):text.index("impl CellularDataTransport for NativeDataTransport")]
        self.assertIn("if !outcome.ok", helper)

    def test_native_namespace_receipt_requires_explicit_verified_restore(self):
        text = (SRC / "hardware/cellular/backends/bearer.rs").read_text()
        cleanup = text[text.index("async fn cleanup_locked("):text.index("async fn release(mut self)")]
        self.assertLess(cleanup.index("clean &= self.namespace.is_empty()"), cleanup.index("clear_receipt("))
        confirmation = text[text.index("fn confirm_namespace_restore<'a>("):]
        confirmation = confirmation[:confirmation.index("\n    fn release(")]
        self.assertLess(confirmation.index("verify_bearer("), confirmation.index("std::mem::take(&mut session.namespace)"))
        self.assertIn("session.namespace = previous", confirmation)
        self.assertIn("namespace_receipt_is_cleared_only_after_verified_restore", text)
        self.assertIn("dropped_handle_retains_unconfirmed_namespace_ownership", text)

    def test_worker_requests_use_cancellation_cleanup_guards(self):
        text = (SRC / "services/ue_worker.rs").read_text()
        self.assertIn("impl Drop for PendingRequestGuard<'_>", text)
        self.assertEqual(text.count("let _request = PendingRequestGuard {"), 2)
        self.assertIn("cancelled_net_config_retires_its_pending_entry_without_a_reply", text)
        self.assertIn("cancelled_socket_create_retires_its_pending_entry_without_a_reply", text)

    def test_native_qmi_leases_precede_commands_and_do_not_reconnect_old_cids(self):
        text = (SRC / "hardware/cellular/backends/io.rs").read_text()
        claim = text[text.index("pub async fn claim("):text.index("async fn verify(&self")]
        self.assertLess(claim.index("verify_receipts_clear("), claim.index("open_qmi_proxy_lease("))
        verify = text[text.index("async fn verify(&self"):text.index("fn verify_receipts_clear(")]
        self.assertIn("lease.verify_alive()?", verify)
        self.assertNotIn("open_qmi_proxy_lease(", verify)
        self.assertIn("qmi_proxy_leases,", claim)

    def test_package_and_services_do_not_unconditionally_start_mm(self):
        for name in ("simadmin.service", "simadmin-loopback.service"):
            text = (ROOT / "scripts" / name).read_text()
            self.assertFalse(any("ModemManager.service" in line for line in text.splitlines() if line.startswith("Wants=")))
        text = (ROOT / "install_latest.sh").read_text()
        self.assertIn('MODEM_BACKEND="$("${INSTALL_DIR}/simadmin" modem-backend-mode)"', text)
        self.assertIn('$mm_debian libqmi-utils', text)


if __name__ == "__main__":
    unittest.main()
