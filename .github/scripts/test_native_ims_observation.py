"""Static X2 wiring guards; real native/serial/service/HTTP tests run only on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


def source(path):
    return (ROOT / path).read_text(encoding='utf-8')


class NativeImsObservationBoundary(unittest.TestCase):
    def test_status_endpoint_never_dispatches_hardware_or_changes_intent(self):
        text = source('backend/src/services/native_ims_observation.rs')
        view = text[text.index('pub async fn status('):text.index('pub async fn probe(')]
        for prohibited in ['.discover(', '.refresh(', '.probe_native_ims(', '.execute(', '.publish(',
                           '.park(', 'set_line_', 'get_or_init', '.create_socket(']:
            self.assertNotIn(prohibited, view)
        self.assertIn('OBSERVATION_TTL', text)
        self.assertIn('scope_current', text)
        self.assertIn('native_observation_epoch()', text)
        self.assertIn('native_observation_binding()', text)
        self.assertIn('modem_observation_not_host_ims_readiness', text)

    def test_probe_is_shielded_and_excludes_both_host_accesses(self):
        text = source('backend/src/services/native_ims_observation.rs')
        for gate in ['profile.cellular_ims_connection_enabled', 'profile.vowifi.enabled',
                     'idle_for_native_ims_observation', 'try_native_observation()',
                     'transition_lock.try_lock()', 'bearer_operation_lock.try_lock()',
                     'cellular_ims_connect_lock.try_lock()', 'vowifi_connect_lock.try_lock()']:
            self.assertIn(gate, text)
        body = text[text.index('pub async fn probe('):]
        self.assertLess(body.index('tokio::spawn(async move'), body.index('.probe_native_ims('))
        self.assertIn('task.await', body)
        self.assertNotIn('timeout(', body)
        self.assertIn('Arc::ptr_eq(current, &line)', body)
        for mutation in ['.set_ready(', '.park(', '.publish(', 'set_line_', 'release_native_ims_bearer(']:
            self.assertNotIn(mutation, body)

    def test_driver_uses_exact_read_query_and_never_the_enable_form(self):
        codec = source('backend/src/hardware/devices/quectel/native_ims.rs')
        self.assertIn('STATUS_COMMAND: &str = "AT+QIMSCFG=\\\"ims_status\\\"";', codec)
        self.assertIn('0 => NativeImsRegistration::UnknownOrUnreported', codec)
        self.assertIn('ut: service(values[5])', codec)
        driver = source('backend/src/hardware/cellular/backends/native_ims.rs')
        self.assertEqual(driver.count('native_ims_read(ec20::STATUS_COMMAND,'), 1)
        for gate in ['verify_primary_slot_locked()', 'parse_iccid(&before)', 'parse_iccid(&after)',
                     'ec20::qualify(&model, &revision)', 'try_lock_owned()', 'task.await']:
            self.assertIn(gate, driver)
        for prohibited in ['.save_receipt(', '.clear_receipt(', 'QCFG=', 'CFUN=', 'QMBNCFG',
                           'set-packet', 'wds-start-network', 'execute_usim_authenticate']:
            self.assertNotIn(prohibited, driver)

    def test_real_native_serial_service_and_http_filters_are_executed(self):
        for workflow in ['beta-validation.yml', 'build-release.yml']:
            text = source('.github/workflows/' + workflow)
            for suite in ['hardware::cellular::backends::native_ims::tests',
                          'hardware::devices::quectel::native_ims::tests',
                          'hardware::devices::quectel::native_ims::serial_tests',
                          'services::native_ims_observation::tests',
                          'http_router_tests::native_ims_observation_api_']:
                self.assertIn(suite, text)
            self.assertIn('dbus-run-session', text)
            self.assertIn('Empty regression filter', text)
        router = source('backend/src/main.rs')
        self.assertIn('/api/modem/lines/{line_id}/native/ims/probe', router)
        self.assertIn('/api/modem/lines/{line_id}/native/ims/status', router)
        api = source('backend/src/api/native_ims_observation.rs')
        self.assertIn('deny_unknown_fields', api)
        self.assertNotIn('.refresh()', api)


if __name__ == '__main__':
    unittest.main()
