"""Static source fences; actual Rust/peer behavior executes only on Actions."""
from pathlib import Path
import re,unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return (P/name).read_text(encoding='utf-8')
class ImDataContracts(unittest.TestCase):
    def test_exact_original_methods_are_distinct_from_sms_and_notification_completion(self):
        data=src('im_data.rs')
        for value in ['Message.MoreMessage','recv.Message.Message','setRead.Message.Message']:self.assertIn('"'+value+'"',data)
        for value in ['notifyEnd.ClientEx','Sip.Message.From','NativeEvent::IncomingMessage','pub struct RuntimeReceipt','ArcPhase::Authenticated']:
            self.assertNotIn(value,data)
    def test_original_box_identity_grammars_do_not_become_phone_numbers_or_routes(self):
        data=src('im_data.rs');self.assertIn('bytes.strip_prefix(b"P2P/")',data);self.assertIn('bytes.strip_prefix(b"Org/")',data)
        self.assertIn("b==b'_'",data);self.assertIn("b==b'-'",data);self.assertIn("b==b'+'",data)
        self.assertIn("last[0]==b'0'||kind==BoxKind::Org&&first[0]==b'0'",data)
        self.assertIn('MAX_BOX_BYTES:usize=512',data)
        self.assertNotIn('parse_object_reference',data);self.assertNotIn('phone_number',data)
    def test_message_key_is_signed64_without_fallback_zero_or_dynamic_permission(self):
        data=src('im_data.rs');self.assertIn('text.parse::<i64>()',data);self.assertIn('if index.to_string()!=text',data)
        self.assertIn('properties.len()!=3',data);self.assertIn('Notify.MsgIdx',data);self.assertIn('Notify.Box',data)
        self.assertNotIn('unwrap_or(0)',data);self.assertNotIn('notification_id:',data)
    def test_record_codec_preserves_two_full_integers_three_strings_and_both_maps(self):
        data=src('im_data.rs');read=data.split('fn read(reader:',1)[1].split('pub(super) fn decode',1)[0]
        self.assertEqual(read.count('reader.i64()?'),2);self.assertEqual(read.count('reader.bytes()?'),3)
        self.assertEqual(read.count('reader.properties()?'),2)
        self.assertIn('let mut reader=Reader::new(bytes)?',data);self.assertIn('reader.finish()?;Ok(message)',data)
        self.assertNotIn('from_utf8_lossy',data);self.assertNotIn('chrono::',data)
    def test_reader_bounds_before_allocating_and_footer_is_not_omitted_on_false(self):
        data=src('im_data.rs');read=data.split('pub(super) fn decode_recv_result',1)[1].split('pub(super) fn encode_gap_recv',1)[0]
        self.assertLess(read.index('reader.count(36)?'),read.index('Vec::with_capacity(count)'))
        self.assertIn('for _ in 0..count{messages.push(ImMessage::read',read)
        self.assertIn('let footer=read_footer(&mut reader)?;reader.finish()?',read)
        self.assertIn('accepted:accepted&&footer.reason.is_empty()',read)
    def test_gap_request_retains_original_ten_and_checked_signed_range(self):
        data=src('im_data.rs');self.assertIn('GAP_BATCH_COUNT:u32=10',data)
        self.assertIn('start.checked_add(i64::from(count)-1).is_none()',data)
        self.assertIn('writer.bytes(box_id.bytes())?;writer.i64(start)?;writer.u32(count)?',data)
        self.assertIn('writer.bytes(box_id.bytes())?;writer.i64(index)?',data)
    def test_pure_record_parser_has_no_network_filesystem_tokens_or_serializable_grants(self):
        data=src('im_data.rs')
        for value in ['Serialize','Deserialize','TcpStream','connect(','std::fs','write_all','token:','callback:','unsafe','impl HostingBackend','impl NativeReceiverBinding']:
            self.assertNotIn(value,data)
        for name in ['ImBox','ImMessageKey','ImMessage','ImMoreMessage','ImRecvResult','ImSetReadResult']:self.assertIn(name+'(<redacted>)',data)
    def test_current_clientex_message_still_nacks_until_real_im_consumer_is_installed(self):
        data=src('rpc_client_ex.rs');self.assertIn('notify_accepted = false;',data)
        self.assertIn('p.key == b"Notify.Command" && p.value == b"Invite"',data)
        self.assertNotIn('decode_more_message',data)
        dispatcher=src('rpc_incoming.rs');self.assertIn('if message.receiver_kind != self.receiver_kind { return Ok(None); }',dispatcher)
    def test_structured_text_uses_original_keys_and_preserves_nonauthorizing_metadata(self):
        text=src('im_text.rs')
        self.assertIn('matches!(message.kind,b"Text"|b"System.Text")',text)
        for key in ['text','imdnId','csnId','displayName','userData','OnlineMsg']:self.assertIn('"'+key+'"',text)
        self.assertIn('map.next_value::<String>()?',text);self.assertIn('ImText(<redacted>)',text)
        self.assertIn('Option<Zeroizing<String>>;6',text)
        self.assertNotIn('Serialize',text);self.assertNotIn('Callback::Channels',text);self.assertNotIn('std::fs',text)
    def test_text_checks_fixed_bytes_depth_duplicates_and_required_text_before_use(self):
        text=src('im_text.rs');self.assertIn('MAX_JSON_DEPTH:usize=32',text)
        self.assertLess(text.index('if !bounded_json(message.content)'),text.index('serde_json::from_slice(message.content)'))
        self.assertIn('keys.len()>=MAX_MAP_ENTRIES',text);self.assertIn('prior.as_str()==key.as_str()',text)
        self.assertIn("value.contains('\\0')",text);self.assertIn('fields[TextField::Text.index()].is_none()',text)
        self.assertIn('map.next_value::<de::IgnoredAny>()?',text)
        names=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',src('im_text_tests.rs'));self.assertEqual(len(names),10)
        self.assertIn('im_text_tests.rs',src('im_data_tests.rs'))
    def test_exact_new_cases_are_selected_without_modifying_workflow_or_budgets(self):
        tests=src('im_data_tests.rs');names=re.findall(r'#\[(?:tokio::)?test\]\s*(?:async\s+)?fn\s+(\w+)',tests)
        self.assertEqual(len(names),20);self.assertEqual(len(set(names)),20);self.assertNotIn('#[ignore',tests)
        self.assertIn('im_data_tests.rs',src('rpc_incoming_tests.rs'));self.assertIn('pub(crate) mod im_data;',src('mod.rs'))
        for workflow in ['build-release.yml','beta-validation.yml']:
            text=(R/'.github/workflows'/workflow).read_text(encoding='utf-8');self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_incoming::tests',text);self.assertIn('--test-threads=1',text)
if __name__=='__main__':unittest.main()
