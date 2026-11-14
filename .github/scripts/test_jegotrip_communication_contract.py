"""Supplemental communication source fences; real Rust/TS/browser runs are Actions-only."""
from pathlib import Path
import json,re,unittest
ROOT=Path(__file__).resolve().parents[2]
P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip/app_login'
def source(name):return (P/name).read_text(encoding='utf-8')
class CommunicationContracts(unittest.TestCase):
    def test_intent_keeps_defaults_and_original_identity_without_native_fallback(self):
        text=source('mod.rs')
        self.assertIn('PrepareLocalReceiver, Communication }',text)
        self.assertIn('matches!(self,Self::PrepareLocalReceiver|Self::Communication)',text)
        self.assertIn('intent==LoginIntent::Communication&&source!="database"',text)
        self.assertIn('self.submit_internal_operation(common,intent,preferred,request.communication_operation,request.expected_configuration_revision)',text)
        self.assertLess(text.index('if intent==LoginIntent::Communication{return self.run_communication'),text.index('client.phone_credentials(&bound).await'))
        self.assertIn('fn default_start_receiver() -> bool { true }',text)
    def test_typed_request_has_no_endpoint_token_or_caller_grant(self):
        text=source('communication.rs')
        operation=text.split('pub enum CommunicationOperation{',1)[1].split('\n}',1)[0]
        for forbidden in ['url:','token:','uuid:','grant:','sip_password','start_receiver']:self.assertNotIn(forbidden,operation)
        self.assertIn('deny_unknown_fields',text)
        self.assertIn('pub allow_business_writes:bool',text)
        self.assertIn('http.endpoint(path)?',text)
        self.assertIn('QueryDisplayStatus',text);self.assertIn('QueryForwardingState',text)
    def test_current_authenticated_number_and_exact_json_protection_are_used(self):
        text=source('communication_http.rs')
        self.assertIn('!self.config.extra_body.is_empty()',text)
        self.assertIn('"phoneServerStatus":status',text);self.assertIn('"type":1',text)
        self.assertIn('Operation::QueryProfile|Operation::Logout=>(None,false)',text)
        self.assertIn('Operation::QueryDisplayStatus=>(None,true)',text)
        self.assertIn('"mobile":bound.mobile.expose_to_backend(),"countryCode":bound.country',text)
        self.assertIn('"jegom_status":jegom_status,"region_code":region_code,"country_code":country_code',text)
        self.assertIn('Authentication::Session,encrypted',text)
        self.assertIn('self.reset_session()',text)
    def test_confirmation_uses_the_right_original_models_not_requested_values(self):
        text=source('communication_http.rs')
        self.assertIn('Operation::SetJegoMStatus{..}=>(endpoints.path(Action::QueryDisplayStatus)',text)
        self.assertIn('self.config.endpoints.call_status.as_str()',text)
        self.assertIn('self.config.endpoints.call_status.clone()',text)
        flow=source('communication_flow.rs');self.assertIn('status.cs_forward==Some(2)',flow);self.assertIn('CommunicationState::Acknowledged',flow)
        self.assertNotIn('CommunicationState::Confirmed;self',flow.split('CommunicationAction::CloseForward{',1)[1].split('CommunicationAction::Logout{',1)[0])
    def test_write_uncertainty_precedes_io_and_is_owner_and_original_context_cas_bound(self):
        flow=source('communication_flow.rs');self.assertLess(flow.index('store::set_communication'),flow.index('client.perform_communication'))
        self.assertIn('self.confirm_device_identity(guard,&key,true)?',flow)
        self.assertIn('native_ownership_pending',flow)
        store=source('store.rs');self.assertIn("intent='communication' AND communication IS ?5",store)
        self.assertIn('old.device_identity_id!=observation.device_identity_id',store)
        self.assertIn('old.configuration_revision!=observation.configuration_revision',store)
        self.assertIn('owner=?4 AND completed=0',store)
    def test_unknown_writes_block_new_nonces_and_do_not_disable_read_only_queries(self):
        store=source('store.rs');self.assertIn('pub(super) fn communication_write_unresolved',store)
        self.assertIn('observation.action.writes()',store)
        self.assertIn('CommunicationState::RequestUncertain|super::communication::CommunicationState::AcceptedUnconfirmed',store)
        manager=source('mod.rs');self.assertIn('store::communication_write_unresolved(&tx,&snapshot.id)?',manager)
        self.assertIn('operation.action().writes()',manager)
        self.assertIn('client.trusted_reuse_write_started()',manager)
    def test_new_observation_is_bounded_and_cannot_override_old_history_or_trust(self):
        store=source('store.rs');text=source('communication.rs')
        self.assertIn('ADD COLUMN communication TEXT',store)
        self.assertIn("typeof(communication)='text'",store);self.assertIn('BETWEEN 1 AND 32768',store)
        self.assertIn('value.len()>32768',text);self.assertIn('stored_code(v).is_some()',text)
        self.assertIn('CommunicationStatus::valid',text);self.assertIn('super::valid_attempt_id(&self.device_identity_id)',text)
        self.assertIn('BETWEEN 1 AND 2048',store);self.assertIn('device_trust',store)
    def test_profile_uses_first_list_and_bounded_typed_countries_without_reflected_names(self):
        text=source('communication.rs');profile=source('communication_profile.rs')
        self.assertIn('body.get("list")',text);self.assertIn('list.first()',text)
        self.assertIn('rows.len()>256',profile);self.assertIn('region.map(|value|value.to_string())',profile)
        self.assertIn('pub region_id:Option<String>',profile)
        self.assertNotIn('pub country_cname:',profile)
        self.assertIn('masked_jegom_phone',text)
    def test_api_keeps_authentication_no_store_and_explicit_refusal_codes(self):
        text=(ROOT/'backend/src/api/app_voice.rs').read_text(encoding='utf-8')
        self.assertIn('app_login_communication_writes_disabled" => StatusCode::FORBIDDEN',text)
        self.assertIn('app_login_communication_unresolved',text)
        tests=(ROOT/'backend/src/api/app_voice_communication_http_tests.rs').read_text(encoding='utf-8')
        for name in ['communication_api_requires_auth','communication_api_configuration_is_same_origin','communication_api_missing_endpoint']:self.assertIn(name,tests)
        self.assertIn('no_store(&headers)',tests);self.assertNotIn('account_runtime::local_nonce()',tests)
    def test_ui_default_does_not_select_a_write_or_native_receiver_for_communication(self):
        policy=(ROOT/'frontend/src/policies/appLogin.ts').read_text(encoding='utf-8')
        self.assertIn("startReceiver: true, intent: 'enable_hosting'",policy)
        self.assertIn('validCommunicationOperation(draft.communicationOperation)',policy)
        self.assertIn('Number.isSafeInteger(draft.configurationRevision)',policy)
        self.assertIn('expected_configuration_revision: draft.configurationRevision',policy)
        pane=(ROOT/'frontend/src/pages/sim/AppAccountLoginPane.tsx').read_text(encoding='utf-8')
        self.assertIn('writeConsent: false',pane);self.assertIn('communicationStatusLines(attempt.communication)',pane)
        self.assertIn('api.getJegotripLoginAttempt(id)',pane)
        for forbidden in ['localStorage','sessionStorage']:self.assertNotIn(forbidden,pane)
    def test_all_new_tests_extend_existing_actions_and_preserve_browser_budgets(self):
        tests=source('communication_tests.rs');names=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',tests)
        self.assertGreaterEqual(len(names),25);self.assertEqual(len(names),len(set(names)))
        self.assertNotIn('#[ignore',tests);self.assertNotIn('RUST_MIN_STACK',tests)
        self.assertIn('mod communication;',source('tests.rs'))
        config=(ROOT/'frontend/playwright.jegotrip.config.ts').read_text();self.assertIn('jegotrip-communication.spec.ts',config)
        for budget in ['timeout: 45_000','timeout: 10_000','workers: 1','retries: 0','timeout: 120_000']:self.assertIn(budget,config)
        package=json.loads((ROOT/'frontend/package.json').read_text());self.assertIn('tests/communicationOps.test.ts',package['scripts']['test:unit'])
        for name in ['build-release.yml','beta-validation.yml']:
            workflow=(ROOT/'.github/workflows'/name).read_text();self.assertIn('connectivity::app_voice::providers::jegotrip::app_login::tests',workflow);self.assertIn('pnpm run type-check',workflow)
    def test_communication_does_not_claim_sdk_sms_media_platform_or_paid_features(self):
        flow=source('communication_flow.rs');wire=source('communication_http.rs')
        for forbidden in ['accounts.create(','.start(','.enable_business(','.sync_device(','.enroll_device(']:self.assertNotIn(forbidden,flow+wire)
        policy=(ROOT/'frontend/src/policies/communicationOps.ts').read_text(encoding='utf-8');self.assertIn('不代表设备授信、SDK或收件成功',policy)
        self.assertIn('Object.prototype.hasOwnProperty.call',policy)
        for forbidden in ['dlopen(', 'Command::new(', 'allow_plain_media:true']:self.assertNotIn(forbidden,flow+wire)
if __name__=='__main__':unittest.main()
