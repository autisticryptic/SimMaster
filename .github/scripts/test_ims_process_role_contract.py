"""Guard the non-manager initializer exception without executing target code."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
LEASE = ROOT / 'backend/src/hardware/devices/qcm410/primary_ims_profile_lease.rs'


class ImsProcessRoleContractTests(unittest.TestCase):
    def test_initializer_exception_is_complete_bounded_cli_not_a_name_substring(self):
        source = LEASE.read_text()
        role = source.split('pub(crate) fn is_device_initializer_process(', 1)[1].split('\n/// A process', 1)[0]
        for guard in ['"simadmin (deleted)"', 'cmdline.len() > 16_384', 'std::str::from_utf8(cmdline)',
                      'args.next() == Some("device-init")', 'args.next() == Some("")', 'args.next().is_none()']:
            self.assertIn(guard, role)
        self.assertNotIn('.contains("device-init")', role)
        self.assertNotIn('unwrap()', role)

    def test_process_scan_remains_read_only_and_bearer_gate_is_not_removed(self):
        source = LEASE.read_text()
        stopped = source.split('fn require_stopped() -> Result<(), String> {', 1)[1].split('\n}', 1)[0]
        self.assertLess(stopped.index('require_no_bearer_receipts()?'), stopped.index('require_no_competing_manager'))
        scan = source.split('fn require_no_competing_manager(', 1)[1].split('\nfn require_stopped()', 1)[0]
        self.assertIn('pid == current_pid', scan)
        self.assertIn('runs_in_ue_namespace(proc_root, pid)', scan)
        self.assertIn('mm_ims_profile_lease_other_simadmin_running', scan)
        self.assertIn('is_device_initializer_process(&exe, &cmdline)', scan)
        for forbidden in ['Command::', 'remove_record(', 'remove_file(', 'kill(', '.write(', 'clear()']:
            self.assertNotIn(forbidden, scan)

    def test_actual_cli_unit_and_new_runtime_cases_stay_bound(self):
        unit = (ROOT / 'deploy/devices/qcm410/system/simadmin-secondary-qmi.service').read_text()
        self.assertIn('ExecStart=/opt/simadmin/simadmin device-init', unit)
        main = (ROOT / 'backend/src/main.rs').read_text()
        self.assertIn('fn secondary_device_initializer_cli_is_not_a_management_server()', main)
        self.assertIn('fn actual_management_server_cli_remains_a_competing_process()', main)
        source = LEASE.read_text()
        for case in ['device_initializer_does_not_block_same_namespace_startup_recovery',
                     'initializer_old_inode_after_atomic_upgrade_remains_non_manager',
                     'true_second_manager_still_blocks_with_initializer_present',
                     'ambiguous_initializer_metadata_does_not_bypass_manager_exclusion',
                     'existing_ue_namespace_exclusion_and_self_pid_are_preserved']:
            self.assertIn('fn ' + case + '()', source)

    def test_both_actions_execute_process_and_cli_regressions(self):
        for name in ['build-release.yml', 'beta-validation.yml']:
            text = (ROOT / '.github/workflows' / name).read_text()
            self.assertIn('mm_startup_policy_tests \\\n', text)
            self.assertIn('hardware::devices::qcm410::primary_ims_lifecycle::profile_lease::tests \\\n', text)


if __name__ == '__main__':
    unittest.main()
