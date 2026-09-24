"""Recovery is explicit bookkeeping, never proof from a reused modem ID."""
from pathlib import Path
import re
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "backend/src"
BACKENDS = SRC / "hardware/cellular/backends"


class NativeRecoveryBoundaryTests(unittest.TestCase):
    def test_recovery_cli_returns_before_normal_backend_and_namespace_startup(self):
        source = (SRC / "main.rs").read_text()
        body = source.split("if let Some(CliCommand::NativeRecovery", 1)[1].split("if let Some(CliCommand::DjiPrepare", 1)[0]
        for required in ("recovery::inventory()", "recovery::plan", "recovery::apply", "return Ok(())"):
            self.assertIn(required, body)
        for forbidden in ("backends::initialize", "reclaim_all_stranded", 'Command::new("systemctl")'):
            self.assertNotIn(forbidden, body)
        self.assertRegex(source, r"if using_mm\s*\{\s*hardware::devices::recover_owned_ims_sessions\(\).await;\s*platform::netns::reclaim_all_stranded_hardware_links\(\).await;")

    def test_archive_never_executes_historical_cids_or_forces_a_service_takeover(self):
        source = (BACKENDS / "recovery.rs").read_text()
        runtime = source.split("#[cfg(test)]", 1)[0]
        for forbidden in ("--client-cid", "--wds-stop-network", "AT+", "run_process(", "tokio::process::Command"):
            self.assertNotIn(forbidden, runtime)
        body = runtime.split("pub async fn apply(", 1)[1]
        self.assertLess(body.index("recovery_locks"), body.index("archive_exact"))
        self.assertLess(body.index("verify_manager_absent"), body.index("archive_exact"))
        self.assertIn("latest.public.revision != revision", body)
        self.assertIn("unconfirmed_resources_require_device_specific_review", runtime)
        self.assertIn("original_owner_still_running", runtime)

    def test_receipts_survive_boot_and_all_scopes_block_before_proxy_open(self):
        source = (BACKENDS / "recovery.rs").read_text()
        self.assertIn('"/var/lib/simadmin/native-control"', source)
        self.assertIn('starts_with("session-")', source)
        self.assertIn("sync_directory(directory)", source)
        clear = source.split("pub(super) fn clear_owned(", 1)[1].split("pub(super) fn pending_paths(", 1)[0]
        self.assertLess(clear.index("cleanup_confirmed = true"), clear.index("std::fs::remove_file"))
        io = (BACKENDS / "io.rs").read_text()
        claim = io.split("pub async fn claim(", 1)[1].split("async fn verify(&self", 1)[0]
        self.assertLess(claim.index("ensure_persistent_clear"), claim.index("open_qmi_proxy_lease"))
        self.assertIn("ensure_persistent_clear", (BACKENDS / "mod.rs").read_text())

    def test_qmi_ledger_begins_before_ctl_allocation_and_closes_after_release(self):
        source = (SRC / "connectivity/modems/ims/vowifi/qmi_uim.rs").read_text()
        allocate = source.split("fn allocate_uim_cid(", 1)[1].split("fn release_uim_cid(", 1)[0]
        self.assertLess(allocate.index("ChannelLease::begin"), allocate.index("build_allocate_uim_cid_frame"))
        release = source.split("fn release_uim_cid(", 1)[1].split("fn resolve_application_aid(", 1)[0]
        self.assertLess(release.index("ensure_success"), release.index("receipt.closed"))
        close = source.split("fn close_logical_channel(", 1)[1].split("fn send_apdu(", 1)[0]
        self.assertIn("channel_closed_retaining_client", close)
        self.assertNotIn("native_channel.take()", close)

    def test_generic_reset_cannot_skip_generation_bound_maintenance(self):
        source = (BACKENDS / "native.rs").read_text()
        body = source.split("pub async fn reset(", 1)[1].split("pub(super) async fn verify_primary_slot", 1)[0]
        self.assertIn("native_reset_requires_explicit_maintenance_plan", body)
        self.assertNotIn("self.command", body)
        self.assertIn("controller_instance", (SRC / "hardware/devices/quectel/maintenance.rs").read_text())
        dji = (SRC / "hardware/devices/dji.rs").read_text()
        self.assertIn("recovery::RECEIPT_DIRECTORY", dji)

    def test_recovery_and_native_qmi_behavior_are_run_in_both_workflows(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            source = (ROOT / ".github/workflows" / name).read_text()
            self.assertIn("hardware::cellular::backends::recovery::tests", source)
            self.assertIn("connectivity::modems::ims::vowifi::qmi_uim::native_ledger_tests", source)


if __name__ == "__main__":
    unittest.main()
