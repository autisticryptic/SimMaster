"""Source fences for explicit native call lifecycle; actual behavior is Actions-only."""
from pathlib import Path
import re
import unittest
ROOT=Path(__file__).resolve().parents[2];P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return (P/name).read_text(encoding='utf-8')
class NativeCallLifecycleContracts(unittest.TestCase):
    def test_explicit_configuration_is_bounded_and_legacy_absence_is_omitted(self):
        config=src('account_config.rs');timing=src('native_call_lifecycle.rs')
        self.assertIn('media.native_lifecycle.is_some() && !self.profile.native_call_control',config)
        self.assertIn('native_lifecycle:Option<super::native_call_lifecycle::NativeCallLifecycleConfig>',config)
        self.assertIn('#[serde(skip_serializing_if = "Option::is_none")]',config)
        self.assertIn('checked_mul(1000).filter(|value|*value<=i32::MAX as u32)',timing)
        self.assertIn('#[serde(deny_unknown_fields)]',timing)
    def test_accept_parameter_map_is_not_an_authority_or_media_route(self):
        timing=src('native_call_lifecycle.rs')
        for part in ['key:b"RseEx.SDP"','key:b"mediaType",value:b"audio"','key:b"keepAlive"','key:b"SDK.IsNew",value:b"1"']:
            self.assertIn(part,timing)
        for forbidden in ['TcpStream','token:', 'authority:', 'RseEx.ClientHost','RseEx.CryptoKey','Uuid::']:
            self.assertNotIn(forbidden,timing)
        self.assertIn('self.ticket.same_call(ticket)&&ticket.is_live()',timing)
        self.assertIn('!reply.original_call(ticket)',timing)
    def test_clock_and_duration_records_are_single_use_and_do_not_catch_up_in_bursts(self):
        timing=src('native_call_lifecycle.rs')
        for part in ['checked_duration_since','checked_add(self.period)','accept_reported=true','connection_reported=true','elapsed/self.period.as_nanos()+1']:
            self.assertIn(part,timing)
        self.assertIn('if self.period.is_zero(){None}',timing)
        self.assertIn('if !periodic&&!connected{return Ok(None);}',timing)
        self.assertNotIn('SystemTime',timing)
    def test_keepalive_has_distinct_pending_brand_and_does_not_replace_existence_or_release(self):
        calls=src('call_control.rs');wire=src('rpc_calls.rs')
        self.assertIn('Self::Inform|Self::KeepAlive=>INFORM_METHOD',wire)
        self.assertIn('slot.phase==Phase::Accepted&&ticket.is_live()&&!slot.keepalive_pending',calls)
        self.assertIn('if slot.phase!=Phase::Accepted||!slot.keepalive_pending{return false;}',calls)
        self.assertIn('operation!=CallOperation::Inform',calls)
        self.assertIn('if !accepted||!live{slot.ticket.revoke();slot.phase=Phase::Failed;}',calls)
        self.assertIn('get(b"domain").is_some_and(valid_identifier)',calls)
    def test_worker_retains_join_and_uses_existing_original_runtime_deadline(self):
        owner=src('account_calls.rs');worker=src('account_call_maintenance.rs')
        self.assertIn('call.ticket.revoke();\n        self.join_native_keepalive(call).await?',owner)
        self.assertIn('*slot=Some(tokio::spawn',worker)
        self.assertIn('task.await',worker);self.assertIn('slot.take();result',worker)
        self.assertIn('CallOperation::KeepAlive,&properties',worker)
        for forbidden in ['abort()', 'backend.login(', 'scope.close(', 'from_secs(60)', 'unregisterAlive']:
            self.assertNotIn(forbidden,worker)
        self.assertIn('drop(current)',worker)
        self.assertIn('if !call.ticket.is_live(){return Ok(());}',worker)
    def test_media_ready_is_still_required_and_legacy_accept_is_byte_compatible(self):
        owner=src('account_calls.rs');worker=src('account_call_maintenance.rs')
        self.assertLess(owner.index('if !self.active(call)||!ready.original'),owner.index('self.native_media_connected(call)?'))
        self.assertIn('CallOperation::Accept,&[RpcProperty{key:b"RseEx.SDP",value:&body}]',owner)
        self.assertIn('self.settings.native_lifecycle.map(|policy|policy.bind(ticket))',owner)
        self.assertIn('!current.active(call)',worker)
    def test_exact_new_suites_cover_timer_ordering_wire_pcma_failure_and_join(self):
        for name,count in [('native_call_lifecycle_tests.rs',16),('native_call_lifecycle_account_tests.rs',3)]:
            text=src(name);names=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',text)
            self.assertEqual(len(names),count);self.assertEqual(len(names),len(set(names)));self.assertNotIn('#[ignore',text)
        self.assertIn('native_lifecycle_hangup_joins_pending_heartbeat_without_late_media_revival',src('native_call_lifecycle_account_tests.rs'))
        self.assertIn('native_call_lifecycle_tests.rs',src('native_call_control_tests.rs'))
        self.assertIn('native_call_lifecycle_account_tests.rs',src('native_receiver_account_tests.rs'))
    def test_existing_workflows_and_regression_filters_remain_selected(self):
        for workflow in ['build-release.yml','beta-validation.yml']:
            text=(ROOT/'.github/workflows'/workflow).read_text(encoding='utf-8')
            for part in ['call_control::tests','runtime_backend::common_tests','account_calls::tests']:
                self.assertIn('connectivity::app_voice::providers::jegotrip::'+part+' \\\n',text)
if __name__=='__main__':unittest.main()
