"""Static native-offline source contracts; never execute Rust, SDK, peers or devices."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
MOD = ROOT / "backend/src/connectivity/app_voice/providers/jegotrip"


def source(name):
    return (MOD / name).read_text(encoding="utf-8")


class OfflineReceptionContracts(unittest.TestCase):
    def test_native_codec_is_exact_separate_signed64_and_does_not_guess_json(self):
        codec = source("rpc_offline.rs")
        self.assertIn('"moreOfflineMessages.MessageReceiver.Client"', codec)
        self.assertIn('"getOfflineMessages.AccountEntry.Account"', codec)
        self.assertIn("pub const OFFLINE_LIMIT: usize = 100", codec)
        self.assertIn("writer.i64(position)?", codec)
        self.assertIn("reader.i64()?", codec)
        nested = codec[codec.index("pub fn decode_offline_message"):codec.index("pub struct OwnedOfflinePage")]
        self.assertIn("Reader::new(bytes)?", nested)
        self.assertNotIn("Reader::versioned", nested)
        self.assertNotIn("serde_json::from", nested)
        self.assertIn("!footer.reason.is_empty()", codec)
        self.assertIn("records.sort_by_key", codec)

    def test_owned_page_is_from_matched_session_not_caller_relabelled_bytes(self):
        session = source("arc_session.rs")
        self.assertIn("PendingKind::Offline { position }", session)
        self.assertIn("OwnedOfflinePage::original(self.owner.clone(), self.connection.clone()", session)
        self.assertIn("&auth.user_id, &auth.selected_session_id", session)
        self.assertIn('metadata.push(RpcProperty { key: b"token", value: &auth.token })', session)
        self.assertIn("reject_auth_keys(routing)?", session)
        self.assertIn("self.pending.values().any(|pending| matches!(pending.kind, PendingKind::Offline", session)
        codec = source("rpc_offline.rs")
        self.assertIn("pub(super) fn original", codec)
        self.assertIn("self.scope.matches(scope) && scope.is_open()", codec)
        self.assertNotIn("derive(Serialize", codec)

    def test_private_option_is_disabled_by_default_and_needs_original_receiver(self):
        config = source("account_config.rs")
        self.assertIn("#[serde(default)]\n    pub offline_reception: bool", config)
        self.assertIn('skip_serializing_if = "offline_disabled"', config)
        self.assertIn("with_offline_reception(p.offline_reception)", config)
        backend = source("runtime_backend.rs")
        self.assertIn("incoming, offline_reception: false", backend)
        self.assertIn("enabled && self.incoming.is_none()", backend)
        self.assertIn("state.logout_started || state.unregister_started", backend)
        self.assertIn("!self.profile.offline_reception", backend)

    def test_durable_cursor_and_sms_are_committed_before_any_next_fetch(self):
        inbox = source("offline_inbox.rs")
        self.assertIn("account_runs::require_live", inbox)
        self.assertIn("account_inbox::require_account", inbox)
        self.assertIn("account_sms::promote_new(&tx", inbox)
        self.assertIn("ON CONFLICT(line_id,revision,identity_key)", inbox)
        self.assertIn("record.position < current", inbox)
        self.assertIn("result.records.last().map_or(result.server_position + 1", inbox)
        self.assertLess(inbox.index("tx.commit().map_err(|_| STORAGE)?;\n            Ok((next, events))"), inbox.index("Ok(CommittedOfflineCursor"))
        consumer = source("offline_reception.rs")
        self.assertIn("context.commit_offline(&run.database, &page)", consumer)
        self.assertIn("proof.matches(&owner, &scope)", consumer)
        self.assertIn("state.cursor = proof.next_position()", consumer)
        self.assertIn("app_account_offline_no_progress", consumer)
        self.assertIn("state.hint_epoch != hint_epoch", consumer)
        self.assertIn("controller.phase() != Phase::Hosted", consumer)

    def test_unknown_raw_and_deleted_sms_are_preserved_without_replay_or_fake_ack(self):
        inbox = source("offline_inbox.rs")
        quarantine = inbox[inbox.index("fn quarantine("):inbox.index("fn compact_prior(")]
        self.assertIn("page.raw()", quarantine)
        self.assertNotIn("INSERT INTO app_voice_offline_cursors", quarantine)
        self.assertNotIn("account_sms::promote", quarantine)
        self.assertIn("envelope_retained=0", inbox)
        self.assertIn("p.envelope_retained=0", inbox)
        self.assertIn("offline_envelope=NULL", source("account_sms.rs"))
        self.assertIn("offline_envelope BLOB", source("account_inbox.rs"))
        self.assertIn("MAX_TOTAL_BYTES", inbox)
        self.assertIn("FROM app_voice_offline_quarantine", source("account_inbox.rs"))

    def test_migration_is_transactional_preserves_autoincrement_and_refuses_unknown_extras(self):
        schema = source("offline_inbox.rs")
        self.assertIn("TransactionBehavior::Immediate", schema)
        self.assertIn("columns.iter().any", schema)
        self.assertIn("if custom { return Err(STORAGE); }", schema)
        self.assertIn("CREATE TEMP TABLE jegotrip_inbox_migration", schema)
        self.assertIn("INSERT INTO app_voice_inbox({COLUMNS}) SELECT {COLUMNS}", schema)
        self.assertIn("sqlite_sequence SET seq=MAX(seq,?1)", schema)
        self.assertIn("CREATE UNIQUE INDEX IF NOT EXISTS idx_app_voice_inbox_original_sequence", schema)
        self.assertNotIn("DROP TABLE sms_messages", schema)
        self.assertNotIn("PRAGMA writable_schema", schema)

    def test_new_and_existing_filters_keep_actions_only_and_fail_empty_or_failed_regressions(self):
        for filename in ["build-release.yml", "beta-validation.yml"]:
            workflow = (ROOT / ".github/workflows" / filename).read_text(encoding="utf-8")
            for module in ["rpc_offline::tests", "arc_session::offline_tests", "offline_inbox::tests", "account_runtime::offline_tests"]:
                self.assertIn("connectivity::app_voice::providers::jegotrip::" + module, workflow)
            for original in ["app_login::tests", "account_runtime::tests", "account_sms::tests", "runtime_backend::common_tests"]:
                self.assertIn("connectivity::app_voice::providers::jegotrip::" + original, workflow)
            self.assertIn("Empty regression filter", workflow)
            self.assertIn("--test-threads=1", workflow)
        for filename in ["rpc_offline_tests.rs", "rpc_offline_session_tests.rs", "offline_inbox_tests.rs", "account_offline_tests.rs", "runtime_common_offline_tests.rs"]:
            text = source(filename)
            self.assertNotIn("#[ignore]", text)
            self.assertNotIn("verificationTrustfulDevice", text)
            self.assertNotIn("Command::new(\"adb\")", text)


if __name__ == "__main__":
    unittest.main()
