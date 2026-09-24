"""Guard wiring and fail-closed ordering; behavioral Rust tests run on Actions."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]
BACKEND = ROOT / "backend/src"


class NativeSmsInboxBoundaryTests(unittest.TestCase):
    def test_commit_and_delete_are_after_durable_staging_under_one_gate(self):
        source = (BACKEND / "hardware/cellular/backends/direct_sms.rs").read_text()
        capture = source.split("pub async fn capture_sms(", 1)[1].split("pub async fn send_sms_persisted(", 1)[0]
        self.assertLess(capture.index("operation.lock().await"), capture.index("bind_current_sim"))
        self.assertLess(capture.index("db.stage_native_pdu"), capture.index("Tool::AtDirectCommit"))
        self.assertLess(capture.index("db.stage_native_pdu"), capture.index("AT+CMGD="))
        self.assertIn("current.get(&index) == Some(&pdu)", capture)
        self.assertIn("pdu.sim_key != key", capture)
        self.assertIn("AT+CSMS?", capture)
        self.assertNotIn("AT+CSMS=", source)

    def test_new_inbox_is_initialized_and_native_listener_uses_it(self):
        db = (BACKEND / "platform/db.rs").read_text()
        self.assertIn('pub mod native_sms_inbox;', db)
        self.assertIn('native_sms_inbox::init(&conn)?;', db)
        source = (BACKEND / "services/messaging/sms_listener.rs").read_text()
        body = source.split("async fn maybe_scan_sms_paths(", 1)[1].split("async fn native_sms_events_pending(", 1)[0]
        self.assertLess(body.index("modem_sms_scan_allowed"), body.index("device.capture_sms"))
        self.assertIn("native_sms::consume_pending", body)
        self.assertIn("context.mt_sms.send", body)
        self.assertIn("sender.forward_sms", body)

    def test_final_dedup_is_sim_scoped_and_not_a_legacy_claim(self):
        source = (BACKEND / "platform/native_sms_inbox.rs").read_text()
        promote = source.split("pub fn promote_native_sms(", 1)[1].split("pub fn begin_native_sms_send(", 1)[0]
        self.assertIn("native_sms_received WHERE line_id=?1 AND sim_key=?2", promote)
        self.assertNotIn("claim_sms_dedup", promote)
        self.assertIn("let tx = conn.transaction()?", promote)
        self.assertLess(promote.index("insert_app_event_for_conn"), promote.index("tx.commit()?"))
        self.assertLess(promote.index("tx.commit()?"), promote.index("app_event_tx.send"))

    def test_status_reports_and_partial_sends_do_not_guess_success(self):
        source = (BACKEND / "platform/native_sms_inbox.rs").read_text()
        self.assertIn("successful == *total && status == 0", source)
        self.assertIn("candidates.as_slice()", source)
        self.assertIn("p.started_at-2 AND p.finished_at+120", source)
        self.assertIn("confirmed && parts != total", source)
        api = (BACKEND / "api/handlers.rs").read_text()
        self.assertIn("device.send_sms_persisted", api)
        self.assertIn('"submission_state": if result.confirmed', api)
        ui = (ROOT / "frontend/src/pages/SMS.tsx").read_text()
        self.assertIn("submission_state === 'unconfirmed'", ui)
        self.assertIn("msg.status === 'delivered'", ui)

    def test_new_behavioral_tests_are_selected_by_both_workflows(self):
        for workflow in ("beta-validation.yml", "build-release.yml"):
            source = (ROOT / ".github/workflows" / workflow).read_text()
            for test_filter in (
                "platform::db::native_sms_inbox::tests",
                "connectivity::core::sms_codec::tests",
                "connectivity::core::sms_codec::modem_tests",
                "hardware::cellular::backends::direct_sms::tests",
                "services::messaging::native_sms::tests",
            ):
                self.assertIn(test_filter, source)


if __name__ == "__main__":
    unittest.main()
