"""Structural guards supplement, but do not replace, Rust regressions on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "backend/src/connectivity/modems/ims"


class ImsFallbackBoundaryTests(unittest.TestCase):
    def test_runtime_uses_slot_bound_aid_before_sim_identity_fallback(self):
        text = (SRC / "cellular_ims/live.rs").read_text()
        load = text[text.index("async fn load_device_identity("):text.index("async fn load_uicc_applications(")]
        self.assertLess(load.index("load_uicc_applications(device)"), load.index("resolve_fallback_imsi("))
        self.assertIn("parse_uicc_applications_for_slot(&output, device.uim_slot)", text)
        self.assertIn("read_uim_identity(device, &aka_aid)", load)
        self.assertIn("identity::read_mnc_length_via_at", load)
        self.assertIn("control::at_command(&device.modem_id, command)", load)

    def test_profile_definition_is_checked_before_any_cgdcont_write(self):
        text = (SRC / "cellular_ims/pcscf.rs").read_text()
        prepare = text[text.index("pub async fn prepare_ims_profile_context("):text.index("fn select_ims_profile_context(")]
        self.assertLess(prepare.index("select_ims_profile_context("), prepare.index('AT+CGDCONT='))
        self.assertLess(prepare.index("ensure_profile_inactive("), prepare.index('AT+CGDCONT='))
        # The codes themselves live in the central `errors::code` table; the
        # call site must reference them rather than re-spelling the literal.
        self.assertIn("code::IMS_PREFERRED_PROFILE_OCCUPIED", text)
        self.assertIn("code::IMS_PROFILE_DEFINITION_AMBIGUOUS", text)
        errors = (SRC / "cellular_ims/errors.rs").read_text()
        self.assertIn('"cellular_ims_preferred_profile_occupied"', errors)
        self.assertIn('"cellular_ims_profile_definition_ambiguous"', errors)

    def test_both_automatic_resolvers_use_the_same_home_boundary(self):
        text = (SRC / "vowifi/profile_store.rs").read_text()
        self.assertEqual(text.count("automatic_home_plmn_hint("), 3)
        self.assertNotIn("cmp(&left.1.meta.plmn.len())", text)
        self.assertIn("automatic_sources_refuse_conflicting_custom_and_catalog_home_boundaries", text)

    def test_pcscf_dns_validation_remains_on_the_ue_socket_path(self):
        text = (SRC / "cellular_ims/pcscf.rs").read_text()
        query = text[text.index("async fn query_dns("):text.index("fn dns_query_id(")]
        self.assertIn("worker.create_socket(spec)", query)
        self.assertIn("parse_dns_response(query_id, name, record_type, &response[..read])", query)
        self.assertNotIn("lookup_host", query)
        parser = (SRC / "cellular_ims/pcscf_dns.rs").read_text()
        self.assertIn("section == 0 && class == IN", parser)
        self.assertIn("answer.owner == terminal", parser)
        self.assertIn("MAX_CNAME_HOPS", parser)

    def test_active_pcscf_wait_is_read_only_and_has_a_total_budget(self):
        text = (SRC / "cellular_ims/pcscf.rs").read_text()
        active = text[text.index("pub async fn discover_pcscf_via_active_at_context("):text.index("async fn run_at(")]
        self.assertIn("tokio::time::timeout(", active)
        self.assertIn("ACTIVE_PCSCF_READ_BUDGET", active)
        self.assertIn("0..ACTIVE_PCSCF_READ_ROUNDS", active)
        self.assertIn("at_active_ims_context_changed", active)
        for command in ("AT+CGACT=", "AT+CGDCONT=", "AT$QCPDPIMSCFGE="):
            self.assertNotIn(command, active)

    def test_mm_bearer_ip_config_uses_typed_get_all_and_unique_owner_checks(self):
        device = ROOT / "backend/src/hardware/devices/qcm410"
        lifecycle = (device / "primary_ims_lifecycle.rs").read_text()
        read = lifecycle[lifecycle.index("pub async fn ip_settings("):lifecycle.index("pub async fn connect(")]
        self.assertIn('.call("GetAll", &(BEARER,))', read)
        self.assertEqual(read.count("owner_is_current().await?"), 2)
        self.assertIn("primary_ims_settings::validate_binding(", read)
        self.assertIn("CacheProperties::No", lifecycle)
        session = (device / "primary_ims_session.rs").read_text()
        read = session[session.index("pub async fn read_ip_settings("):session.index("pub async fn stop(")]
        self.assertEqual(read.count("self.check_liveness()?"), 2)
        self.assertIn("owned(&self.bearer)", read)

    def test_qca410_ip_and_dns_do_not_fall_back_to_guessed_at_addressing(self):
        device = ROOT / "backend/src/hardware/devices/qcm410"
        driver = (device / "ims_bearer.rs").read_text()
        self.assertIn("session.read_ip_settings().await?", driver)
        self.assertNotIn("read_cgcontrdp_settings(", driver)
        self.assertIn('settings_source = "modemmanager_bearer_ip_config"', driver)
        parser = (device / "primary_ims_settings.rs").read_text()
        for field in ('"Ip4Config"', '"Ip6Config"', '"dns1"', '"dns2"', '"dns3"'):
            self.assertIn(field, parser)
        self.assertIn("qca410_primary_mm_ip_method_unsupported", parser)

    def test_active_at_pcscf_is_bound_to_the_bearer_source_address(self):
        text = (SRC / "cellular_ims/pcscf.rs").read_text()
        active = text[text.index("async fn discover_active_pcscf_with<"):text.index("async fn run_at(")]
        self.assertIn("bearer_local_addresses.contains(&local)", active)
        self.assertIn("observed.ipv4_address", active)
        self.assertIn("observed.ipv6_address", active)
        self.assertIn("at_active_ims_bearer_address_missing", active)
        self.assertIn("active_pcscf_does_not_borrow_another_same_apn_bearer_address", text)

    def test_network_forced_family_retry_survives_and_only_skips_a_repeat(self):
        text = (SRC / "cellular_ims/native_bearer.rs").read_text()
        establish = text[text.index("pub async fn establish_native_ims_bearer("):text.index("fn forced_family_needs_another_attempt(")]
        # The single-family retry is what a v4-only/v6-only network needs and
        # what the validated IPv4 line registration used. It must remain.
        self.assertIn("forced_single = Some(forced)", establish)
        self.assertIn("&[forced],", establish)
        self.assertIn("if let [single] = attempt_families", establish)
        self.assertIn("forced_family_needs_another_attempt(&attempted_single, forced)", establish)
        # A pinned profile alone must not cancel the retry, and the original
        # bearer error must not be replaced by a synthetic pin-conflict code.
        self.assertNotIn("pinned_profile_forced_family_error", text)
        self.assertNotIn("profile_pin_family_conflict", text)
        self.assertIn("fn forced_family_needs_another_attempt", text)
        self.assertIn("network_forced_family_keeps_its_retry_unless_already_attempted", text)

    def test_mm_property_regressions_are_executed_on_actions(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            self.assertIn("primary_ims_settings::tests", text)
            private_bus = text[text.index("dbus-run-session"):]
            self.assertIn("primary_ims_lifecycle::ip_config_dbus_tests", private_bus)

    def test_retained_pcscf_parser_and_serial_regressions_are_executed_on_actions(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            self.assertIn("hardware::devices::qcm410::primary_ims_pcscf::tests", text)
            self.assertIn("hardware::cellular::serial::tests", text)
            self.assertIn("primary_ims_lifecycle::ip_config_dbus_tests", text)

    def test_mm_pcscf_prefix_association_remains_provider_scoped_and_pinned(self):
        device = ROOT / "backend/src/hardware/devices/qcm410"
        text = (device / "primary_ims_pcscf.rs").read_text()
        association = text[text.index("fn association("):text.index("pub(super) async fn discover_with<")]
        for gate in ("profile_id != Some(u32::from(cid))", "state.active.as_slice() != [cid]",
                     "expected.ipv6_prefix != Some(64)", "at[..8] == mm[..8]"):
            self.assertIn(gate, association)
        self.assertIn("if read().await? != *expected", text)
        self.assertIn("context_rows(&at(command).await?, cid, apn)? != rows", text)
        for write in ("AT+CGACT=", "AT+CGDCONT=", "AT$QCPDPIMSCFGE=", "qmicli"):
            self.assertNotIn(write, text)
        driver = (device / "ims_bearer.rs").read_text()
        self.assertIn(".discover_pcscf(&self.expected_settings)", driver)
        self.assertIn("let expected_settings = settings.clone()", driver)

    def test_pcscf_rpc_cancellation_keeps_shared_serial_and_lease_guards(self):
        device = ROOT / "backend/src/hardware/devices/qcm410"
        lifecycle = (device / "primary_ims_lifecycle.rs").read_text()
        task = lifecycle[lifecycle.index("async fn retained_serial_read<"):lifecycle.index("fn create_properties<")]
        self.assertIn("tokio::spawn(async move", task)
        self.assertIn("let _lease = guard", task)
        self.assertIn("serial::acquire_for(&modem)", task)
        self.assertIn("timeout_at(deadline", task)
        self.assertIn("read.await", task)
        self.assertIn("at_read_timeout_unverified", lifecycle)
        self.assertIn("pcscf_publication_timeout_drains_one_dispatched_read_without_publishing_it", lifecycle)
        serial = (ROOT / "backend/src/hardware/cellular/serial.rs").read_text()
        self.assertIn("let _guard = acquire_for(resource_key).await", serial)

    def test_provider_failure_does_not_reenter_legacy_at_and_checks_after_each_await(self):
        text = (SRC / "cellular_ims/native_bearer.rs").read_text()
        method = text[text.index("pub async fn discover_pcscf<"):text.index("pub async fn move_into_worker(")]
        self.assertIn("Ok(None) => exact_at_fallback().await", method)
        self.assertIn("Err(error) => Err(cellular_ims_error_from_ims_bearer(error))", method)
        self.assertEqual(method.count("self.check_liveness()?"), 3)
        self.assertEqual(method.count("self.worker_binding_is_current()"), 3)
        self.assertIn("provider_success_missing_and_lost_results_never_retry_legacy_at", text)

    def test_known_invalid_pcscf_binding_never_uses_reusable_selector_cleanup(self):
        text = (SRC / "cellular_ims/live.rs").read_text()
        cleanup = text[text.index("async fn cleanup_unverified_native_bearer("):text.index("async fn cleanup_pending_native_bearer(")]
        self.assertIn("release_unverified_native_ims_bearer(native).await", cleanup)
        strategy = (SRC / "cellular_ims/native_bearer.rs").read_text()
        provider_only = strategy[strategy.index("async fn release_unverified_native_ims_bearer("):strategy.index("pub async fn release_native_ims_bearer(")]
        self.assertIn("bearer.handle.release().await", provider_only)
        self.assertNotIn("teardown_bearer_network_in_worker(", provider_only)
        self.assertNotIn("restore_from_worker(", provider_only)
        self.assertNotIn("disable_pcscf_reporting(", cleanup)
        self.assertNotIn("cleanup_ims_profile_lease(", cleanup)
        self.assertEqual(text.count("cleanup_unverified_native_bearer(&mut native_bearer).await"), 2)
        self.assertIn("Err(error) if !pcscf_observation_allows_fallback(&error)", text)
        policy = text[text.index("fn pcscf_observation_allows_fallback("):text.index("async fn cleanup_unverified_native_bearer(")]
        self.assertIn("error.code() == code::RUNTIME_ALL_PCSCF_FAILED", policy)

    def test_mm_dual_request_and_actual_grant_are_not_reduced_to_the_first_family(self):
        device = ROOT / "backend/src/hardware/devices/qcm410"
        driver = (device / "ims_bearer.rs").read_text()
        self.assertIn("MmIpFamily::from_requested(families)", driver)
        self.assertNotIn("families.first()", driver)
        self.assertIn("family: requested_family", driver)
        self.assertIn("ip_type: granted_family.as_str().to_string()", driver)
        self.assertIn("requested_ip_type = requested_family.as_str()", driver)
        self.assertIn("granted_ip_type = granted_family.as_str()", driver)
        lifecycle = (device / "primary_ims_lifecycle.rs").read_text()
        self.assertIn("let family = request.family.flags()", lifecycle)
        parser = (device / "primary_ims_settings.rs").read_text()
        self.assertIn("Self::Ipv4v6 => 4", parser)
        self.assertIn("dual_does_not_hide_unsupported_or_untyped_companion_configuration", parser)

    def test_mm_network_batch_is_fully_recorded_before_configuration_and_shielded(self):
        device = ROOT / "backend/src/hardware/devices/qcm410"
        driver = (device / "ims_bearer.rs").read_text()
        self.assertLess(driver.index("session.network_will_be_configured(&networks)"),
                        driver.index("configure_primary_networks(networks, network_guard,"))
        configure = driver[driver.index("async fn configure_primary_networks<"):driver.index("struct GrantedSettings")]
        self.assertIn("tokio::spawn(async move", configure)
        self.assertIn("let _guard = guard", configure)
        self.assertIn("for network in networks", configure)
        self.assertIn("cancelling_second_family_keeps_the_lease_guard_until_io_finishes", driver)

    def test_mm_dual_receipts_reject_plan_shrink_and_verify_every_cleanup(self):
        device = ROOT / "backend/src/hardware/devices/qcm410"
        lifecycle = (device / "primary_ims_lifecycle.rs").read_text()
        self.assertIn("qca410_primary_mm_lease_network_change_refused", lifecycle)
        self.assertIn("legacy_v1_json_without_additional_networks_remains_readable", lifecycle)
        self.assertIn("netdev::teardown_verified(&interface, &network).await", lifecycle)
        cleanup = lifecycle[lifecycle.index("async fn cleanup_networks_with<"):lifecycle.index("pub(super) fn cleanup_in_background")]
        self.assertIn("for network in record.networks()", cleanup)
        self.assertIn("first_error.get_or_insert(error)", cleanup)
        netdev = (device / "netdev.rs").read_text()
        for failure in ("address_remaining", "routes_remaining", "rule_remaining"):
            self.assertIn("qca410_primary_ims_cleanup_" + failure, netdev)

    def test_mm_cleanup_observes_the_link_without_an_address_family_filter(self):
        netdev = (ROOT / "backend/src/hardware/devices/qcm410/netdev.rs").read_text()
        self.assertIn('read_ip_json(&["-j", "-N", "address", "show", "dev", interface])', netdev)
        self.assertNotIn('family, "address", "show", "dev", interface', netdev)
        self.assertIn("unfiltered_cleanup_snapshot_can_contain_only_the_other_family", netdev)
        self.assertIn("timed_out_ip_command_is_killed_and_reaped_before_returning", netdev)

    def test_dual_cleanup_regressions_are_selected_in_both_workflows(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            for module in ("ims_bearer", "netdev", "primary_ims_session", "primary_ims_lifecycle", "primary_ims_settings"):
                self.assertIn("hardware::devices::qcm410::" + module + "::tests", text)

    def test_srv_ports_survive_into_the_live_udp_channel(self):
        text = (SRC / "cellular_ims/live.rs").read_text()
        family = text[text.index("async fn connect_family("):text.index("async fn", text.index("async fn connect_family(") + 10)]
        self.assertIn("pcscf: SocketAddr", family)
        self.assertIn("pcscf_addr: pcscf", family)
        self.assertNotIn("pcscf_socket(pcscf)", family)
        discovery = (SRC / "cellular_ims/pcscf.rs").read_text()
        self.assertIn("target.endpoint(address)", discovery)
        self.assertNotIn('format!("_sip._tcp.', discovery)

    def test_ci_selects_the_fallback_and_end_to_end_batch_regressions(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / name).read_text()
            for group in ("cellular_ims::identity::tests", "cellular_ims::plan::tests",
                          "cellular_ims::native_bearer::tests", "cellular_ims::pcscf::tests",
                          "cellular_ims::pcscf_dns::tests", "vowifi::profile_store::tests", "vowifi::profile_record::tests",
                          "api::handlers::tests::cellular_ims_profile_batch"):
                self.assertIn(group, text)
        package = (ROOT / "frontend/package.json").read_text()
        self.assertIn("tests/cellularImsErrorFormat.test.ts", package)


if __name__ == "__main__":
    unittest.main()
