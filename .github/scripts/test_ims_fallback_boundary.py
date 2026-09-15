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
        self.assertIn("volte_ims_preferred_profile_occupied", text)
        self.assertIn("volte_ims_profile_definition_ambiguous", text)

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
        active = text[text.index("pub async fn discover_pcscf_via_active_at_context("):text.index("/// Read the full IP configuration")]
        self.assertIn("tokio::time::timeout(", active)
        self.assertIn("ACTIVE_PCSCF_READ_BUDGET", active)
        self.assertIn("0..ACTIVE_PCSCF_READ_ROUNDS", active)
        self.assertIn("at_active_ims_context_changed", active)
        for command in ("AT+CGACT=", "AT+CGDCONT=", "AT$QCPDPIMSCFGE="):
            self.assertNotIn(command, active)

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
