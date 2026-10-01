"""Guard the production default policy; behavior is tested by the Rust suites."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / 'backend/src'
IMS = SRC / 'connectivity/modems/ims/cellular_ims'


class DefaultImsFamilyPolicyTests(unittest.TestCase):
    def test_production_configuration_has_no_family_override(self):
        config = (SRC / 'platform/config.rs').read_text()
        for forbidden in ('pub cellular_ims_ip_families', 'pub enum CellularImsIpFamily',
                          'fn set_line_cellular_ims_ip_families', 'fn get_line_cellular_ims_ip_families'):
            self.assertNotIn(forbidden, config)
        contract = (ROOT / 'frontend/src/api/contracts.ts').read_text()
        self.assertNotIn('cellular_ims_ip_families', contract)
        self.assertNotIn('CellularImsIpFamily', contract)

    def test_production_always_constructs_the_default_plan(self):
        live = (IMS / 'live.rs').read_text()
        production = live.split('pub async fn connect_live_for_line(', 1)[1].split('pub(crate) async fn probe_owned_profile_once(', 1)[0]
        self.assertIn('let plan = ImsConnectionPlan::default();', production)
        self.assertNotIn('line_ip_families', live)
        self.assertNotIn('for_profile_probe', production)
        self.assertIn('ImsConnectionPlan::for_profile_probe(family)', live)
        plan = (IMS / 'plan.rs').read_text()
        self.assertIn('bearer_attempts: vec![IpType::Ipv4v6, IpType::Ipv6, IpType::Ipv4]', plan)
        for forbidden in ('from_families(', 'from_preference(', 'with_catalog_ip_stack_hint('):
            self.assertNotIn(forbidden, plan)

    def test_retired_endpoints_have_no_handler_or_route(self):
        handlers = (SRC / 'api/handlers.rs').read_text()
        main = (SRC / 'main.rs').read_text().split('#[cfg(test)]\nmod http_router_tests', 1)[0]
        self.assertNotIn('set_cellular_ims_line_ip_families_handler', handlers + main)
        self.assertNotIn('"/api/cellular-ims/lines/{line_id}/ip-families"', main)
        self.assertNotIn('"/api/volte/lines/{line_id}/ip-families"', main)

    def test_migration_covers_both_spellings_and_only_retired_keys(self):
        store = (SRC / 'platform/config_store.rs').read_text()
        for prefix in ('cellular_ims', 'volte'):
            for suffix in ('ip_families', 'ip_families_auto'):
                self.assertIn(f"'$.{prefix}_{suffix}'", store)
        self.assertIn('malformed_configuration_does_not_partially_retire_overrides', store)
        self.assertIn('failed_migration_rolls_back_every_line', store)

    def test_fault_is_observed_before_admission_can_discard_the_result(self):
        handlers = (SRC / 'api/handlers.rs').read_text()
        result = handlers.split('let unsafe_failure =', 1)[1]
        self.assertLess(result.index('observe_cellular_ims_failure(error)'), result.index('if !runtime.task_is_current()'))
        self.assertIn('if source != "manual"', handlers)
        runtime = (SRC / 'hardware/devices/qcm410/primary_ims_profile_runtime.rs').read_text()
        self.assertIn('bearer_failure_after_cleanup(', runtime)

    def test_diagnostic_budget_does_not_remove_production_forced_retry(self):
        native = (IMS / 'native_bearer.rs').read_text()
        self.assertIn('plan.allows_forced_family_retry()', native)
        for name in ('production_fallback_visits_all_families_after_ordinary_refusals',
                     'production_forced_family_remains_bounded_and_deduplicated',
                     'diagnostic_network_rejection_never_dispatches_another_family',
                     'baseband_fault_stops_activation_without_changing_the_default_plan'):
            self.assertIn(name, native)

    def test_both_ci_suites_run_migration_and_removed_route_tests(self):
        for workflow in ('beta-validation.yml', 'build-release.yml'):
            text = (ROOT / '.github/workflows' / workflow).read_text()
            self.assertIn('platform::config_store::tests', text)
            self.assertIn('retired_family_override_routes_cannot_modify_any_line', text)
            self.assertIn('services::line_registry::tests', text)
            self.assertIn('connectivity::modems::ims::cellular_ims::native_bearer::tests', text)


if __name__ == '__main__':
    unittest.main()
