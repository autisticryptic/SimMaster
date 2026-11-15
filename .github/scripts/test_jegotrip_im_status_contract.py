"""Supplemental original Group IM status DATA contracts; no runtime proof."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
class ImStatusContracts(unittest.TestCase):
    def test_original_refresh_types_are_distinct_from_sms_message_and_user_marking(self):
        text=(P/'im_status.rs').read_text(encoding='utf-8')
        for exact in ['refreshStatuses.GroupServer.Group','Status.Update','b"ML"','b"MLB"','b"P:P_MR"','b"P:P_MV-N"','b"P:P_UDMR-N"','b"P:P_MPR-N"']:
            self.assertIn(exact,text)
        self.assertIn('w.bytes(uid)?',text);self.assertIn('string_set(&mut w,&labels)?',text);self.assertIn('string_set(&mut w,&REFRESH_TYPES)?',text);self.assertIn('w.i64(since)?',text)
    def test_status_parser_preserves_nested_map_footer_and_rejects_missing_cursor_inference(self):
        text=(P/'im_status.rs').read_text(encoding='utf-8')
        for exact in ['r.count(8)?','StatusLabel::parse(r.bytes()?)?','r.properties()?','update_time=r.i64()?','read_footer(&mut r)?','latest:Option<i64>','peer_read:Option<i64>','last_received:Option<i64>','last_read:Option<i64>','let target=self.latest?;let current=self.peer_read?','CodecError::DuplicateKey']:
            self.assertIn(exact,text)
        self.assertIn('n.to_string()!=text',text);self.assertNotIn('unwrap_or(0)',text);self.assertNotIn('unwrap_or(-1)',text)
    def test_status_data_has_no_serialized_auth_or_current_io_consumer(self):
        text=(P/'im_status.rs').read_text(encoding='utf-8')
        for forbidden in ['Deserialize','Serialize','AttemptOwner','RpcConnectionScope','connect(','tokio::spawn','std::fs','reqwest','unsafe','SdkLoginRequest']:
            self.assertNotIn(forbidden,text)
        self.assertIn('Group.GroupId',text);self.assertIn('Group.UpdateTime',text)
    def test_status_tests_are_mounted_under_retained_actions_filter_without_skip(self):
        self.assertIn('im_status_tests.rs',(P/'im_data_tests.rs').read_text(encoding='utf-8'))
        self.assertNotIn('#[ignore',(P/'im_status_tests.rs').read_text(encoding='utf-8'))
        for workflow in ['build-release.yml','beta-validation.yml']:
            self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_incoming::tests',(R/'.github/workflows'/workflow).read_text(encoding='utf-8'))
if __name__=='__main__':unittest.main()
