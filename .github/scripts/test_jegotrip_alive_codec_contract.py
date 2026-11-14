"""Passive AccountAlive source contracts; never execute SDK/Rust/network peers."""
from pathlib import Path
import re
import unittest
ROOT = Path(__file__).resolve().parents[2]
MODULE = ROOT / "backend/src/connectivity/app_voice/providers/jegotrip"
def source(name): return (MODULE / name).read_text(encoding="utf-8")
class PassiveAliveContracts(unittest.TestCase):
    def test_exact_type_zero_version_list_arguments_and_shared_footer(self):
        text = source("rpc_alive.rs")
        self.assertIn('"registerAlive.AccountAlive.Account"', text)
        for part in ["Writer::versioned()?", "Reader::versioned(bytes)?", "writer.bytes(arguments.user_id)?", "writer.bytes(arguments.selected_session_id)?", "writer.u64(arguments.request_timestamp_ms)?", "writer.u32(arguments.interval_seconds)?", "read_footer(&mut reader)?", "reader.finish()?"]:
            self.assertIn(part, text)
        self.assertIn("MAX_FIELD_BYTES", text); self.assertIn("MAX_STREAM_BYTES", text)
        self.assertIn("checked_add", text)
    def test_decoded_acceptance_is_data_not_an_owned_registration_or_capability(self):
        text = source("rpc_alive.rs")
        self.assertIn("struct RawRegisterAliveResult", text)
        self.assertIn("returned_acceptance", text)
        self.assertIn("if code != 0 { return Err(CodecError::RemoteFailure); }", text)
        self.assertIn("if !footer.reason.is_empty()", text)
        for forbidden in ["OwnedAlive", "is_registered", "RpcConnectionScope", "AttemptOwner", "TlsAuthenticated", "Serialize", "Callback::", "AliveState::"]:
            self.assertNotIn(forbidden, text)
    def test_codec_does_not_create_transport_clock_retry_or_credential_sources(self):
        text = source("rpc_alive.rs")
        for forbidden in ["tokio::", "std::net", "std::fs", "std::process", "SystemTime", "Instant", "reqwest", "TcpStream", "unsafe", "extern ", "dlopen", "ConfigDecodeKey", "RecordSecurity"]:
            self.assertNotIn(forbidden, text)
        self.assertIn("RegisterAliveArguments(<redacted>)", text)
        self.assertIn("RawRegisterAliveResult(<redacted>)", text)
    def test_runtime_consumer_requires_original_binding_and_codec_alone_cannot_upgrade_check_alive(self):
        allowed={"mod.rs", "rpc_alive.rs", "rpc_alive_tests.rs", "alive_session.rs", "runtime_alive_tests.rs", "alive_session_tests.rs", "alive_io_tests.rs"}
        for path in MODULE.rglob("*.rs"):
            if path.name not in allowed:
                self.assertNotIn("rpc_alive", path.read_text(encoding="utf-8"), str(path.relative_to(ROOT)))
        self.assertIn("ClientExKind::CheckAlive => alive.0", source("rpc_client_ex.rs"))
        self.assertIn("map_or((false,0),|alive|alive.check(&self.owner,&self.scope,&self.token", source("rpc_client_ex.rs"))
        self.assertIn('state.phase!="registered"', source("alive_session.rs"))
        self.assertIn("else { Self::Unsupported }", source("runtime_provision.rs"))
        self.assertIn("pub(crate) mod rpc_alive;", source("mod.rs"))
    def test_independent_fixtures_and_authenticated_but_unregistered_negative_case_are_retained(self):
        text = source("rpc_alive_tests.rs")
        names = re.findall(r"#\[test\]\s*fn\s+(\w+)", text)
        self.assertEqual(len(names), 16); self.assertEqual(len(set(names)), 16)
        self.assertIn("let expected = [0, 0, 1, 0, 0", text)
        self.assertIn("decoded_true_result_cannot_change_check_alive_even_after_matching_sdk_login", text)
        self.assertIn("assert_eq!(checked.result.expose_to_transport(), [0; 10])", text)
        self.assertIn("for length in 0..bytes.len()", text)
        for forbidden in ["#[ignore]", "TcpListener", "Command::new", "time::sleep"]: self.assertNotIn(forbidden, text)
    def test_both_existing_workflows_select_the_new_pure_suite(self):
        for name in ["build-release.yml", "beta-validation.yml"]:
            text = (ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            self.assertEqual(text.count("connectivity::app_voice::providers::jegotrip::rpc_alive::tests"), 1)
            self.assertIn("connectivity::app_voice::providers::jegotrip::rpc_client_ex::tests", text)
            self.assertIn("Empty regression filter", text); self.assertIn("--test-threads=1", text)
if __name__ == "__main__": unittest.main()
