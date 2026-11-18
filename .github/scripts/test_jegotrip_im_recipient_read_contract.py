"""Supplemental fences: original nonUID resolution is not raw-data WRITE trust."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def text(name):return(P/name).read_text(encoding='utf-8')
class RecipientReadContracts(unittest.TestCase):
 def test_recipient_intent_has_no_uid_shortcut_and_hash_is_private_nullable_history(self):
  s=text('im_user_read_journal.rs');self.assertIn('parsed.kind()==super::super::user_entry_data::AidKind::Uid',s);self.assertIn('ADD COLUMN recipient_hash BLOB',s);self.assertIn('recipient_hash IS NULL OR',s);self.assertIn('saved_recipient.as_deref()!=current_recipient',s)
 def test_owned_recipient_reply_must_match_original_action_and_local_body_uid(self):
  s=text('im_user_read_journal.rs');self.assertIn('reply.resolved_uid_for(action)!=Some(&self.box_id[4..])',s);self.assertIn('self.verify_message(&tx,true)',s);self.assertIn('recipient_reply:Option<Arc<',s)
 def test_alias_cannot_admit_from_group_bool_or_current_lookup_projection_alone(self):
  s=text('im_user_read_journal.rs').split('fn admit',1)[1];self.assertIn('self.action.recipient().is_some()',s);self.assertIn('resolved.resolved_uid_for(action)',s);self.assertIn('reply.request().query_arc()',s);self.assertIn('status.accepted',s)
 def test_runtime_resolves_before_original_group_read_and_permit_write_without_relogin(self):
  s=text('runtime_user_read.rs');self.assertLess(s.index('user_read_recipient_reply'),s.index('commands.read_user_target'));self.assertLess(s.index('context.confirm_recipient'),s.index('context.admit'));self.assertIn('Box::pin(async',s);r=text('runtime_user_read_recipient.rs');self.assertIn('commands.original_user_entry_auth',r);self.assertIn('resolve_original(action.clone()',r);self.assertNotIn('loginSession.',r);self.assertNotIn('tokio::spawn',r)
 def test_authenticated_strict_api_accepts_only_explicit_original_intent(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('pub struct ImUserReadRecipientRequest{expected_revision:u64,nonce:String,recipient:String,consent:bool}',s);self.assertIn('valid_user_read_recipient(&request.recipient)',s);self.assertIn('zeroize(&mut self.recipient)',s)
 def test_ui_nonce_follows_current_baseline_intent_and_both_operations_share_same_serial_lock(self):
  s=(R/'frontend/src/pages/sim/AppImMessagesPane.tsx').read_text(encoding='utf-8');self.assertIn('const markRead = async (row: ImHistoryRow, alias?: string)',s);self.assertIn('userReadBaselineReady',s);self.assertIn('marking.current = true',s);self.assertIn('markAppImUserReadViaRecipient',s);self.assertIn('userReadKnownUnknown',s);self.assertNotIn('dangerouslySetInnerHTML',s);self.assertNotIn("'0'.repeat(32)",s)
if __name__=='__main__':unittest.main()
