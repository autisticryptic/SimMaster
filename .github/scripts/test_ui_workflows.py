"""UI wiring guards; behavioral helpers run in Node and Rust runs in Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]

class UiWorkflowTests(unittest.TestCase):
    def test_flight_mode_has_compact_status_without_raw_stage_paragraphs(self):
        text = (ROOT / 'frontend/src/pages/sim/ModemLinesPanel.tsx').read_text()
        row = text[text.index('<FlightTakeoff'):text.index('<TravelExplore')]
        self.assertIn('airplaneControlLabel(', row)
        self.assertNotIn('开关表示保存的意图', row)
        self.assertNotIn('airplane_stage', row)
        self.assertIn('checked={airplaneEnabled}', row)
        self.assertIn('toggleAirplaneMode(', row)

    def test_registration_mode_selector_is_removed_not_just_disabled(self):
        text = (ROOT / 'frontend/src/pages/sim/ModemLinesPanel.tsx').read_text()
        self.assertNotIn('ImsRegistrationSettings', text)
        self.assertFalse((ROOT / 'frontend/src/pages/sim/ImsRegistrationSettings.tsx').exists())
        policies = (ROOT / 'frontend/src/policies/imsRegistration.ts').read_text()
        self.assertNotIn('registrationModeOptions', policies)
        self.assertIn('hasConfirmedDualRegistration', policies)
        config = (ROOT / 'backend/src/platform/config.rs').read_text()
        migration = config[config.index('fn migrate_automatic_ims_registration('):config.index('impl ConfigManager {', config.index('fn migrate_automatic_ims_registration('))]
        self.assertIn('profile.ims_access_preference = ImsAccessPreference::Concurrent', migration)
        self.assertNotIn('vowifi.enabled =', migration)
        self.assertNotIn('cellular_ims_connection_enabled =', migration)
        self.assertIn('ims_registration_mode_automatic_only', config)

    def test_quick_esim_switch_uses_existing_endpoint_and_readback(self):
        page = (ROOT / 'frontend/src/pages/SimCard.tsx').read_text()
        hook = (ROOT / 'frontend/src/hooks/useEsimManager.ts').read_text()
        store = (ROOT / 'frontend/src/utils/esimManagerStore.ts').read_text()
        cards = (ROOT / 'frontend/src/pages/EsimManager.tsx').read_text()
        self.assertIn('useEsimManager(lineId', page)
        self.assertIn('<EsimProfileManager state={esimState}', page)
        self.assertIn('!line?.modem.present', page)
        self.assertNotIn('api.enableEsimProfile', page)
        self.assertIn('api.enableEsimProfile(id, iccid)', hook)
        self.assertIn('api.getEsimProfiles(id)', hook)
        self.assertIn('api.getBasebandRestartStatus(id)', hook)
        self.assertIn('snapshot.loading || snapshot.operation || snapshot.needsRefresh', store)
        self.assertIn('await switchEsimProfile(api, lineId, iccid', store)
        self.assertIn('disabled={unavailable || Boolean(switchBlocked)}', cards)
        self.assertNotIn('完整管理', page)
        helper = (ROOT / 'frontend/src/utils/esimQuickSwitch.ts').read_text()
        self.assertEqual(helper.count('await api.enable('), 1)
        self.assertLess(helper.index('const before = await api.profiles('), helper.index('await api.enable('))
        self.assertIn('progress.steps.find(', helper)
        self.assertIn('profile.iccid === iccid && esimProfileActive(profile)', helper)
        self.assertIn('不要重复提交', helper)
        self.assertNotIn('setInterval', helper)
        self.assertNotIn('api.setLineEsimControl', helper)

    def test_regression_filters_and_frontend_workflow_cover_new_checks(self):
        package = (ROOT / 'frontend/package.json').read_text()
        self.assertIn('tests/uiWorkflows.test.ts', package)
        for workflow in ('build-release.yml', 'beta-validation.yml'):
            text = (ROOT / '.github/workflows' / workflow).read_text()
            for suite in ('platform::config::tests', 'connectivity::core::ims_access::tests', 'ims_registration_preference_api_preserves_cost_and_enable_settings'):
                self.assertIn(suite, text)

if __name__ == '__main__':
    unittest.main()
