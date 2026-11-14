"""Static fences for original native CallServerEx wiring; runtime proof is Actions-only."""
from pathlib import Path
import re
import unittest
ROOT=Path(__file__).resolve().parents[2]
P=ROOT/'backend/src/connectivity/app_voice/providers/jegotrip'
def native(name):return (P/name).read_text(encoding='utf-8')

class NativeCallControlContracts(unittest.TestCase):
    def test_explicit_native_flag_is_default_off_and_old_private_serialization_is_preserved(self):
        source=native('account_config.rs')
        self.assertIn('#[serde(default)]\n    pub native_call_control:bool',source)
        self.assertIn('#[serde(skip_serializing_if = "offline_disabled")]\n    native_call_control:bool',source)
        self.assertEqual(source.count('native_call_control:p.native_call_control'),2)
        self.assertIn('if p.native_call_control{profile.with_native_call_control().map_err(|_|INVALID)}',source)
        self.assertIn('pub call_control:Option<Vec<ProfileProperty>>',source)

    def test_native_route_requires_common_receiver_and_cannot_replace_legacy(self):
        source=native('runtime_backend.rs')
        new=source.split('fn with_native_call_control(',1)[1].split('pub fn supplied(',1)[0]
        for guard in ['self.common.is_none()','self.native_receivers.is_none()','self.call_incoming.is_some()','self.call_control.is_some()','self.native_call_control']:
            self.assertIn(guard,new)
        self.assertIn('add_bytes(&mut self.byte_length,super::call_control::NATIVE_CONTROL_BYTES)?',new)
        old=source.split('pub fn with_call_control(',1)[1].split('fn with_native_call_control(',1)[0]
        self.assertIn('validate_routing(routing)',old)
        self.assertIn('|| self.native_call_control',old)

    def test_native_default_and_legacy_domain_policy_remain_distinct(self):
        source=native('call_control.rs')
        self.assertIn('enum CallRouting { Explicit(Vec<Property>), NativeDefault }',source)
        legacy=source.split('fn validate_routing(',1)[1].split('struct CallIdentity',1)[0]
        self.assertIn('get(b"domain").is_some_and(valid_identifier)',legacy)
        self.assertIn('CallRouting::NativeDefault=>vec![RpcProperty{key:b"id",value:NATIVE_CALL_OBJECT}]',source)
        self.assertIn('ReferenceTarget::IdentityOnly',source)
        self.assertIn('reference.components().len()==1&&reference.identity()==value',source)
        for forbidden in ['TcpStream','connect(', 'root_der_base64', 'disable_tls']:
            self.assertNotIn(forbidden,source)

    def test_native_existence_query_has_exact_properties_and_keeps_per_call_handoff(self):
        source=native('call_control.rs')
        self.assertIn('NATIVE_CALL_OBJECT:&[u8]=b"#CallServerEx"',source)
        self.assertIn('EXISTENCE_INFO:&[u8]=b"Invited callId is exist?"',source)
        arguments=source.split('fn arguments(',1)[1].split('/// Duplicate invitations',1)[0]
        self.assertIn('operation==CallOperation::Inform',arguments)
        self.assertIn('if !properties.is_empty(){return Err(CodecError::InvalidField);}',arguments)
        self.assertIn('key:b"RseEx.Info",value:EXISTENCE_INFO',arguments)
        self.assertIn('ticket.native_id()',arguments)
        self.assertIn('if operation!=CallOperation::Inform{if let Some(id)=&slot.override_id',source)
        self.assertIn('if notice.kind()==CallNoticeKind::Invited{return Ok(None);}',source)

    def test_activation_stays_original_common_authenticated_and_same_batch_ready(self):
        session=native('arc_session.rs')
        block=session.split('fn configure_native_call_control(',1)[1].split('fn register_call_notice(',1)[0]
        self.assertIn('self.phase()!=ArcPhase::Authenticated',block)
        self.assertIn('!self.common_transport_claimed',block)
        self.assertIn('self.connection.session_revoked()',block)
        self.assertIn('self.call_book.configure_native()',block)
        self.assertIn('self.call_book.arguments(ticket,operation,properties)?',session)
        self.assertIn('metadata.push(RpcProperty{key:b"token",value:&auth.token})',session)
        self.assertIn('if setup.native_call_control { self.rpc.configure_native_call_control()?; }',native('arc_protocol.rs'))
        self.assertIn('capture_native_control(policy.clone(),self.profile.client_ex)',native('runtime_backend.rs'))

    def test_call_configuration_is_not_answer_or_media_readiness(self):
        source=native('account_config.rs')
        self.assertIn('let Some(media) = &self.call_media else { return Ok(None); }',source)
        self.assertIn('self.profile.call_control.is_none() && !self.profile.native_call_control',source)
        self.assertIn('if !input.allow_plain_media',native('call_settings.rs'))
        self.assertIn('state.phase==Phase::Connected&&self.ticket.is_live()',native('account_calls.rs'))
        self.assertIn('call_control_present:p.call_control.is_some() || p.native_call_control',native('app_login/push_status.rs'))

    def test_new_rust_suites_preserve_negative_and_true_wire_cases(self):
        names=[]
        for file,count in [('native_call_control_tests.rs',18),('native_call_protocol_tests.rs',3),('native_call_runtime_tests.rs',2),('native_call_account_tests.rs',1)]:
            source=native(file);found=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',source)
            self.assertEqual(len(found),count);self.assertNotIn('#[ignore',source);names+=found
        self.assertEqual(len(names),len(set(names)))
        for name in ['native_original_invitation_oid_is_used_only_after_existence_for_accept_and_release',
                     'native_existence_pending_rejects_foreign_owner_scope_sequence_and_duplicate_result',
                     'default_native_batch_call_retains_original_ack_boundary_and_does_not_apply_direct_domain_filter',
                     'native_default_common_call_wire_preserves_ack_existence_handoff_and_release_without_media_grant',
                     'native_default_control_without_media_never_publishes_answer_ready_or_fake_call_history']:
            self.assertIn(name,names)

    def test_new_suites_use_existing_actions_filters_and_no_budget_or_dependency_change(self):
        for parent,child in [('call_control_tests.rs','native_call_control_tests.rs'),('native_receiver_protocol_tests.rs','native_call_protocol_tests.rs'),
                             ('native_receiver_runtime_tests.rs','native_call_runtime_tests.rs'),('native_receiver_account_tests.rs','native_call_account_tests.rs')]:
            self.assertIn(child,native(parent))
        for workflow in ['build-release.yml','beta-validation.yml']:
            source=(ROOT/'.github/workflows'/workflow).read_text(encoding='utf-8')
            for suffix in ['call_control::tests','native_receiver::tests','runtime_backend::common_tests']:
                self.assertIn('connectivity::app_voice::providers::jegotrip::'+suffix+' \\\n',source)
        self.assertIn('const MAX_CALL_HISTORY:usize=128',native('call_control.rs'))
        self.assertIn('const MAX_STREAM_BYTES',native('rpc_stream.rs'))

if __name__=='__main__':unittest.main()
