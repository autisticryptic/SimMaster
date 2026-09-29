"""Static wiring guards supplement Actions Rust/TypeScript behavioural tests."""
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


class AutomationDialBoundaryTests(unittest.TestCase):
    def test_dial_outcome_keeps_the_cause_for_logs_and_notifications(self):
        text = (ROOT / 'backend/src/services/automation/scheduler.rs').read_text()
        self.assertIn('task_outcome(result.ok(), timeout_seconds, &task.action)', text)
        self.assertIn('handler.execute_report(app, &params)', text)
        self.assertIn('Some(Ok(report)) => ("success", report.detail())', text)
        self.assertIn('dial_failure_summary(&error)', text)
        self.assertIn('error.chain().take(8)', text)
        self.assertIn('message: detail.clone()', text)
        self.assertIn('"Automation execution failed"', text)
        self.assertIn('[number-redacted]', text)
        self.assertIn('执行超时 (超过{timeout_seconds}秒限制)', text)

    def test_call_cost_and_radio_guards_are_not_removed_for_automation(self):
        text = (ROOT / 'backend/src/api/handlers.rs').read_text()
        call = text[text.index('pub(crate) async fn start_call_for_automation('):text.index('pub(crate) async fn hangup_call_for_automation(')]
        self.assertIn('initially_vowifi_only', call)
        cost = text[text.index('fn cellular_call_cost_rule('):text.index('pub(crate) async fn start_owned_automation_call(')]
        self.assertIn('voice_vowifi_only_required', cost)
        self.assertIn('voice_registered_home_required', cost)
        self.assertGreaterEqual(call.count('admit_cellular_call_cost('), 3)
        self.assertIn('native_call_radio_admission(', call)
        self.assertIn('{ims_error};modem_call_failed:{error}', call)
        handler = (ROOT / 'backend/src/services/automation/tasks/dial_call.rs').read_text()
        self.assertIn('start_owned_automation_call(&app, &target.line_id, &phone, flag)', handler)
        self.assertIn('run_owned_call(', handler)
        self.assertIn('CancelCallTask', handler)
        self.assertNotIn('hangup_call_on_modem', handler)
        self.assertNotIn('set_trunk_vowifi_only', handler)

    def test_failed_config_save_does_not_close_dialog_or_enable_scheduler(self):
        helper = (ROOT / 'frontend/src/utils/automationConfig.ts').read_text()
        self.assertIn("response.status !== 'ok'", helper)
        self.assertIn('throw new Error', helper)
        page = (ROOT / 'frontend/src/pages/AutomationCenter.tsx').read_text()
        update = page[page.index('const updateConfig ='):page.index('// Toggle single task enabled')]
        self.assertIn('await persistAutomationConfig', update)
        self.assertNotIn('catch', update)
        self.assertNotIn('enabled: true', update)
        dialog = (ROOT / 'frontend/src/pages/automation/AutomationTaskDialog.tsx').read_text()
        self.assertLess(dialog.index('await onSave(newTask)'), dialog.index('onClose()', dialog.index('await onSave(newTask)')))

    def test_dial_classification_uses_scoped_evidence_not_status_alone(self):
        classifier = (ROOT / 'backend/src/services/automation/tasks/dial_outcome.rs').read_text()
        for guard in ('diagnostic.network_response', 'diagnostic.initial_invite', 'diagnostic.sip_status != *status',
                      'self.ringing', 'VoiceCallObservation::EvidenceLost', 'OperatorEvent::AttemptChanged'):
            self.assertIn(guard, classifier)
        handler = (ROOT / 'backend/src/services/automation/tasks/dial_call.rs').read_text()
        self.assertIn('subscribe_call_events()', handler)
        self.assertNotIn('subscribe_commands()', handler)
        self.assertIn('events.try_recv()', handler)
        self.assertIn('hangup(call_id).await.context("自动挂机失败")?', handler)
        for path in ('cellular_ims/live.rs', 'vowifi/operator.rs'):
            producer = (ROOT / 'backend/src/connectivity/modems/ims' / path).read_text()
            for fact in ('VoiceCallObservation::RemoteRinging', 'VoiceCallObservation::RemoteAnswered',
                         'VoiceCallObservation::RemoteEnded', '.for_initial_invite('):
                self.assertIn(fact, producer)

    def test_observer_metadata_does_not_rewrite_call_history_or_bridge_frames(self):
        link = (ROOT / 'backend/src/services/trunk/operator.rs').read_text()
        self.assertIn('self.inner.call_observations.send(event.clone())', link)
        self.assertIn('OperatorEvent::Observation { .. } | OperatorEvent::AttemptChanged { .. }', link)
        self.assertIn('return; // No SIP bridge, call-history or media side effects.', link)
        router = (ROOT / 'backend/src/services/trunk/access_router.rs').read_text()
        self.assertIn('VoiceCallObservation::EvidenceLost', router)
        self.assertIn('VoiceCallObservation::LocalCancelled', router)
        self.assertLess(router.index('// Invalidate evidence BEFORE'), router.index('let task = admissions.spawn'))

    def test_successful_task_cards_keep_peer_outcome_visible(self):
        page = (ROOT / 'frontend/src/pages/automation/AutomationTaskCard.tsx').read_text()
        self.assertIn('automationOutcomeLabel(latestLog.status, latestLog.detail)', page)
        self.assertIn('tests/automationOutcome.test.ts', (ROOT / 'frontend/package.json').read_text())

    def test_new_regressions_are_in_both_actions(self):
        for workflow in ('build-release.yml', 'beta-validation.yml'):
            text = (ROOT / '.github/workflows' / workflow).read_text()
            for suite in ('services::automation::scheduler::tests', 'services::automation::target::tests', 'services::automation::tasks::dial_call::tests', 'services::automation::tasks::dial_outcome::tests', 'connectivity::core::ims_failure::tests', 'vowifi::operator::tests::two_dialogs_keep_progress_media_dtmf_and_reinvite_state_independent'):
                self.assertIn(suite, text)
        package = (ROOT / 'frontend/package.json').read_text()
        self.assertIn('tests/automationConfig.test.ts', package)


if __name__ == '__main__':
    unittest.main()
