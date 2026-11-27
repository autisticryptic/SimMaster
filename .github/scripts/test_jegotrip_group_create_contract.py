"""Group creation SOURCE and in-memory SQL DATA checks; not a native peer or device."""
from pathlib import Path
import re
import sqlite3
import unittest

ROOT = Path(__file__).resolve().parents[2]
P = ROOT / 'backend/src/connectivity/app_voice/providers/jegotrip'
API = ROOT / 'backend/src/api'
UI = ROOT / 'frontend'

def text(file):
    return file.read_text(encoding='utf-8')

def query(function, prefix):
    block = text(P/'group_create_store.rs').split('fn '+function+'(', 1)[1].split('\npub(super) fn ', 1)[0]
    return next(s for s in re.findall(r'"([^"\\]+)"', block) if s.startswith(prefix))

class CreationContract(unittest.TestCase):
    def test_config_is_independent_and_schema_is_installed_only_on_explicit_account_edit(self):
        self.assertEqual(text(P/'account_store.rs').count('if config.profile.group_creation.is_some(){super::group_create_store::initialize_schema(&tx)?;}'), 2)
        self.assertNotIn('group_create_store', text(P/'im_inbox.rs'))
        self.assertIn('GroupWriteOperation::Create=>false', text(P/'group_management.rs'))
        self.assertIn('request.operation==GroupWriteOperation::Create{return failure(', text(API/'app_voice_group_management.rs'))
    def test_inputs_measure_before_allocating_and_do_not_look_up_or_synthesize_owner(self):
        s = text(P/'group_create.rs'); self.assertLess(s.index('let(a,b)=size('), s.index('scope.reserve_native_rpc(')); self.assertLess(s.index('scope.reserve_native_rpc('), s.index('members.iter().collect::<Vec<_>>()'))
        for word in ['uid!=self.owner.as_slice()', 'm.label==owner).count()!=1', 'Group.Org.Name', 'Group.Org.Type', 'compare_keys(a.label,b.label)', 'read_footer(&mut r)?;r.finish()?']: self.assertIn(word, s)
        self.assertNotIn('RecipientUri::parse', s); self.assertNotIn('group_id:String', text(API/'app_voice_group_create.rs'))
    def test_same_shared_intent_exists_before_await_and_http_secrets_do_not_survive_it(self):
        s = text(P/'account_group_create.rs').split('fn prepare_group_create(', 1)[1].split('async fn finish_group_create', 1)[0]; self.assertIn('RelationIntent::capture', s); self.assertNotIn('.await', s)
        s = text(API/'app_voice_group_create.rs'); self.assertIn('relation_type:u32,first:String,second:String', s); self.assertLess(s.index('reserve_group_create_http_scratch'), s.index('.collect::<Vec<_>>()')); self.assertLess(s.index('drop(request);drop(scratch)'), s.index('finish_group_create(p).await'))
    def test_committed_brand_is_private_and_not_reconstructed_from_a_row(self):
        s = text(P/'relation_journal.rs'); resolve = s.split('fn resolve(', 1)[1].split('fn cancel(', 1)[0]; self.assertLess(resolve.index('commit_result'), resolve.index('tx.commit()')); self.assertLess(resolve.index('tx.commit()'), resolve.index('let token=CreateCommit'))
        self.assertIn('#[serde(skip)]pub(super) creation_receipt', s); self.assertIn('creation_receipt:None', s); self.assertIn('self.intent.fixed(c,1,Some(self.write))?', s)
        self.assertIn('self.configuration.require_current', s.split('impl CreateCommit', 1)[1]); self.assertIn('self.created.base,self.created.update', s)
    def test_query_cloning_does_not_duplicate_request_and_actual_binding_is_checked(self):
        s = text(P/'group_create_read.rs'); self.assertIn('claimed.compare_exchange(false,true', s); self.assertIn('self.request.compare_exchange(0,id', s); self.assertIn('Arc::ptr_eq(self.token.binding(),binding)', s); self.assertIn('reply.request().call_id()', s); self.assertIn('Arc::ptr_eq(receipt,self)', s)
        self.assertIn('receipt.arm_read(&binding,&gate,call_id,deadline)?', text(P/'rpc_im.rs')); self.assertNotIn('UPDATE app_voice', s)
    def test_read_retirement_reaches_physical_packets_and_old_channel_guards(self):
        s = text(P/'arc_protocol.rs'); self.assertIn('capture_created_read', s); self.assertIn('packet.retain_group_event_packet(guard.clone())', s); self.assertIn('self.rpc.retire_im_read(id);return Err(e.into())', s)
        self.assertIn('self.created_read.as_ref().is_none_or(|r|r.active())', text(P/'group_event_packets.rs')); self.assertIn('receipt.retired.store', text(P/'group_create_read.rs'))
    def test_shared_marker_only_displays_read_of_exact_paired_write(self):
        flow = text(UI/'src/policies/groupCreateFlow.ts'); self.assertLess(flow.index('storeRelationMarker('), flow.index('await ports.post(body)')); self.assertIn('relationMarkerResolved(marker, after)', flow); self.assertIn('intent.writeId === write.id', flow); self.assertIn('create.write.nonceTag === write.nonceTag', flow)
        pane = text(UI/'src/pages/sim/AppGroupCreatePane.tsx'); self.assertIn('simadmin-relation-write-v1:${lineId}', pane); self.assertIn('ifAvailable: true', pane); self.assertIn('setForm(empty)', pane); self.assertNotIn('setInterval(', pane)
    def test_new_tests_are_wired_into_existing_runners(self):
        for parent,leaf in [('relation_protocol_tests.rs','group_create_tests.rs'),('relation_account_tests.rs','group_create_account_tests.rs'),('relation_io_tests.rs','group_create_io_tests.rs')]: self.assertIn('#[path="'+leaf+'"]\nmod creation;', text(P/parent))
        self.assertIn('include!("app_voice_group_create_http_tests.rs")', text(API/'app_voice_http_tests.rs')); self.assertIn(' tests/groupCreate.test.ts', text(UI/'package.json')); self.assertIn(".concat(['jegotrip-group-create.spec.ts'])", text(UI/'playwright.jegotrip.config.ts'))
    def test_actual_sql_predicates_distinguish_preparing_bound_confirmed_and_corrupt_data(self):
        conn = sqlite3.connect(':memory:'); self.addCleanup(conn.close)
        for file in ['relation_journal.rs','relation_intent.rs','group_management_store.rs','group_create_store.rs']:
            ddl = re.search(r'execute_batch\("(CREATE TABLE IF NOT EXISTS app_voice_[^"\\]+)"\)', text(P/file)).group(1); conn.executescript(ddl)
        line,nonce='local','e'*32; pair=(line,nonce); body=b'B'*32; reply=b'R'*32; group=b'811111111-922222222'
        conn.execute("INSERT INTO app_voice_relation_intents(line_id,revision,user_nonce,body_hash,run_nonce,claim_nonce,configuration_hash,target_kind,additions,updates,removals,state,updated_at) VALUES(?,1,?,?,'r',?,zeroblob(32),'group',0,0,0,0,'now')", (*pair,body,'a'*32)); conn.execute(query('capture_input','INSERT INTO'), (*pair,1,2,1,1,body))
        get=lambda: conn.execute(query('get','SELECT d.properties'), pair).fetchone(); validate=query('validate_shared','SELECT EXISTS(SELECT 1 FROM app_voice_group_creations'); self.assertEqual(get(),(2,1,None,1,1,1)); self.assertEqual(conn.execute(validate,(line,)).fetchone(),(0,)); self.assertEqual(conn.execute(query('require_input','SELECT EXISTS'),(*pair,1,body,2,1,1)).fetchone(),(1,))
        conn.execute("INSERT INTO app_voice_relation_writes(line_id,revision,user_nonce,intent_hash,identity_key,run_nonce,scope_epoch,target_kind,additions,updates,removals,state,request_id,updated_at) VALUES(?,1,?,zeroblob(32),zeroblob(32),'r','s','group',0,0,0,0,2,'now')",pair); conn.execute(query('bind','UPDATE app_voice_group_creations'),(*pair,1)); conn.execute('UPDATE app_voice_relation_intents SET write_id=1,state=1'); self.assertEqual(get(),(2,1,1,1,1,1)); self.assertEqual(conn.execute(validate,(line,)).fetchone(),(0,))
        evidence=(*pair,1,body,group,reply,7,0,1,2); matches=query('receipt_matches','SELECT EXISTS'); self.assertEqual(conn.execute(matches,evidence).fetchone(),(0,)); conn.execute('UPDATE app_voice_relation_writes SET state=1,result_code=0,base_time=-3,update_time=5'); self.assertIsNone(get()[-1]); conn.execute(query('commit_result','UPDATE app_voice_group_creations'),(*pair,1,group,reply,7,0,1)); self.assertEqual(get()[-1],1); self.assertEqual(conn.execute(matches,evidence).fetchone(),(1,)); conn.execute('UPDATE app_voice_group_creations SET body_hash=zeroblob(32)'); self.assertIsNone(get()[-1]); self.assertEqual(conn.execute(validate,(line,)).fetchone(),(1,)); self.assertEqual(conn.execute(matches,evidence).fetchone(),(0,))

if __name__=='__main__': unittest.main()
