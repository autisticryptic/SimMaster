from pathlib import Path
import unittest
import re

ROOT = Path(__file__).resolve().parents[2]


class NativeSimLedgerBoundaryTests(unittest.TestCase):
    def test_ledger_tracks_metadata_not_subscriber_or_auth_material(self):
        source = (ROOT / 'backend/src/hardware/cellular/backends/sim_ledger.rs').read_text()
        owner = source.split('pub struct ChannelOwner {', 1)[1].split('}', 1)[0]
        for forbidden in ('aid:', 'apdu:', 'imsi:', 'iccid:', 'rand:', 'autn:', 'key:'):
            self.assertNotIn(forbidden, owner)
        drop = source.split('impl Drop for ChannelLease', 1)[1].split('#[cfg(test)]', 1)[0]
        self.assertNotIn('clear_receipt', drop)
        self.assertNotIn('.execute', drop)
        self.assertIn('reconciliation_required = true', drop)

    def test_at_qmi_and_external_helpers_share_a_ledger(self):
        paths = ['backend/src/hardware/cellular/backends/sim.rs',
                 'backend/src/connectivity/modems/ims/vowifi/qmi_uim.rs',
                 'backend/src/hardware/sim/esim.rs']
        for path in paths:
            self.assertIn('sim_ledger::ChannelLease::begin', (ROOT / path).read_text())
        qmi = re.sub(r'\s+', '', (ROOT / paths[1]).read_text())
        self.assertIn('receipt.opened', qmi)
        self.assertIn('receipt.closed', qmi)
        self.assertIn('QmiUimError::ResultFailure', qmi)
        at = (ROOT / paths[0]).read_text()
        self.assertIn('channel.receipt.opened', at.replace('\n', '').replace(' ', ''))

    def test_status_and_regressions_are_wired(self):
        self.assertIn('"sim_channels": device.sim_channel_status()',
                      (ROOT / 'backend/src/api/handlers.rs').read_text())
        for workflow in ['beta-validation.yml', 'build-release.yml']:
            source = (ROOT / '.github/workflows' / workflow).read_text()
            self.assertIn('backends::sim_ledger::tests', source)
            self.assertIn('vowifi::qmi_uim::tests', source)


if __name__ == '__main__':
    unittest.main()
