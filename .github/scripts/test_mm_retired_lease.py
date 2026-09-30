"""Static safety wiring for same-owner retired MM object recovery."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
DEVICE = ROOT / "backend/src/hardware/devices/qcm410"


def section(text, start, end):
    return text.split(start, 1)[1].split(end, 1)[0]


class RetiredLeaseTests(unittest.TestCase):
    def test_retirement_uses_object_manager_not_unknown_method_text(self):
        text = (DEVICE / "primary_ims_lifecycle.rs").read_text(encoding="utf-8")
        block = section(text, "async fn recorded_objects_retired(", "async fn may_clean_interface(")
        for expected in ("same_generation(", "self.owner_is_current().await?", "self.proxy(",
                         '"org.freedesktop.DBus.ObjectManager"', '"GetManagedObjects"',
                         "path.as_str() == record.modem", "path.as_str() == record.bearer"):
            self.assertIn(expected, block)
        errors = section(text, "fn bus_error(", "async fn timed")
        self.assertNotIn('"unknownmethod"', errors.lower())

    def test_network_retirement_is_read_only_and_checks_namespace(self):
        text = (DEVICE / "primary_ims_lifecycle.rs").read_text(encoding="utf-8")
        block = section(text, "async fn verify_retired_network_readonly(", "fn verify_retired_namespace_snapshot(")
        for required in ("verify_mm_data_interface(", "verify_teardown_readonly(",
                         'fs::symlink_metadata(', "verify_retired_namespace_snapshot(",
                         '"address", "show"', '"route", "show"', '"rule", "show"'):
            self.assertIn(required, block)
        for forbidden in ("move_iface", "deconfigure(", '"delete"', '"flush"', "remove_record("):
            self.assertNotIn(forbidden, block)
        netdev = (DEVICE / "netdev.rs").read_text(encoding="utf-8")
        readonly = section(netdev, "pub(super) async fn verify_teardown_readonly(", "async fn read_ip_json(")
        self.assertNotIn("deconfigure(", readonly)
        self.assertIn("verify_teardown(", readonly)

    def test_retirement_requires_observations_on_both_sides_of_network_verification(self):
        text = (DEVICE / "primary_ims_lifecycle.rs").read_text(encoding="utf-8")
        block = section(text, "async fn retired_cleanup_verified_with", "async fn verify_retired_network_readonly")
        self.assertEqual(block.count("objects_retired().await?"), 2)
        first = block.index("objects_retired().await?")
        network = block.index("network_absent().await?")
        last = block.rindex("objects_retired().await?")
        self.assertLess(first, network)
        self.assertLess(network, last)
        finish = section(text, "async fn finish_failed_cleanup(", "fn forget(")
        self.assertIn("Ok(true) =>", finish)
        self.assertIn("self.forget(record)?", finish)
        self.assertIn("Ok(false) => Err(original)", finish)

    def test_both_ci_workflows_keep_real_dbus_and_network_regressions(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            for filter in ("hardware::devices::qcm410::primary_ims_lifecycle::tests",
                           "hardware::devices::qcm410::primary_ims_lifecycle::ip_config_dbus_tests",
                           "hardware::devices::qcm410::netdev::tests"):
                self.assertIn(filter, text)
            self.assertIn("dbus-run-session", text)


if __name__ == "__main__":
    unittest.main()
