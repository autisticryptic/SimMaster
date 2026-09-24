"""Guard native URC ownership, admission, bounded hints and CI execution."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
CELLULAR = ROOT / 'backend/src/hardware/cellular'


class NativeUrcBoundaryTests(unittest.TestCase):
    def test_passive_poll_has_no_at_writes_and_has_a_read_budget(self):
        source = (CELLULAR / 'at_session.rs').read_text()
        body = source.split('    fn poll_events(', 1)[1].split('    fn pop_line(', 1)[0]
        self.assertIn('MAX_DRAIN_READS', body)
        self.assertNotIn('write_command', body)
        self.assertNotIn('execute_command', body)
        wrapper = source.split('pub fn poll_events(device:', 1)[1].split('#[cfg(not(unix))]', 1)[0]
        self.assertIn('session.lock()', wrapper.replace('\n', '').replace(' ', ''))

    def test_urc_api_exposes_only_coalesced_boolean_hints(self):
        source = (CELLULAR / 'at_urc.rs').read_text()
        fields = source.split('pub struct UrcEvents {', 1)[1].split('}', 1)[0]
        self.assertNotIn('String', fields)
        self.assertNotIn('Vec', fields)
        self.assertEqual(fields.count(': bool'), 7)

    def test_sms_poll_is_admitted_and_uses_the_physical_command_gate(self):
        source = (ROOT / 'backend/src/services/messaging/sms_listener.rs').read_text()
        body = source.split('async fn native_sms_events_pending(', 1)[1].split('async fn scan_all_modems_or_rebind(', 1)[0]
        for gate in ['profile.enabled', 'binding.present']:
            self.assertLess(body.index(gate), body.index('device.poll_events()'))
        for gate in ['sms_reception_enabled', 'modem_sms_scan_allowed', 'modem_sms_paused_for_ims']:
            self.assertIn(gate, body)
        admitted_scan = source.split('async fn maybe_scan_sms_paths(', 1)[1].split('async fn native_sms_events_pending(', 1)[0]
        self.assertLess(admitted_scan.index('modem_sms_scan_allowed'), admitted_scan.index('device.initialize_sms()'))
        source = (CELLULAR / 'backends/messages.rs').read_text()
        body = source.split('pub async fn poll_sms_events(', 1)[1].split('pub async fn initialize_sms(', 1)[0]
        self.assertIn('self.require_sms_reception()?', body)
        self.assertIn('self.command(request).await?', body)
        self.assertNotIn('self.io.execute', body)

    def test_calls_and_registration_reconcile_on_events_or_loss(self):
        for path, hint in [('backend/src/api/handlers.rs', 'event.hints.call_changed'),
                           ('backend/src/main.rs', 'event.hints.registration_changed')]:
            source = (ROOT / path).read_text()
            self.assertIn('backends::events::subscribe()', source)
            self.assertIn(hint, source)
            self.assertIn('RecvError::Lagged', source)
        source = (CELLULAR / 'backends/events.rs').read_text()
        self.assertIn('broadcast::channel(64)', source)

    def test_regressions_are_run_not_just_compiled(self):
        for workflow in ['beta-validation.yml', 'build-release.yml']:
            source = (ROOT / '.github/workflows' / workflow).read_text()
            for module in ['at_session', 'at_urc']:
                self.assertIn(f'hardware::cellular::{module}::tests', source)
            self.assertIn('hardware::cellular::backends::events::tests', source)


if __name__ == '__main__':
    unittest.main()
