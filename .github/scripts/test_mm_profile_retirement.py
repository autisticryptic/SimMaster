"""Guard the explicit cross-owner ABSENT-resource retirement boundary."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
QCM = ROOT / "backend/src/hardware/devices/qcm410"


def part(source, start, end):
    return source.split(start, 1)[1].split(end, 1)[0]


class AbsentProfileRetirementTests(unittest.TestCase):
    def test_retirement_never_mutates_modem_or_recovers_unknown_resources(self):
        source = (QCM / "primary_ims_profile_retirement.rs").read_text()
        for forbidden in ('"Delete"', '"Set"', '"Connect"', '"Disconnect"',
                          '"DeleteBearer"', '"CreateBearer"', '"AT+CGDCONT=',
                          'recover_owned()', 'reclaim_', 'systemctl'):
            self.assertNotIn(forbidden, source)
        for required in ('current.profiles.contains_key(&owned.id)',
                         'current.definitions.contains_key(&owned.id)',
                         'current.reporting.get(&owned.id)', 'creator_alive',
                         'control_topology', 'verify_retired_network_readonly',
                         'name_has_owner', 'require_stopped()', 'no_bearer_work()'):
            self.assertIn(required, source)

    def test_only_explicit_retirement_may_archive_after_two_current_snapshots(self):
        source = (QCM / "primary_ims_profile_retirement.rs").read_text()
        run = source.split('pub(super) async fn run(', 1)[1]
        self.assertEqual(run.count('observe_absence(io, &receipt).await?'), 2)
        self.assertLess(run.index('verify_snapshot_pair'), run.index('archive_source('))
        self.assertIn('expected_plan != Some(plan.as_str())', run)
        self.assertLess(run.index('if action == "inspect-retired"'), run.index('archive_source('))
        runtime = (QCM / "primary_ims_profile_runtime.rs").read_text()
        production = runtime.split('pub async fn prepare(', 1)[1]
        self.assertNotIn('retirement::run', production)
        self.assertNotIn('archive_source(', production)

    def test_archive_preserves_exact_record_and_never_overwrites_evidence(self):
        source = (QCM / "primary_ims_profile_retirement.rs").read_text()
        archive = part(source, 'fn archive_source(', 'pub(super) async fn run(')
        self.assertIn('create_new(true)', archive)
        self.assertIn('output.write_all(source)', ''.join(archive.split()))
        self.assertLess(archive.index('output.sync_all()'), archive.index('fs::remove_file(file)'))
        self.assertIn('read_source(file)? != source', archive)
        self.assertIn('mm_ims_profile_retirement_archive_unconfirmed', archive)

    def test_both_suites_run_retirement_pure_and_private_bus_tests(self):
        for name in ('beta-validation.yml', 'build-release.yml'):
            text = (ROOT / '.github/workflows' / name).read_text()
            prefix = 'hardware::devices::qcm410::primary_ims_lifecycle::profile_lease::runtime::'
            self.assertIn(prefix + 'retirement::tests', text)
            self.assertIn(prefix + 'dbus_tests', text)


if __name__ == '__main__':
    unittest.main()
