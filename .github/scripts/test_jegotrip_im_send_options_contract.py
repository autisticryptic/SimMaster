"""Supplemental source fences for original outgoing notification DATA only."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
class SendOptions(unittest.TestCase):
 def test_exact_original_online_key_is_not_generic_namespace_or_boolean_permission(self):
  s=(P/'im_send_data.rs').read_text(encoding='utf-8');self.assertIn('notify&&p.key!=b"OnlineMsg"',s);self.assertIn('b"Notify.ImUserData"',s);self.assertIn('b"Notify.ImdnId"',s);self.assertNotIn('parse::<bool>',s)
 def test_original_defaults_copy_only_exact_present_message_string_fields(self):
  s=(P/'im_send_data.rs').read_text(encoding='utf-8');self.assertIn('message.parameters.iter().find(|p|p.key==source)',s);self.assertIn('default_user_send_notifications',s);self.assertIn('digest.filter(|value|!value.is_empty()).unwrap_or(message.content)',s);self.assertNotIn('std::fs',s)
 def test_digest_is_zeroizing_data_not_identity_or_key_and_conflicting_overrides_fail(self):
  s=(P/'im_outbox.rs').read_text(encoding='utf-8');self.assertIn('notification_digest:Option<Zeroizing<Vec<u8>>>',s);self.assertIn('self.notifications.is_some()',s);self.assertIn('with_notification_digest',s);s=(P/'im_send_data.rs').read_text(encoding='utf-8');self.assertIn('notifications.is_some()&&digest.is_some()',s);self.assertIn('notifications.unwrap_or(&defaults)',s)
 def test_missing_digest_keeps_legacy_fingerprint_new_digest_is_bound_to_intent(self):
  s=(P/'im_outbox.rs').read_text(encoding='utf-8');self.assertIn('if let Some(value)=&self.notification_digest{h.update(b"original-im-notification-digest-v1',s);self.assertIn('message.fingerprint()',s);self.assertIn('self.notification_digest.as_ref().map_or(0,|value|value.capacity())',s)
 def test_http_optional_digest_wipes_and_stays_before_original_manager_admission(self):
  s=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8');self.assertIn('notification_digest:Option<String>',s);self.assertIn('if let Some(s)=&mut self.notification_digest{zeroize::Zeroize::zeroize(s);}',s);self.assertLess(s.index('body.with_notification_digest(value)'),s.index('manager.send_im_message_with_admission'))
 def test_new_cases_are_additive_leafs_and_no_stack_filter_retry_or_skip_expansion(self):
  for file,name in [('im_send_data_tests.rs','im_send_option_data_tests.rs'),('im_send_protocol_tests.rs','im_send_option_protocol_tests.rs'),('im_send_account_tests.rs','im_send_option_account_tests.rs')]:self.assertIn(name,(P/file).read_text(encoding='utf-8'))
  for file in ['im_send_option_data_tests.rs','im_send_option_protocol_tests.rs','im_send_option_account_tests.rs']:
   s=(P/file).read_text(encoding='utf-8');self.assertNotIn('#[ignore',s);self.assertNotIn('RUST_MIN_STACK',s);self.assertNotIn('stack_size(',s)
 def test_legacy_capture_shape_cannot_be_preempted_by_expanded_fresh_defaults(self):
  s=(P/'im_outbox.rs').read_text(encoding='utf-8');self.assertIn('validate_user_send_capture(message,notifications)',s);s=(P/'im_send_data.rs').read_text(encoding='utf-8');self.assertIn('let capture:&[RpcProperty',s);self.assertIn('Some(capture)',s);self.assertIn('if notifications.is_none(){default_user_send_notifications',s)
if __name__=='__main__':unittest.main()
