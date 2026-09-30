"""Explicit temporary profile maintenance must not become silent IMS fallback."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "backend/src/hardware/devices/qcm410/primary_ims_profile_lease.rs"


def part(text, start, end):
    return text.split(start, 1)[1].split(end, 1)[0]


class TemporaryProfileLeaseTests(unittest.TestCase):
    def test_create_omits_index_and_never_modifies_existing_profile(self):
        text = SRC.read_text(encoding="utf-8")
        create = part(text, "async fn create(&self, apn: &str, family: u32, tag: &str)", "async fn restore_reporting(")
        self.assertIn('("profile-name", Value::from(tag))', create)
        self.assertIn('"Set"', create)
        self.assertNotIn('("profile-id",', create)
        self.assertNotIn("CGDCONT=", text)
        self.assertNotIn("CGACT=", text)
        self.assertNotIn("SetInitialEpsBearerSettings", text)

    def test_intent_precedes_set_and_returned_profile_is_read_back(self):
        text = SRC.read_text(encoding="utf-8")
        acquire = part(text, "async fn acquire_with", "async fn release_with")
        self.assertLess(acquire.index("store.save(&receipt)?"), acquire.index("io.create("))
        self.assertIn("owned.name != receipt.tag", acquire)
        self.assertIn("owned.family != family", acquire)
        self.assertIn("after.profiles.get(&owned.id)", acquire)
        self.assertIn("unchanged_except(", acquire)
        self.assertIn("io.snapshot().await? != after", acquire)

    def test_delete_is_owned_and_ambiguous_deletion_never_retries(self):
        text = SRC.read_text(encoding="utf-8")
        release = part(text, "async fn release_with", "fn profile(")
        self.assertLess(release.index("Phase::Deleting"), release.index("io.delete(id)"))
        self.assertIn("mm_ims_profile_lease_deletion_unresolved", release)
        self.assertIn("owned_matches(&receipt", release)
        self.assertIn("io.restore_reporting(id, original)", release)
        self.assertIn("snapshot == receipt.before", release)
        self.assertIn("/var/lib/simadmin/mm-ims-profile-lease", text)
        self.assertIn("libc::LOCK_EX | libc::LOCK_NB", text)
        self.assertIn("libc::O_NOFOLLOW", text)

    def test_cli_does_not_start_server_or_join_automatic_fallback(self):
        main = (ROOT / "backend/src/main.rs").read_text(encoding="utf-8")
        block = part(main, "if let Some(CliCommand::MmImsProfileLease", "if let Some(CliCommand::DeviceInit")
        self.assertIn("mm_ims_profile_lease::maintain", block)
        self.assertIn("return Ok(())", block)
        text = SRC.read_text(encoding="utf-8")
        for forbidden in ('"Enable"', '"Disable"', '"Connect"', '"CreateBearer"', "systemctl", "mm-pcscf-recovery"):
            self.assertNotIn(forbidden, text)
        for file in ("live.rs", "native_bearer.rs", "pcscf.rs"):
            production = (ROOT / "backend/src/connectivity/modems/ims/cellular_ims" / file).read_text(encoding="utf-8")
            self.assertNotIn("profile_lease::maintain", production)

    def test_both_ci_suites_include_pure_and_private_bus_tests(self):
        for name in ("beta-validation.yml", "build-release.yml"):
            text = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            self.assertIn("hardware::devices::qcm410::primary_ims_lifecycle::profile_lease::tests", text)
            self.assertIn("hardware::devices::qcm410::primary_ims_lifecycle::profile_lease::dbus_tests", text)
            self.assertIn("dbus-run-session", text)


if __name__ == "__main__":
    unittest.main()
