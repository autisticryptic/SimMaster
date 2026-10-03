"""Guard automatic boot retirement's placement and metadata-only proof boundary."""
from pathlib import Path
import unittest
ROOT=Path(__file__).resolve().parents[2]
BASE=ROOT/'backend/src/hardware/devices/qcm410'

class RebootAbsenceTests(unittest.TestCase):
    def test_boot_recovery_precedes_new_namespace_creation(self):
        text=(ROOT/'backend/src/main.rs').read_text()
        self.assertLess(text.index('hardware::devices::recover_owned_ims_sessions().await'),text.index('match line_registry.refresh().await'))

    def test_every_refresh_keeps_prior_boot_proof_ahead_of_provisioning(self):
        main=(ROOT/'backend/src/main.rs').read_text()
        gate=main.index('if hardware::devices::requires_pre_namespace_ims_recovery()')
        self.assertLess(gate, main.index('line_registry.defer_ims_startup_recovery().await'))
        self.assertLess(gate, main.index('match line_registry.refresh().await'))
        registry=(ROOT/'backend/src/services/line_registry.rs').read_text()
        refresh=registry[registry.index('pub async fn refresh(&self)'):]
        self.assertLess(refresh.index('.ensure_ready(devices::recover_owned_ims_sessions)'), refresh.index('self.observations.discover().await'))
        self.assertIn('.map_err(|reason| ObservationError::Unavailable(reason.into()))?', refresh)
        for workflow in ('beta-validation.yml', 'build-release.yml'):
            text=(ROOT/'.github/workflows'/workflow).read_text()
            self.assertIn('services::line_registry::ims_startup_gate::tests', text)
            self.assertIn('hardware::sim::esim::tests', text)

    def test_archive_resume_never_overwrites_evidence(self):
        text=(BASE/'primary_ims_profile_retirement.rs').read_text()
        body=text[text.index('fn archive_source('):text.index('pub(super) async fn run(')]
        self.assertIn('std::io::ErrorKind::AlreadyExists', body)
        self.assertIn('open_validated_source(&archive)?', body)
        self.assertIn('if bytes != source', body)
        self.assertLess(body.index('fs::File::open(directory)'), body.index('fs::remove_file(file)'))
        self.assertNotIn('.truncate(true)', body)

    def test_old_boot_path_never_uses_same_owner_profile_cleanup(self):
        text=(BASE/'primary_ims_profile_runtime.rs').read_text()
        recover=text[text.index('async fn recover_inner('):]
        self.assertLess(recover.index('owner.boot_id != boot'),recover.index('recovery_admitted('))
        branch=recover[recover.index('if owner.boot_id != boot'):recover.index('recovery_admitted(')]
        self.assertIn('reboot_absence_io',branch)
        self.assertIn('retirement::retire_reboot_absence',branch)
        self.assertNotIn('release_with',branch)

    def test_reboot_archive_retains_double_absence_and_unchanged_sim_checks(self):
        text=(BASE/'primary_ims_profile_retirement.rs').read_text()
        body=text[text.index('pub(super) async fn retire_reboot_absence('):text.index('fn read_source(')]
        self.assertEqual(body.count('observe_absence(io, receipt).await?'),2)
        self.assertIn('verify_snapshot_pair(&first, &second)?',body)
        self.assertIn('old_owner_absent(io, receipt).await?',body)
        self.assertIn('archive_source(&store.file, &source)?',body)
        self.assertIn('require_stopped()?',body)
        self.assertIn('no_bearer_work()?',body)
        guard=text[text.index('fn validate_reboot_absence('):text.index('pub(super) async fn retire_reboot_absence(')]
        self.assertIn('owner.boot_id == boot',guard)
        self.assertIn('stable_sim_fingerprint != current.stable_sim_fingerprint',guard)
        self.assertIn('validate_absent_profile(',guard)
        for forbidden in ('release_with(', 'recover_owned(', 'Command::', '.delete('):self.assertNotIn(forbidden,body)

if __name__=='__main__':unittest.main()
