"""SOURCE + synthetic SQLite data only; live ownership/IO must pass the original Actions."""
from pathlib import Path
import re
import sqlite3
import unittest
R=Path(__file__).resolve().parents[2];P=R/'backend/src/connectivity/app_voice/providers/jegotrip'
def src(name):return (P/name).read_text(encoding='utf-8')

class OwnerOnlyContract(unittest.TestCase):
    def test_opt_in_requires_existing_create_native_bootstrap_and_derived_uri(self):
        body=src('runtime_backend.rs').split('fn with_group_owner_only(',1)[1].split('fn with_group_management',1)[0]
        for word in ['let Some(config)=config else{return Ok(self);}','!config.from_session','self.group_creation.is_none()','self.common_bootstrap.is_none()','self.gateway_uri.is_some()']:self.assertIn(word,body)
        self.assertIn('group_owner_only:false',src('runtime_backend.rs'));self.assertIn('pub group_owner_only:Option<',src('account_config.rs'));self.assertEqual(src('account_store.rs').count('if config.profile.group_owner_only.is_some(){super::group_create_store::enable_owner_only(&tx)?;}'),2)
    def test_source_is_derived_from_the_current_attempt_not_http_or_an_alive_token(self):
        s=src('group_owner_only.rs');self.assertIn('reference.matches(owner,scope,router)',s);self.assertIn('book.permits_client_read(owner,scope)',s);self.assertLess(s.index('scope.reserve_native_rpc'),s.index('first.to_vec()'))
        self.assertIn('RecipientUri::parse(uri)?.value()',s);self.assertIn('valid_uid(uid)',s);self.assertNotIn('AliveLink',s);self.assertNotIn('user_entry_rpc::',s)
        s=src('runtime_relation_updates.rs').split('fn reserve_owner_only_message',1)[1].split('fn reserve_creation_scratch',1)[0];self.assertIn('attempt.session_uri',s);self.assertIn('attempt.user_entry_uid_local',s);self.assertNotIn('.await',s)
    def test_positive_actor_id_is_not_allocated_before_original_uri_check(self):
        s=src('rpc_relation_update_session.rs');self.assertLess(s.index('permit.matches_session_uri(self.login_uri'),s.index('let id=self.next_id'));self.assertLess(s.index('permit.matches_session_uri(self.login_uri'),s.index('encode_original('));self.assertIn('source.arguments(uid,&self.properties)',src('group_create.rs'))
    def test_shared_intent_and_physical_guard_retain_live_owner_scope(self):
        self.assertIn('message.owner_source_current(&owner,&scope)',src('relation_intent.rs'));self.assertIn('self.message.owner_source_current(self.owner(),&self.scope())',src('relation_journal.rs'));self.assertIn('self.input.owner_only.as_ref().is_none_or',src('relation_journal.rs'));self.assertIn('original-session-owner-only-group-create-v1',src('group_create.rs'))
        self.assertIn('r.permit().packet_current(r.id())',src('group_event_packets.rs'));self.assertNotIn('CREATE TABLE',src('group_owner_only.rs'))
    def test_dto_rejects_private_inputs_and_wipes_before_await(self):
        s=(R/'backend/src/api/app_voice_group_owner_only.rs').read_text(encoding='utf-8');fields=s.split('struct OwnerOnlyRequest {',1)[1].split('}',1)[0];self.assertIn('session_owner_consent:bool',fields);self.assertIn('deny_unknown_fields',s)
        for k in ['uid:','members:','uri:','token:','owner_first:','group_id:']:self.assertNotIn(k,fields)
        self.assertLess(s.index('reserve_owner_only_http_scratch'),s.index('.collect::<Vec<_>>()'));self.assertLess(s.index('drop(request);drop(scratch)'),s.index('finish_group_create(p).await'))
    def test_schema_upgrade_preserves_old_rows_and_check_constraints_bind_both_mode_witnesses(self):
        c=sqlite3.connect(':memory:');self.addCleanup(c.close);s=src('group_create_store.rs');ddl=re.search(r'execute_batch\("(CREATE TABLE IF NOT EXISTS app_voice_group_creations[^"\\]+)"',s)[1];upgrade=re.search(r'execute_batch\("(ALTER TABLE app_voice_group_creations[^"\\]+)"',s)[1];initial=re.search(r'"(INSERT INTO app_voice_group_creations[^"\\]+)"',s)[1];mode=re.search(r'"(SELECT CASE WHEN owner_only=0[^"\\]+)"',s)[1];enable=next(x for x in re.findall(r'"(INSERT INTO app_voice_group_creations[^"\\]+)"',s) if 'owner_only_hash' in x);trigger=re.search(r'execute_batch\("(CREATE TRIGGER IF NOT EXISTS app_voice_owner_mode_immutable[^"\\]+)"',s)[1]
        c.executescript(ddl);a=('fixture','a'*32);b=('fixture','b'*32);c.execute(initial,(*a,1,2,2,1,b'A'*32));c.executescript(upgrade);c.executescript(trigger);self.assertEqual(c.execute(mode,a).fetchone(),(0,));self.assertEqual(c.execute(enable,(*b,2,3,1,1,b'B'*32)).rowcount,1);self.assertEqual(c.execute(mode,b).fetchone(),(1,))
        with self.assertRaises(sqlite3.IntegrityError):c.execute(enable,('fixture','c'*32,3,2,2,1,b'C'*32))
        for set_clause in ['owner_only=0','owner_only=0,owner_only_hash=NULL','owner_only_hash=NULL','initial_members=3','owner_only_hash=zeroblob(32)']:
            with self.assertRaises(sqlite3.IntegrityError):c.execute('UPDATE app_voice_group_creations SET '+set_clause+' WHERE user_nonce=?',[b[1]])
        self.assertEqual(c.execute(mode,a).fetchone(),(0,));self.assertEqual(c.execute(mode,b).fetchone(),(1,))
    def test_legacy_mode_and_new_mode_use_distinct_grants_and_one_shot_result(self):
        s=src('relation_update_policy.rs');self.assertIn('Self::OwnerOnly(p)',s);self.assertIn('GroupWriteOperation::CreateOwnerOnly&&p.create',s);self.assertIn('operation(c,line,nonce)?==input.operation()',src('group_create_store.rs'));self.assertIn('is_some_and(|op|op.is_create())',src('rpc_relation_update.rs'))
        self.assertIn('create_owner_only',src('group_management.rs'));self.assertIn('receipt.arm_read(&binding,&gate,call_id,deadline)?',src('rpc_im.rs'))
    def test_ui_correlation_is_mode_specific_with_same_origin_marker(self):
        s=(R/'frontend/src/policies/groupCreateFlow.ts').read_text(encoding='utf-8');self.assertIn("captureOwnerOnlyBody(input), 'create_owner_only'",s);self.assertIn('management: { operation, properties:',s);self.assertIn('relationMarkerResolved(marker, after)',s);self.assertLess(s.index('storeRelationMarker('),s.index('await ports.post(body)'))
    def test_new_actions_cases_are_mounted_without_another_runner(self):
        for parent,leaf in [('group_create_tests.rs','group_owner_only_tests.rs'),('group_create_account_tests.rs','group_owner_only_account_tests.rs'),('group_create_io_tests.rs','group_owner_only_io_tests.rs')]:self.assertIn('#[path="'+leaf+'"]\nmod owner_only;',src(parent))
        self.assertIn('app_voice_group_owner_only_http_tests.rs',(R/'backend/src/api/app_voice_http_tests.rs').read_text(encoding='utf-8'));self.assertIn('groupOwnerOnly.test.ts',(R/'frontend/package.json').read_text(encoding='utf-8'));self.assertIn('jegotrip-group-owner-only.spec.ts',(R/'frontend/playwright.jegotrip.config.ts').read_text(encoding='utf-8'))
if __name__=='__main__':unittest.main()
