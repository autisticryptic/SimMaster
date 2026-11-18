"""Supplemental original nonUID manual position fences, not SDK/IO execution."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(n):return(P/n).read_text(encoding='utf-8')
class ManualRecipientContracts(unittest.TestCase):
 def test_alias_intent_is_nonuid_current_p2p_not_group_uidlocal_or_callerindex(self):
  s=src('im_read_status.rs');self.assertIn('fn original_with_recipient',s);self.assertIn('parsed.kind()==super::super::user_entry_data::AidKind::Uid',s);self.assertIn('target.span.box_bytes().starts_with(b"P2P/")',s);self.assertIn('UserEntryAction::original(owner.clone(),scope.clone(),gate.clone(),deadline,uri)',s)
 def test_only_private_owned_reply_with_same_purpose_and_target_can_confirm(self):
  s=src('im_read_status_recipient.rs');self.assertIn('reply:OwnedUserEntryReply',s);self.assertIn('reply.resolved_uid_for(action)!=Some(&self.target.span.box_bytes()[4..])',s);self.assertIn('self.target.validate_current(&tx)',s);self.assertIn('drop(current);self.require_recipient()',s)
 def test_recipient_hash_is_nullable_domain_separated_intent_data(self):
  s=src('im_read_status.rs');self.assertIn('PRAGMA table_info(app_voice_im_read_status_actions)',s);self.assertIn('recipient_hash IS NULL',s);self.assertIn('length(recipient_hash)=32',s);self.assertIn('recipient_hash IS ?12',s);s=src('im_read_status_recipient.rs');self.assertIn('original-im-read-status-recipient-v1',s);self.assertIn('saved_recipient.as_deref()!=recipient_hash',s)
 def test_metadata_nonce_and_unknown_preflight_precede_any_new_lookup(self):
  s=src('runtime_read_status.rs');self.assertLess(s.index('recipient_preflight()?'),s.index('read_status_recipient_reply(recipient)'));self.assertLess(s.index('confirm_recipient(matched)'),s.index('begin_read_status('));s=src('im_read_status_recipient.rs');self.assertIn('state=0) OR EXISTS',s);self.assertIn('self.same_nonce(&tx)',s);self.assertNotIn('DELETE FROM',s)
 def test_manual_admission_still_requires_original_resolved_result_not_hash_or_uidbool(self):
  s=src('im_read_status.rs');self.assertIn('self.require_recipient()?;',s);self.assertIn('recipient_hash.as_ref().map(|h|h.as_slice())',s);self.assertIn('self.same_nonce(conn)',s)
 def test_lookup_uses_current_sdk_auth_original_auxbook_router_and_original_agent_no_new_task(self):
  s=src('runtime_read_status_recipient.rs');self.assertIn('original_user_entry_auth(who,&parent)',s);self.assertIn('attempt.user_entries.acquire',s);self.assertIn('resolve_original(action.clone(),auth,target,&lease,&common.router',s);self.assertNotIn('tokio::spawn',s);self.assertNotIn('user_read_enabled',s);self.assertNotIn('allow_legacy_stcp',s)
 def test_account_leaves_controller_before_owned_io_and_rechecks_current_run(self):
  s=src('account_read_status.rs');self.assertIn('ReadStatusAction::original_with_recipient',s);self.assertIn('drop(controller);let result=run.backend.set_read_status(action).await?;',s);self.assertIn('run.configuration_current()?',s)
 def test_strict_private_api_drops_recipient_and_no_caller_scope_token_or_grant(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('pub struct ImReadStatusRecipientRequest{expected_revision:u64,expected_index:String,nonce:String,recipient:String,consent:bool}',s);self.assertIn('zeroize(&mut self.recipient)',s);self.assertIn('valid_user_read_recipient(&request.recipient)',s)
 def test_ui_keeps_shared_unknown_generation_baseline_and_no_write_lookup_errors_distinct(self):
  s=(R/'frontend/src/pages/sim/AppImReadStatusPane.tsx').read_text(encoding='utf-8');self.assertIn('readStatusBaselineReady(loaded, context, current.current.generation)',s);self.assertIn('readStatusRecipientNoWriteError(error)',s);self.assertIn('target.kind !== \'p2p\'',s);self.assertIn('writing.current || unknown',s);self.assertIn('setAppImReadStatusViaRecipient',s)
 def test_own_suites_are_additive_under_original_filters_and_keep_join_assertions(self):
  self.assertIn('im_read_status_recipient_protocol_tests.rs',src('im_read_status_protocol_tests.rs'));self.assertIn('im_read_status_recipient_account_tests.rs',src('user_entry_common_tests.rs'));s=src('im_read_status_recipient_account_tests.rs');self.assertIn('sdk.await.unwrap();resolver.await.unwrap();relay.await.unwrap()',s);self.assertIn('recipient_hash=NULL',s);self.assertIn('ownership_pending',s);self.assertNotIn('#[ignore',s)
 def test_current_run_resolution_claim_precedes_lookup_and_never_reopens_nonce(self):
  s=src('runtime_read_status.rs');self.assertLess(s.index('claim_recipient_resolution()?'),s.index('read_status_recipient_reply(recipient)'));s=src('im_read_status_recipient.rs');self.assertIn('original-manual-recipient-inflight-v1',s);self.assertIn('entry.fingerprint!=fingerprint',s);self.assertIn('app_account_im_read_status_resolution_inflight',s);self.assertIn('entries.len()>=MAX_LOCAL as usize',s);self.assertIn('entry.active=false',s)
 def test_stale_recipient_target_is_guaranteed_before_admission_and_invalidates_ui_baseline(self):
  s=src('im_read_status_recipient.rs');self.assertIn('app_account_im_read_status_recipient_target_stale',s);self.assertIn('e==SCOPE||e==STATUS_SCOPE',s);s=(R/'frontend/src/policies/imReadStatus.ts').read_text(encoding='utf-8');self.assertIn('app_account_im_read_status_recipient_target_stale',s);self.assertNotIn("'app_account_im_read_status_scope_invalid'",s);self.assertIn('scenario(7)',src('im_read_status_recipient_account_tests.rs'))
 def test_default_and_alias_runtime_claim_same_nonce_before_lookup_or_write(self):
  s=src('runtime_read_status.rs');self.assertIn('let _resolution=action.claim_recipient_resolution()?;',s);self.assertLess(s.index('claim_recipient_resolution()?'),s.index('begin_read_status('));self.assertIn('Some("app_account_im_read_status_nonce_conflict")',src('im_read_status_recipient_account_tests.rs'));self.assertIn('pending_alias_and_default_intent_share_original_nonce_exclusion',src('im_read_status_recipient_protocol_tests.rs'))
 def test_scope_and_timeout_after_admission_are_never_reported_as_prewrite_lookup_failure(self):
  s=src('runtime_read_status.rs');self.assertIn('action.write_admission_observed(){"app_account_im_read_status_uncertain"}',s);self.assertIn('!action.write_admission_observed(){"app_account_im_read_status_recipient_unconfirmed"}',s);self.assertIn('write_admitted.store(true,Ordering::SeqCst)',src('im_read_status.rs'))
if __name__=='__main__':unittest.main()
