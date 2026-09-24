from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DjiMaintenanceBoundaryTests(unittest.TestCase):
    def test_default_cli_path_is_passive_and_apply_requires_two_confirmations(self):
        main = (ROOT / 'backend/src/main.rs').read_text()
        self.assertIn('dji_expected_generation_required', main)
        self.assertIn('dji_explicit_confirmation_required', main)
        source = (ROOT / 'backend/src/hardware/devices/dji.rs').read_text()
        passive = source.split('fn plan_at(', 1)[1].split('pub fn plan(', 1)[0]
        self.assertNotIn('fs::write', passive)
        self.assertNotIn('ioctl', passive)
        self.assertNotIn('Command::', passive)

    def test_ioctl_uses_the_target_libc_signature_without_checked_sign_conversion(self):
        source = (ROOT / 'backend/src/hardware/devices/dji.rs').read_text()
        self.assertIn('libc::ioctl(file.as_raw_fd(), request as _, &mut control)', source)
        self.assertNotIn('as libc::c_ulong', source)
        self.assertNotIn('request.try_into()', source)

    def test_apply_is_generation_and_owner_guarded_without_nv_or_service_changes(self):
        source = (ROOT / 'backend/src/hardware/devices/dji.rs').read_text()
        for check in ['dji_plan_stale', 'dji_qmi_interface_must_be_unbound', 'LOCK_EX',
                      'ensure_mm_handover_clear', 'dji_requires_single_matching_usb_device',
                      'dji_foreign_interface_driver', 'session-dji-', '--dms-get-operating-mode']:
            self.assertIn(check, source)
        for forbidden in ['systemctl', 'mmcli', 'QCFG', 'usb_modeswitch', '--dms-set']:
            self.assertNotIn(forbidden, source)
        for workflow in ['beta-validation.yml', 'build-release.yml']:
            self.assertIn('hardware::devices::dji::tests', (ROOT / '.github/workflows' / workflow).read_text())


if __name__ == '__main__':
    unittest.main()
