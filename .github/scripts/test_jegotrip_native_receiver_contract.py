"""Static native adapter boundary; only source reads, never Rust/SDK/devices."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
MODULE = ROOT / "backend/src/connectivity/app_voice/providers/jegotrip"
def source(name): return (MODULE / name).read_text(encoding="utf-8")

class NativeReceiverContracts(unittest.TestCase):
    def test_direct_and_batch_policies_are_distinct_and_match_known_objects(self):
        policy = source("native_receiver.rs")
        self.assertIn('MESSAGE_OBJECT: &[u8] = b"MessageReceiver"', policy)
        self.assertIn('CALL_OBJECT: &[u8] = b"CallClientEx"', policy)
        self.assertIn('get(b"id") != Some(object.identity())', policy)
        self.assertIn("if transport == ReceiverTransport::Direct", policy)
        self.assertIn("DomainMode::DenyList => !listed, DomainMode::AllowList => listed", policy)
        self.assertIn("filter(|value| !value.is_empty())", policy)
        self.assertIn("values[..index].contains(value)", policy)
        self.assertNotIn("parse_object_reference", policy)
    def test_native_constructor_does_not_replace_or_relax_legacy_exact_binding(self):
        for name in ["rpc_incoming.rs", "call_binding.rs", "rpc_client_ex.rs"]:
            text = source(name)
            self.assertIn("for_native_client", text)
            self.assertIn("ReceiverTransport::Batch", text)
            self.assertIn("ReceiverTransport::Direct", text)
            self.assertIn('property(b"domain") != Some(self.domain.as_slice())', text)
            self.assertIn("native.policy().validate", text)
        backend = source("runtime_backend.rs")
        self.assertIn("if self.common.is_none() || self.native_receivers.is_some() || self.incoming.is_some() || self.call_incoming.is_some()", backend)
        self.assertIn("if self.native_receivers.is_some() && binding.is_some()", backend)
    def test_native_activation_follows_matched_login_on_original_common_scope(self):
        arc = source("arc_session.rs")
        self.assertIn("self.common_transport_claimed && self.phase() == ArcPhase::Authenticated", arc)
        self.assertIn("!self.connection.session_revoked()", arc)
        protocol = source("arc_protocol.rs")
        self.assertIn("!self.rpc.can_arm_native_receivers()", protocol)
        reply = protocol[protocol.index("Ok(Some(reply)) =>"):protocol.index("Ok(None) => Ok(false)")]
        self.assertLess(reply.index("self.activate_native_receivers()?"), reply.index("deliver(ProtocolEvent::Reply(reply))"))
        self.assertIn("self.rpc.configure_call_control(&routing)?", protocol)
        self.assertIn("self.native_receivers = None", protocol)
    def test_actor_original_setup_and_configuration_are_private_bounded_and_not_a_ready_grant(self):
        actor = source("transport_actor.rs")
        self.assertIn("Operation::ArmNativeReceivers(setup)", actor)
        self.assertIn("Operation::ConfigureNativeReceivers(incarnation)", actor)
        self.assertIn("Self::ArmNativeReceivers(setup) => setup.retained_bytes()", actor)
        self.assertIn("if !self.accepts(owner, scope)", actor)
        offline = source("offline_backend.rs")
        self.assertIn("self.protocol.native_setup().is_none()", offline)
        self.assertIn("self.protocol.native_message_dispatcher()?", offline)
        self.assertIn("self.protocol.take_batch_scheduler(incarnation)?", offline)
    def test_persistence_represents_missing_single_domain_without_fake_empty_or_retroactive_migration(self):
        inbox = source("account_inbox.rs")
        self.assertIn("domain: Option<Zeroizing<Vec<u8>>>", inbox)
        self.assertIn("if self.receiver.is_some() || !binding.matches(&self.owner, &self.scope)", inbox)
        self.assertIn("domain: None, kind: Zeroizing::new(smgw_text::SMGW_RECEIVER.to_vec())", inbox)
        self.assertIn("receiver_domain IS ?12", inbox)
        offline = source("offline_inbox.rs")
        self.assertIn('if receiver.domain.is_none() { digest.update(b"native-client-adapter-v1\\0"); }', offline)
        self.assertIn("receiver.domain.as_ref().map(|domain| domain.as_slice())", offline)
        for text in [inbox, offline]:
            self.assertNotIn("UPDATE app_voice_inbox SET receiver_domain", text)
        consumer = source("account_runtime.rs")
        self.assertIn("original_native_binding(&delivery.owner, &delivery.scope)", consumer)
        self.assertIn("context.with_native_receiver(&binding)", consumer)
        pending = consumer[consumer.index("async fn handle_message"):consumer.index("async fn consume_events")]
        self.assertIn("controller.phase() == Phase::HostingPending", pending)
        self.assertLess(pending.index("pending.push_back(delivery); return;"), pending.index("InboxScope::original"))
        self.assertIn("MAX_PENDING_MESSAGES: usize = 16", consumer)
        self.assertIn("MAX_PENDING_MESSAGE_BYTES: usize = 1024 * 1024", consumer)
    def test_config_requires_opt_in_and_keeps_capabilities_separate(self):
        config = source("account_config.rs")
        self.assertIn("#[serde(default)]\n    pub native_receivers: Option<super::native_receiver::NativeReceiverConfig>", config)
        self.assertIn('skip_serializing_if = "Option::is_none"', config)
        self.assertIn("with_native_receivers(p.native_receivers.as_ref())", config)
        self.assertIn("else if self.profile.call_control.is_none()", config)
        backend = source("runtime_backend.rs")
        self.assertIn("original_native_binding", backend)
        self.assertIn("(self.profile.call_incoming.is_some() || self.profile.native_receivers.is_some()) && self.profile.call_control.is_some()", backend)
        self.assertIn("ClientExKind::CheckAlive => false", source("rpc_client_ex.rs"))
    def test_session_revocation_retires_logical_media_while_only_original_control_writer_can_finish(self):
        scope = source("rpc_pending.rs")
        revoked = scope[scope.index("pub(super) fn mark_session_revoked"):scope.index("pub(super) fn matches")]
        self.assertIn("self.1.lock()", revoked)
        self.assertIn("self.3.store(true, Ordering::Release); self.2.notify_waiters()", revoked)
        self.assertIn("if !self.is_open() || self.session_revoked()", revoked)
        media = source("call_control.rs")
        self.assertIn("self.scope.is_open()&&!self.scope.session_revoked()&&self.identity.live.load", media)
        self.assertIn("self.scope.with_io_gate(||if !self.scope.session_revoked()", media)
        self.assertIn("self.scope.session_ended()", media)
        self.assertIn("call_session_revocation_tests.rs", source("call_control_tests.rs"))
        self.assertNotIn("#[ignore]", source("call_session_revocation_tests.rs"))

    def test_all_new_suites_run_on_both_workflows_with_original_filters_and_no_ignored_cases(self):
        for name in ["build-release.yml", "beta-validation.yml"]:
            text = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            for module in ["native_receiver::tests", "rpc_incoming::tests", "rpc_client_ex::tests", "runtime_backend::common_tests", "app_login::tests", "account_runtime::tests"]:
                self.assertIn("connectivity::app_voice::providers::jegotrip::" + module, text)
            self.assertIn("--test-threads=1", text); self.assertIn("Empty regression filter", text)
        for name in ["native_receiver_tests.rs", "native_receiver_protocol_tests.rs", "native_receiver_runtime_tests.rs", "native_receiver_account_tests.rs"]:
            self.assertNotIn("#[ignore]", source(name))
        self.assertIn("native_receiver_protocol_tests.rs", source("native_receiver_tests.rs"))
        self.assertIn("native_receiver_runtime_tests.rs", source("runtime_common_backend_tests.rs"))
        self.assertIn("native_receiver_account_tests.rs", source("native_receiver_runtime_tests.rs"))

if __name__ == "__main__": unittest.main()
