"""Source fences for explicitly approved, independent and original-bound Alive IO."""
from pathlib import Path
import re
import unittest
ROOT=Path(__file__).resolve().parents[2];P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return (P/name).read_text(encoding='utf-8')
class AccountAliveRuntimeContracts(unittest.TestCase):
    def test_legacy_transport_is_explicit_and_never_a_tls_fallback(self):
        policy=src('alive_profile.rs')
        self.assertIn('pub allow_legacy_stcp:bool',policy);self.assertIn('if !self.allow_legacy_stcp',policy)
        self.assertIn('endpoints[0].scheme()!=b"stcp"',policy);self.assertIn('endpoints.len()!=1',policy)
        self.assertIn('e.host.as_bytes()==endpoints[0].host()&&e.port==endpoints[0].port()',policy)
        self.assertIn('!(60..=600).contains',policy);self.assertIn('fn default_interval()->u32{180}',policy)
        self.assertNotIn('unsafe',policy)
    def test_only_original_sdk_auth_and_bootstrap_data_can_construct_the_live_binding(self):
        arc=src('arc_session.rs');runtime=src('runtime_backend.rs')
        self.assertIn('self.alive.bind_authenticated(&self.owner,&self.connection,result.user_id,&selected_session_id,result.session_token',arc)
        self.assertIn('plan.alive_reference_for(&attempt.owner,&attempt.scope,&common.router)',runtime)
        self.assertIn('self.common_bootstrap.is_none()||self.native_receivers.is_none()||!self.client_ex',runtime)
        self.assertIn('destination.alive_link()',runtime)
        self.assertIn('self.policy.matches(router)',src('runtime_provision.rs'))
    def test_main_and_independent_checks_require_this_agent_token_and_current_registration(self):
        state=src('alive_session.rs');dispatch=src('rpc_client_ex.rs')
        for text in ['state.phase!="registered"','id.token.as_slice()==token','RpcConnectionScope::is_open','elapsed<Duration::from_secs(u64::from(state.interval)*2)','state.activity=Some(now)']:
            self.assertIn(text,state)
        self.assertIn('self.scope.matches',dispatch)
        self.assertIn('accepted && control.is_some()',dispatch)
        self.assertIn('map_or((false,0),|alive|alive.check',dispatch)
        self.assertIn('if *old!=hash{return Err',state)
    def test_requests_keep_uid_selector_native_token_main_account_and_own_rpc_sequence(self):
        state=src('alive_session.rs')
        for text in ['user_id:&identity.uid','selected_session_id:&identity.selector','request_timestamp_ms:wall_ms','key:b"token",value:&identity.token','key:b"mainAccount"','reply.call_id!=pending.id','now>=pending.deadline']:
            self.assertIn(text,state)
        self.assertIn('main.mark_session_revoked()',state)
        self.assertIn('state.agent.as_ref().is_some_and(|s|s.matches(&self.scope))',state)
        self.assertNotIn('pub token:',state)
    def test_tcp_common_partial_deadlines_and_every_io_poll_are_original_gated(self):
        wire=src('alive_io.rs')
        for text in ['prepare_socket_addrs','lookup.resolve().await','main.with_io_gate','scope.with_io_gate','main.session_revoked()','frames.has_partial()','partial_end','CommonAssembler','CommonControls','RecordSecurity::SeededXor']:
            self.assertIn(text,wire)
        for forbidden in ['danger_accept_invalid','NoCertificateVerification','load_library','dlopen','to_socket_addrs']:
            self.assertNotIn(forbidden,wire)
        worker=src('alive_runtime.rs')
        self.assertLess(worker.index('for message in &messages'),worker.index('current.agent.response('))
    def test_worker_backoff_and_owned_join_do_not_log_in_again_or_invent_an_unregister(self):
        worker=src('alive_runtime.rs');runtime=src('runtime_alive.rs')
        for text in ['Duration::from_secs(6)','Duration::from_secs(600)','task.await','self.link.close()','self.task=None']:
            self.assertIn(text,worker)
        for forbidden in ['backend.login(', 'unregisterAlive', '.abort()', 'forget(']:self.assertNotIn(forbidden,worker)
        self.assertIn('alive_stopping=true',runtime);self.assertIn('state.alive_stopping',runtime)
        self.assertIn('run.close().await',runtime)
        self.assertIn('self.task=None;joined',worker)
        self.assertIn('if main.matches(&scope)',src('alive_io.rs'))
        self.assertIn('unregister_started = true; attempt.stop_alive().await?',src('runtime_backend.rs'))
    def test_all_new_behavior_tests_run_under_existing_actions_filter(self):
        names=[]
        for name,count in [('alive_session_tests.rs',18),('alive_io_tests.rs',10),('alive_account_tests.rs',3)]:
            text=src(name);found=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',text)
            if name=='alive_io_tests.rs':
                self.assertIn('consumed_panicked_alive_join_is_not_repolled_on_cleanup_retry',found)
                self.assertIn('cancelled_alive_join_remains_owned_until_the_same_task_finishes_once',found)
            self.assertEqual(len(found),count);names+=found;self.assertNotIn('#[ignore',text)
        self.assertEqual(len(names),len(set(names)))
        self.assertIn('native_bootstrap_independent_alive_main_and_agent_checks_and_stop_join_keep_original_account',names)
        self.assertIn('runtime_alive_tests.rs',src('runtime_common_backend_tests.rs'))
        for workflow in ['build-release.yml','beta-validation.yml']:
            text=(ROOT/'.github/workflows'/workflow).read_text(encoding='utf-8');self.assertIn('connectivity::app_voice::providers::jegotrip::runtime_backend::common_tests \\\n',text)
    def test_ui_never_conflates_registration_transport_approval_and_receiver_ready(self):
        policy=(ROOT/'frontend/src/policies/appLogin.ts').read_text(encoding='utf-8')
        for text in ['独立注册已确认（旧式stcp，非TLS）','不代表短信、来电或媒体就绪','未批准旧式stcp传输，未连接']:self.assertIn(text,policy)
        e2e=(ROOT/'frontend/e2e/jegotrip-alive-status.spec.ts').read_text(encoding='utf-8');self.assertIn('expect(writes).toEqual([])',e2e)
        self.assertIn('accountAliveLabel(account.account_alive_state)',(ROOT/'frontend/src/pages/sim/JegotripPushStatusPane.tsx').read_text(encoding='utf-8'))
if __name__=='__main__':unittest.main()
