"""Static fences for durable original IM associations; not runtime acceptance."""
from pathlib import Path
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return (P/name).read_text(encoding='utf-8')
class ImAssociationContracts(unittest.TestCase):
    def test_private_association_has_exact_original_id_run_scope_and_payload_hash(self):
        s=src('im_associations.rs')
        for value in ['PRIMARY KEY(line_id,run_nonce,scope_epoch,notification_id)','length(notification_id)=8','key_fingerprint','notice_fingerprint','message_fingerprint','outcome IN (0,1,2)','TransactionBehavior::Immediate','MAX_ASSOC_PER_ACCOUNT:i64=1024','MAX_ASSOC_TOTAL:i64=4096']:
            self.assertIn(value,s)
        for bad in ['Serialize','Deserialize','send_call','tokio::spawn','std::fs','setRead']:
            self.assertNotIn(bad,s)
    def test_shared_im_binding_creates_only_one_epoch_for_both_moremessage_and_clientex(self):
        s=src('arc_session.rs');client=s.split('fn native_client_ex_dispatcher',1)[1].split('fn can_arm_native_receivers',1)[0]
        self.assertNotIn('self.native_im_binding(',client)
        protocol=src('arc_protocol.rs');self.assertIn('self.im_binding = self.rpc.native_im_binding(setup.policy.clone())?',protocol)
        self.assertIn('with_im(self.im_binding.clone())?',protocol)
        delivery=src('im_delivery.rs');self.assertIn('receipt_epoch:Zeroizing<String>',delivery)
        self.assertIn('receipt_epoch()==self.receipt_epoch.as_str()',src('im_inbox.rs'))
    def test_pending_sql_row_is_not_a_new_consumption_deadline_or_serializable_grant(self):
        s=src('im_associations.rs');self.assertIn('struct ImPendingAssociation{ticket:ImReplyTicket',s)
        self.assertIn('if row.outcome==PENDING',s);self.assertIn('SET outcome=1 WHERE',s)
        self.assertIn('proof.is_some_and(|p|p.authorizes(ticket,Instant::now()))',s)
        self.assertIn('AND outcome=0',s);self.assertIn('if !result{return Err',s)
        self.assertIn('SET outcome=1,message_id=NULL,message_fingerprint=NULL',s)
        self.assertNotIn('clipped_to(',s)
    def test_consumed_row_replay_rechecks_data_identity_and_never_trusts_row_id_alone(self):
        s=src('im_associations.rs')
        for value in ['row.identity.as_slice()!=self.binding.identity().as_slice()','row.key.as_slice()!=key.digest().as_slice()','row.notice.as_slice()!=fingerprint.as_slice()',
                      'AND identity_key=?4 AND box_id=?5 AND message_index=?6 AND fingerprint=?7','scope_epoch=?3','revision=?5']:
            self.assertIn(value,s)
        self.assertIn('AssociationAdmission::Replay((id>0).then(||self.permit(key,hash,id)))',s)
    def test_live_budget_only_releases_terminal_drained_groups_with_durable_association(self):
        s=src('account_im.rs')
        self.assertIn('scope.open_association(&run.database,&delivery)?',s)
        self.assertIn('group.context.finish_association(&run.database,pending,proof)?',s)
        self.assertIn('state.groups.retain(|_,group|group.outcome.is_none()||!group.holders.is_empty())',s)
        self.assertLess(s.index('complete(run,delivery,proof,deadline).await?'),s.index('group.holders.pop_front()'))
        self.assertIn('MAX_GROUPS:usize=64',s);self.assertIn('MAX_HOLDERS:usize=256',s);self.assertIn('MAX_BYTES:usize=1024*1024',s)
    def test_only_original_quiescence_or_os_locked_orphan_release_compacts_associations(self):
        s=src('account_runs.rs');block=s.split('fn release_in(',1)[1].split('pub(super) fn orphaned_leases',1)[0]
        self.assertIn('super::im_inbox::retire_run_associations(&tx,token)?',block)
        self.assertIn('proof:&RunQuiescence',s);self.assertIn('release_in(tx,&token)?',s)
        private=src('im_associations.rs').split('fn retire_run_associations',1)[1].split('fn delete_account',1)[0]
        self.assertIn('WHERE line_id=?1 AND revision=?2 AND run_nonce=?3',private)
        self.assertNotIn('DELETE FROM app_voice_im_messages',private)
    def test_individual_delete_keeps_tombstone_and_account_delete_requires_removed_account(self):
        s=src('im_inbox.rs');self.assertIn("SET deleted=1,body=X'',text='',sender=X'',attachment_count=0",s)
        self.assertNotIn('DELETE FROM app_voice_im_messages',s)
        private=src('im_associations.rs').split('fn delete_account',1)[1]
        self.assertIn('account_store::metadata(conn,line)?.is_some(){return Err(SCOPE);}',private)
        self.assertIn('DELETE FROM app_voice_im_messages WHERE line_id=?1',private)
        self.assertNotIn('DELETE FROM sms_',private)
        self.assertIn('super::im_inbox::delete_account(conn, line)?',src('account_inbox.rs'))
    def test_new_association_regressions_run_under_unchanged_existing_filters(self):
        for parent,child in [('im_protocol_tests.rs','im_association_tests.rs'),('im_association_tests.rs','im_association_lifecycle_tests.rs'),('im_account_tests.rs','im_retention_account_tests.rs')]:
            self.assertIn(child,src(parent));self.assertNotIn('#[ignore',src(child))
        for workflow in ['build-release.yml','beta-validation.yml']:
            s=(R/'.github/workflows'/workflow).read_text(encoding='utf-8')
            self.assertIn('connectivity::app_voice::providers::jegotrip::rpc_client_ex::tests',s)
            self.assertIn('connectivity::app_voice::providers::jegotrip::runtime_backend::common_tests',s)
if __name__=='__main__':unittest.main()
