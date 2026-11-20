"""Supplemental current Hosted native IM sync/cursor source fences (Actions execute)."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return(P/name).read_text(encoding='utf-8')
class ImConsumerContracts(unittest.TestCase):
 def test_one_owned_read_chain_is_hosted_opt_in_and_never_a_detached_task(self):
  s=src('account_im_sync.rs');self.assertIn('if !run.backend.im_sync_enabled(){return Ok(())',s);self.assertIn('controller.phase()!=Phase::Hosted',s);self.assertIn('type ReadFuture=Pin<Box<dyn Future',s);self.assertIn('backend.read_im(&owner,&scope,submitted,gate).await',s);self.assertNotIn('tokio::spawn',s);self.assertNotIn('.abort(',s);self.assertIn('self.reading.take()',s)
 def test_event_loop_keeps_callbacks_stop_and_read_data_in_the_same_original_consumer(self):
  s=src('account_runtime.rs');self.assertIn('let mut im_sync = im_sync::State::default()',s);self.assertIn('result = im_sync.next(), if im_read_pending',s);self.assertIn('im_sync.received(&run,phase,&mut im,result)',s);self.assertIn('im_sync.clear()',s);self.assertIn('im_sync.drive(&run,&controller,&mut im)',s)
 def test_original_holder_first_deadline_and_old_inflight_budget_cover_ordering_queue(self):
  s=src('account_im.rs');self.assertIn('state.ordered.push_back(delivery);state.holders+=1;state.bytes=bytes',s);self.assertIn('state.holders>=MAX_HOLDERS||bytes>MAX_BYTES',s);self.assertIn('Instant::now()>=delivery.ticket().deadline()',s);self.assertIn('blocked_box==Some(delivery.notice().key().box_id().bytes())',s);self.assertNotIn('checked_add(Duration',s)
 def test_current_original_reply_query_binding_gate_and_live_sql_run_are_all_required(self):
  s=src('im_cursor.rs');self.assertIn('Arc::ptr_eq(&plan.query,reply.request().query_arc())',s);self.assertIn('Arc::ptr_eq(reply.request().binding(),&self.context.binding)',s);self.assertIn('Arc::ptr_eq(&reply.request().gate(),&self.context.gate)',s);self.assertIn('self.context.validate(&tx)?;self.reply_current(reply)?',s);self.assertIn('Gap{start:*start,count:*count}',s)
 def test_atomic_gap_uses_actual_last_record_or_original_positive_empty_end_not_notification_index(self):
  s=src('im_cursor.rs');self.assertIn('records.last().map_or(requested_end,|r|r.key.index())',s);self.assertIn('if !result.accepted{return Err("app_account_im_receive_rejected")',s);self.assertIn('for r in &records{let(id,event)=self.insert_record(&tx,r)?',s);self.assertIn('self.advance(&tx,box_id,plan.baseline,plan.consumed,gap,end,&span_hash)?',s);self.assertIn('for event in events{db.publish_committed_app_event(event)',s);self.assertNotIn('Notify.MsgIdx',s)
 def test_unknown_records_and_noncontiguous_cursor_cas_never_silently_advance(self):
  s=src('im_cursor.rs');self.assertIn('ImContent::capture(binding,message)?.ok_or(UNSUPPORTED)?',s);self.assertIn('ImText::decode(message)',src('im_content.rs'));self.assertIn('if !binding.receives_info(){return Ok(None);}',src('im_content.rs'));self.assertIn('consumed.checked_add(1)',s);self.assertIn('AND baseline=?9 AND consumed=?10 AND target>=?5',s);self.assertIn('if n!=1{return Err(CURSOR)',s);self.assertIn('target.max(old_target)',s);self.assertNotIn('consumed=baseline',s)
 def test_empty_baseline_and_deleted_receipt_do_not_materialize_body_and_no_write_is_granted(self):
  s=src('im_cursor.rs');self.assertIn('OnlinePosition::MissingOlder',s);self.assertIn('fingerprint.as_slice()!=record.fingerprint.as_slice()',s);self.assertIn('last_span_hash',s);self.assertNotIn('prepare_im_write',s);self.assertNotIn('SetReceived=>',s);self.assertIn('super::cursor::delete_account(conn,line)?',src('im_associations.rs'))
 def test_real_current_consumer_and_atomic_sql_regressions_are_mounted_in_retained_filters(self):
  self.assertIn('im_cursor_tests.rs',src('im_protocol_tests.rs'));self.assertIn('im_sync_account_tests.rs',src('im_account_tests.rs'))
  for file in ['im_cursor_tests.rs','im_sync_account_tests.rs']:
   self.assertNotIn('#[ignore',src(file));self.assertNotIn('dlopen',src(file));self.assertNotIn('Command::new("adb")',src(file))
if __name__=='__main__':unittest.main()
