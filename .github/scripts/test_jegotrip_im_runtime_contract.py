"""Supplemental IM runtime source fences. Rust/SDK peers/UI run only in Actions."""
from pathlib import Path
import re,unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return (P/name).read_text(encoding='utf-8')
class ImRuntimeContracts(unittest.TestCase):
    def test_im_opt_in_is_independent_of_sms_and_call_media_and_omitted_when_absent(self):
        policy=src('native_receiver.rs');self.assertIn('#[serde(default)]\n    pub im_messages: Option<super::im_delivery::NativeImConfig>',policy)
        self.assertIn('policy.im_messages().is_some() && !client_ex',policy)
        config=src('account_config.rs');self.assertIn('receiver.im_messages.is_some()) && !p.client_ex',config)
        self.assertIn('#[serde(skip_serializing_if = "Option::is_none")]\n    im_messages:',config)
        self.assertIn('im_messages: config.im_messages.as_ref()',config)
    def test_original_sdk_identity_and_scope_bind_im_before_any_notification_can_supply_a_key(self):
        arc=src('arc_session.rs');self.assertIn('self.native_receiver_scope()',arc);self.assertIn('&authentication.user_id,&authentication.selected_session_id',arc)
        codec=src('rpc_client_ex.rs');self.assertIn('p.key == b"Notify.Command" && p.value == b"Message"',codec)
        self.assertIn('property(b"token") == Some(self.token.as_slice())',codec);self.assertIn('if let Some(binding)=&self.im',codec)
        delivery=src('im_delivery.rs');self.assertIn('jegotrip-native-im-context-v1\\0',delivery)
        self.assertNotIn('phone_uuid',delivery);self.assertNotIn('parse_object_reference',delivery)
    def test_im_message_dispatch_keeps_original_native_domain_rules_and_both_ticket_brands(self):
        delivery=src('im_delivery.rs')
        for value in ['ReceiverTransport::Direct','ReceiverTransport::Batch','incoming.ticket().matches','ImReplySource::Direct','ImReplySource::Batch','IncomingImDelivery(<redacted>)']:
            self.assertIn(value,delivery)
        protocol=src('arc_protocol.rs');self.assertIn('DirectResponse::Im',protocol);self.assertIn('self.capture_im',protocol)
        offline=src('offline_backend.rs');self.assertIn('im_pending.insert',offline);self.assertIn('ProtocolEvent::Im(delivery)',offline)
        self.assertIn('self.im_pending.contains_key(&(ticket.peer_incarnation(),ticket.sequence()))',offline)
    def test_only_original_persistent_consumption_permit_can_authorize_a_positive_reply(self):
        store=src('im_inbox.rs');self.assertIn('pub struct CommittedIm{',store);self.assertIn('ticket.matches(&self.owner,&self.scope)',store)
        self.assertIn('ticket.binding().identity()==&self.identity',store);self.assertIn('ticket.key().digest()==self.key.digest()',store)
        self.assertIn('!ticket.is_message()||ticket.fingerprint()==&self.fingerprint',store)
        self.assertIn('now<ticket.completion_deadline()',store);self.assertIn('self.gate.live()',store)
        self.assertNotIn('derive(Deserialize)',store)
        actor=src('transport_actor.rs');self.assertIn('proof.authorizes(ticket,Instant::now())',actor)
        self.assertIn('Operation::CompleteIm(Box::new',actor)
    def test_sql_transaction_preserves_raw_im_text_receipt_event_and_original_run_revision(self):
        store=src('im_inbox.rs')
        for value in ['TransactionBehavior::Immediate','account_runs::runtime_matches','account_runs::require_live','row.revision!=self.run.revision()',
                      'UNIQUE(line_id,revision,identity_key,box_id,message_index)','ImText::decode','insert_app_event_for_conn(&tx,"im.received"','db.publish_committed_app_event']:
            self.assertIn(value,store)
        self.assertLess(store.index('tx.commit().map_err(|_|STORAGE)?;Ok((id,Some(event)))'),store.index('Ok(Some(self.permit'))
        for value in ['INSERT INTO sms','INSERT INTO call_history','std::fs','with_io_gate(','with_media_gate(']:self.assertNotIn(value,store)
    def test_delete_is_a_private_tombstone_not_row_reuse_or_message_resurrection(self):
        store=src('im_inbox.rs');self.assertIn("SET deleted=1,body=X'',text='',sender=X'',attachment_count=0",store)
        self.assertIn('saved.as_slice()!=fingerprint.as_slice()',store)
        self.assertIn('return Ok((id,None))',store)
        self.assertNotIn('DELETE FROM app_voice_im_messages',store);self.assertNotIn('REPLACE INTO app_voice_im_messages',store)
        view=store.split('pub struct ImMessageView{',1)[1].split('}',1)[0]
        for private in ['identity_key','fingerprint','body:','sender:','box_id:','token:']:self.assertNotIn(private,view)
        self.assertIn('pub message_index:String',view)
    def test_fixed_deadlines_and_byte_budgets_survive_early_queue_and_pending_notification_drain(self):
        consumer=src('account_im.rs')
        for value in ['MAX_GROUPS:usize=64','MAX_HOLDERS:usize=256','MAX_BYTES:usize=1024*1024','MAX_EARLY:usize=16','Phase::HostingPending',
                      'state.early.push_back','complete(run,delivery,proof,group.deadline).await?','state.holders.checked_sub(1)']:
            self.assertIn(value,consumer)
        self.assertLess(consumer.index('complete(run,delivery,proof,group.deadline).await?'),consumer.index('group.holders.pop_front()'))
        self.assertIn('ticket.completion_deadline=ticket.completion_deadline.min(deadline)',src('im_delivery.rs'))
    def test_parent_stop_retirement_revokes_queued_im_io_before_cleanup_and_uses_owned_consumer(self):
        account=src('account_runtime.rs');self.assertIn('im_gate:super::im_inbox::ImReceiptGate::fresh()',account)
        self.assertIn('run.im_gate.retire();',account);self.assertIn('self.im_gate.retire();',account)
        self.assertIn('let mut im = im::State::default()',account);self.assertIn('im::delivery(&run,controller.phase()',account)
        self.assertNotIn('tokio::spawn',src('account_im.rs'));self.assertNotIn('.abort(',src('account_im.rs'))
        actor=src('transport_actor.rs');self.assertIn('im_waiting.push(gate.retired())',actor)
        self.assertIn('let mut sending = Box::pin(self.send_packets',actor)
        offline=src('offline_backend.rs');self.assertIn('self.im_completed.values().any(|(ticket,proof)|!proof.authorizes',offline)
        self.assertIn('im_completed.remove(&(retirement.peer_incarnation(),*sequence))',offline)
    def test_im_api_is_authenticated_bounded_and_never_exporting_raw_or_attachment_paths(self):
        api=(R/'backend/src/api/app_voice.rs').read_text(encoding='utf-8')
        self.assertIn('/im-messages", get(get_im_messages)',api);self.assertIn('/im-messages/{message_id}", delete(delete_im_message)',api)
        self.assertIn('id.to_string()!=message_id',api);self.assertIn('manager.im_messages(&line_id,request.limit)',api)
        self.assertIn('no_store(',api)
        self.assertIn('app_voice_im_http_tests.rs',(R/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'))
    def test_im_ui_renders_only_known_bounded_data_and_never_replays_unknown_delete(self):
        text=(R/'frontend/src/policies/imHistory.ts').read_text(encoding='utf-8');pane=(R/'frontend/src/pages/sim/AppImMessagesPane.tsx').read_text(encoding='utf-8')
        for value in ['BigInt(value)','Number.isSafeInteger(id)','value.slice(0, 128)','rows.length === 16','Object.prototype.hasOwnProperty.call']:
            self.assertIn(value,text)
        self.assertIn('不是运营商短信',text);self.assertIn('不等于服务端已读',text)
        for value in ['original.generation','readSerial','deleting.current','window.confirm','不自动重放删除']:
            self.assertIn(value,pane)
        for bad in ['dangerouslySetInnerHTML','localStorage','submitJegotripLogin','setAppAccountRunning','sendMessage','answerCall']:
            self.assertNotIn(bad,pane)
    def test_new_suites_are_mounted_under_existing_actions_filters(self):
        self.assertIn('im_protocol_tests.rs',src('rpc_client_ex_tests.rs'));self.assertIn('im_account_tests.rs',src('native_receiver_account_tests.rs'))
        for file in ['im_protocol_tests.rs','im_account_tests.rs']:
            self.assertNotIn('#[ignore',src(file));self.assertNotIn('dlopen',src(file))
        for workflow in ['build-release.yml','beta-validation.yml']:
            text=(R/'.github/workflows'/workflow).read_text(encoding='utf-8')
            for name in ['rpc_client_ex::tests','runtime_backend::common_tests']:
                self.assertIn('connectivity::app_voice::providers::jegotrip::'+name,text)
            self.assertIn('http_router_tests::app_voice_accounts',text);self.assertIn('--test-threads=1',text)
if __name__=='__main__':unittest.main()
