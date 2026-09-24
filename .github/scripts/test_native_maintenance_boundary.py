from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class NativeMaintenanceBoundaryTests(unittest.TestCase):
    def test_maintenance_is_native_targeted_and_never_an_ims_fallback(self):
        api = (ROOT / 'backend/src/api/native_controls.rs').read_text()
        self.assertIn('backends::native_device(&binding.modem_path)', api)
        self.assertIn('native_maintenance_disable_ims_and_data_first', api)
        impl = (ROOT / 'backend/src/hardware/devices/quectel/maintenance.rs').read_text()
        for forbidden in ['mmcli', 'systemctl', 'CGDCONT', 'CGACT=0', 'QCFG=\\"usbcfg\\",']:
            self.assertNotIn(forbidden, impl)
        self.assertIn('native_maintenance_plan_stale', impl)
        self.assertIn('native_maintenance_confirmation_required', impl)
        self.assertIn('tokio::spawn(async move', impl)
        self.assertLess(impl.index('device.io.save_receipt'), impl.index('let commands:'))
        for path in (ROOT / 'backend/src/connectivity/modems/ims/cellular_ims').glob('*.rs'):
            self.assertNotIn('maintenance::apply', path.read_text())

    def test_unresolved_maintenance_blocks_reactivation(self):
        io = (ROOT / 'backend/src/hardware/cellular/backends/io.rs').read_text()
        self.assertIn('for role in ["ims", "data", "maintenance", "sim"]', io)
        self.assertIn('native_maintenance_reconciliation_required', io)
        for workflow in ['beta-validation.yml', 'build-release.yml']:
            self.assertIn('hardware::devices::quectel::maintenance::tests',
                          (ROOT / '.github/workflows' / workflow).read_text())


if __name__ == '__main__':
    unittest.main()
