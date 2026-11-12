"""Static ClientEx source contracts. Never run Rust, SDK, peers or devices."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
MODULE = ROOT / "backend/src/connectivity/app_voice/providers/jegotrip"

def source(name):
    return (MODULE / name).read_text(encoding="utf-8")

class ClientExContracts(unittest.TestCase):
    def test_exact_versioned_control_shapes_and_independent_alive_registration(self):
        codec = source("rpc_client_ex.rs")
        for method in ["checkAlive.ClientEx.Client", "notifyCalls.ClientEx.Client", "notifyDisconnect.ClientEx.Client"]:
            self.assertIn('"' + method + '"', codec)
        self.assertIn("Reader::versioned(arguments)?", codec)
        self.assertIn("reader.u64()?", codec)
        self.assertIn("disconnect_reason(reader.bytes()?)", codec)
        self.assertIn("writer.u32(alive_time)?", codec)
        self.assertIn('writer.bytes(b"")?', codec)
        self.assertIn("ClientExKind::CheckAlive => false", codec)
        self.assertIn("if !properties.is_empty()", codec)
        self.assertNotIn("Callback::Channels", codec)
        self.assertNotIn("AccountAliveState::Disabled => true", codec)

    def test_original_token_and_binding_are_captured_from_matched_arc_not_configuration(self):
        arc = source("arc_session.rs")
        self.assertIn("&authentication.token", arc)
        self.assertIn("self.phase() != ArcPhase::Authenticated || self.connection.session_revoked()", arc)
        protocol = source("arc_protocol.rs")
        self.assertIn("self.incoming.as_ref().ok_or(CodecError::InvalidField)?.binding()", protocol)
        self.assertIn("dispatcher.matches_binding(&binding)", protocol)
        codec = source("rpc_client_ex.rs")
        self.assertIn('property(b"id") != Some(self.object.as_slice())', codec)
        self.assertIn('property(b"domain") != Some(self.domain.as_slice())', codec)
        self.assertIn('property(b"token") == Some(self.token.as_slice())', codec)
        self.assertIn("self.scope.mark_session_revoked()", codec)
        self.assertIn("Zeroizing<Vec<u8>>", codec)
        self.assertNotIn("derive(Serialize", codec)

    def test_prearming_is_data_only_and_activates_on_matched_login_before_next_packet(self):
        protocol = source("arc_protocol.rs")
        arm = protocol[protocol.index("pub(super) fn arm_client_ex"):protocol.index("pub(super) fn install_client_ex")]
        self.assertIn("ArcPhase::Initial", arm)
        self.assertIn("ClientExRegistration::capture", arm)
        self.assertNotIn("token", arm.split("///")[0])
        reply = protocol[protocol.index("Ok(Some(reply)) =>"):protocol.index("Ok(None) => Ok(false)")]
        self.assertLess(reply.index("client_ex_dispatcher(registration.binding())"), reply.index("deliver(ProtocolEvent::Reply(reply))"))
        backend = source("runtime_backend.rs")
        self.assertEqual(backend.count("self.arm_original_client_ex(&attempt, &commands).await?"), 2)
        self.assertIn("commands.arm_client_ex(&attempt.owner, &attempt.scope, binding.borrowed())", backend)

    def test_revocation_is_sticky_before_io_and_not_reclassified_by_queue_failure(self):
        scope = source("rpc_pending.rs")
        self.assertIn("pub(super) fn session_revoked", scope)
        self.assertIn("self.3.store(true, Ordering::Release)", scope)
        actor = source("transport_actor.rs")
        self.assertIn("if self.scope.session_revoked() { RuntimeError::SessionRevoked } else { result }", actor)
        self.assertIn("if self.scope.session_revoked() { return Some(super::session::BackendFailure::SessionRevoked); }", actor)
        self.assertIn("&& !self.scope.session_revoked() { Some(self.events)", actor)
        self.assertIn("visible.clear()", actor)
        backend = source("runtime_backend.rs")
        self.assertIn("if result.is_err() && self.original(owner).is_ok_and", backend)
        self.assertIn("BackendFailure::SessionRevoked } else { reason }", backend)
        self.assertIn("attempt.scope.session_revoked() && attempt.scope.is_open()", backend)
        self.assertIn("terminal_after_runtime_close", backend)

    def test_control_hints_keep_original_polls_budgets_and_durable_message_ack_separate(self):
        backend = source("runtime_backend.rs")
        self.assertIn("const POLL_CADENCE: Duration = Duration::from_millis(250)", backend)
        self.assertIn("let mut poll_requested = false", backend)
        self.assertIn("last.elapsed() >= POLL_CADENCE", backend)
        self.assertIn("attempt.poll_work.lock().await", backend)
        self.assertIn("borrowed(&profile.poll_routing)", backend)
        self.assertNotIn("timer.reset_immediately", backend)
        self.assertNotIn("timer.reset_at", backend)
        offline = source("offline_backend.rs")
        self.assertIn("scheduler.complete_server_request(incoming.ticket(), 0, action.result.expose_to_transport())", offline)
        self.assertIn("Ok(false) => { processing_error = Some(CodecError::InvalidField.into())", offline)
        self.assertIn("if revoked { break; }", offline)
        self.assertNotIn("commit_offline", source("rpc_client_ex.rs"))
        self.assertNotIn("insert_sms", source("rpc_client_ex.rs"))

    def test_optional_private_configuration_keeps_old_json_and_needs_original_receiver(self):
        config = source("account_config.rs")
        self.assertIn("#[serde(default)]\n    pub client_ex: bool", config)
        self.assertIn('skip_serializing_if = "offline_disabled")]', config)
        self.assertIn("with_client_ex(p.client_ex)", config)
        backend = source("runtime_backend.rs")
        self.assertIn("offline_reception: false, client_ex: false", backend)
        self.assertIn("self.incoming.as_ref().ok_or(ProfileError::InvalidConfiguration)?", backend)
        self.assertNotIn("DEFAULT_CLIENT_OBJECT", backend)
        self.assertNotIn("DEFAULT_RECEIVER_DOMAIN", backend)

    def test_revoked_session_is_a_nonretryable_failure_and_frontend_does_not_echo_reason(self):
        session = source("session.rs")
        self.assertIn("BackendFailure::SessionRevoked | BackendFailure::HostingRejected => Phase::AuthenticationFailed", session)
        account = source("account_runtime.rs")
        self.assertIn('BackendFailure::SessionRevoked => "app_account_session_revoked"', account)
        retryable = account[account.index("fn reconnect_retryable"):account.index("fn schedule_reconnect")]
        self.assertNotIn("session_revoked", retryable)
        frontend = (ROOT / "frontend/src/policies/appLogin.ts").read_text(encoding="utf-8")
        self.assertIn("app_account_session_revoked:", frontend)
        self.assertIn("不会自动重连争抢", frontend)
        self.assertIn("Never echo an unrecognized service error", frontend)

    def test_new_regressions_are_selected_in_both_actions_without_ignored_or_live_operations(self):
        for name in ["build-release.yml", "beta-validation.yml"]:
            workflow = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            for module in ["rpc_client_ex::tests", "runtime_backend::tests", "runtime_backend::common_tests", "transport_actor::tests", "account_runtime::tests",
                           "rpc_incoming::tests", "rpc_offline::tests", "app_login::tests"]:
                self.assertIn("connectivity::app_voice::providers::jegotrip::" + module, workflow)
            self.assertIn("Empty regression filter", workflow)
            self.assertIn("--test-threads=1", workflow)
        for name in ["rpc_client_ex_tests.rs", "runtime_client_ex_tests.rs", "runtime_common_client_ex_tests.rs", "transport_client_ex_tests.rs", "account_client_ex_tests.rs"]:
            text = source(name)
            for forbidden in ["#[ignore]", 'Command::new("adb")', "verificationTrustfulDevice", "dlopen", "time::sleep(Duration::from_secs(60))"]:
                self.assertNotIn(forbidden, text)
        for parent, child in [("runtime_backend_tests.rs", "runtime_client_ex_tests.rs"), ("runtime_common_backend_tests.rs", "runtime_common_client_ex_tests.rs"),
                              ("transport_actor_tests.rs", "transport_client_ex_tests.rs"), ("account_runtime_tests.rs", "account_client_ex_tests.rs")]:
            self.assertIn(child, source(parent))

if __name__ == "__main__":
    unittest.main()
