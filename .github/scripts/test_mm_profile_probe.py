"""Guards for the explicit bounded REGISTER probe, not production fallback."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
QCM = ROOT / "backend/src/hardware/devices/qcm410"
LIVE = ROOT / "backend/src/connectivity/modems/ims/cellular_ims/live.rs"


def between(text, start, end):
    return text.split(start, 1)[1].split(end, 1)[0]


class ProfileProbeTests(unittest.TestCase):
    def test_probe_reuses_register_core_without_server_or_listener(self):
        text = LIVE.read_text(encoding="utf-8")
        block = between(text, "pub(crate) async fn probe_owned_profile_once(", "fn failure_stage(")
        for required in ("connect_inner(", "Duration::from_secs(240)", "runtime.observe_mm_binding(&binding)",
                         "runtime.for_generation(generation)", "probe.binding().await?", "probe.serving_access(&binding)",
                         'PathBuf::from(":memory:")', "ImsProfileSource::Derived", "SimOverride::default()"):
            self.assertIn(required, block)
        for forbidden in ("start_live_listener(", "NotificationSender::", "ConfigManager::", "start_line_cellular_ims_restore", "ensure_modemmanager_debug_override"):
            self.assertNotIn(forbidden, block)
        self.assertIn('"registered":true', block)
        self.assertIn("Ok(Ok(session))", block)

    def test_probe_strict_profile_does_not_run_apn_only_preparation(self):
        text = LIVE.read_text(encoding="utf-8")
        block = between(text, "let diagnostic_context =", "let mut request = BearerRequest::")
        self.assertIn("probe.verify().await", "".join(block.split()))
        self.assertIn("is_standard_derived_profile", block)
        self.assertIn("probe.accepts_endpoint", block)
        self.assertIn("cid:probe.cid()", block.replace(" ", ""))
        self.assertIn("diagnostic_context", block)
        self.assertIn("prepare_ims_profile_context", block)
        ordinary = between(text, "pub async fn connect_live_for_line(", "pub(crate) async fn probe_owned_profile_once(")
        self.assertIn('None,', ordinary[ordinary.index("match connect_inner("):])

    def test_probe_transport_blocks_second_or_wrong_family_activation(self):
        text = (QCM / "primary_ims_profile_lease.rs").read_text(encoding="utf-8")
        block = between(text, "impl crate::hardware::devices::transport::ImsBearerTransport for VerifiedProfileProbe", "fn probe_family_matches(")
        for required in ("probe_family_matches(", "self.used.swap(true", "expected_mm_sim.is_none()",
                         "profile_id != Some(u32::from(self.cid()))", "self.verify()"):
            self.assertIn(required, block)
        self.assertIn("profile_probe_request_changed_or_repeated", block)
        self.assertIn("Phase::Probing", text)
        self.assertIn("Phase::Probed", text)

    def test_probe_cleans_bearer_before_worker_and_only_its_own_namespace(self):
        text = LIVE.read_text(encoding="utf-8")
        block = between(text, "pub(crate) async fn probe_owned_profile_once(", "fn failure_stage(")
        self.assertLess(block.index("cleanup_live_session(&live)"), block.index("probe.drain_bearers()"))
        self.assertLess(block.index("probe.drain_bearers()"), block.index("worker.shutdown()"))
        self.assertLess(block.index("worker.shutdown()"), block.index("netns::remove("))
        self.assertIn('name == "lo"', block)
        self.assertIn('"bearer_cleanup_verified"', block)
        self.assertNotIn("reclaim_stranded", block)

    def test_probe_carries_original_owner_through_actual_session_creation(self):
        text = (QCM / "primary_ims_profile_lease.rs").read_text(encoding="utf-8")
        block = between(text, "impl crate::hardware::devices::transport::ImsBearerTransport for VerifiedProfileProbe", "fn probe_family_matches(")
        self.assertIn("establish_with_bus(", block)
        self.assertIn("Some(Arc::clone(&self.bus))", block)
        session = (QCM / "primary_ims_session.rs").read_text(encoding="utf-8")
        branch = between(session, "let bus = match pinned_bus", "let controller =")
        pinned = between(branch, "Some(bus) =>", "None =>")
        self.assertIn("bus.ensure_sim_binding().await?", pinned)
        self.assertNotIn("MmBus::new", pinned)
        self.assertNotIn("pin_sim_binding", pinned)

    def test_probe_durable_cleanup_and_namespace_gate_precede_release(self):
        text = (QCM / "primary_ims_profile_lease.rs").read_text(encoding="utf-8")
        drain = between(text, "pub(crate) async fn drain_bearers", "impl crate::hardware::devices::transport")
        self.assertIn("require_no_bearer_receipts()?", drain)
        release = between(text, '"release" => {', "release_with(&io, &store, receipt).await?")
        self.assertIn("netns::exists(&namespace)", release)
        self.assertIn("mm_ims_profile_probe_namespace_remaining", release)

    def test_profile_reconciliation_never_retargets_bearer_cleanup(self):
        text = (QCM / "primary_ims_profile_lease.rs").read_text(encoding="utf-8")
        block = between(text, "fn rebind_profile_receipt(", "struct DiskStore")
        for required in ("stable_sim_fingerprint", "control_topology", "eps_fingerprint", "old.owner", "old.bus_id", "owned_matches("):
            self.assertIn(required, block)
        self.assertEqual(block.count("original_modem_absent(io, &original).await?"), 2)
        for forbidden in ('"Disconnect"', '"DeleteBearer"', "move_iface", "remove_record("):
            self.assertNotIn(forbidden, block)


if __name__ == "__main__":
    unittest.main()
