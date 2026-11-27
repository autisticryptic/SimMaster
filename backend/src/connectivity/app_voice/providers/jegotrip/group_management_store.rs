//! Optional operation descriptors over the SAME relation intent/write journal.
//! Shared original rows enforce nonce uniqueness and Unknown-WRITE exclusion;
//! descriptors do not fabricate member counts or absent setProps timestamps.
use super::group_management::GroupWriteOperation;
use rusqlite::{Connection,params,OptionalExtension};
const STORAGE:&str="app_account_relation_storage_failed";
#[derive(Clone,Copy,Debug,PartialEq,Eq)]
pub(super) struct Descriptor{pub operation:GroupWriteOperation,pub properties:usize}
fn installed(c:&Connection)->Result<bool,&'static str>{c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='app_voice_group_write_operations')",[],|r|r.get(0)).map_err(|_|STORAGE)}
pub(super) fn initialize_schema(c:&Connection)->Result<(),&'static str>{c.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_group_write_operations(
 line_id TEXT NOT NULL,user_nonce TEXT NOT NULL CHECK(length(user_nonce)=32),operation TEXT NOT NULL CHECK(operation IN('set_properties','disband')),properties INTEGER NOT NULL CHECK(properties BETWEEN 0 AND 128),intent_id INTEGER,write_id INTEGER,PRIMARY KEY(line_id,user_nonce),CHECK(intent_id IS NOT NULL OR write_id IS NOT NULL));").map_err(|_|STORAGE)}
/// Admission-only integrity scan: optional descriptors may not orphan their
/// shared exclusion rows or turn corrupt management history into empty state.
pub(super) fn validate_shared(c:&Connection,line:&str)->Result<(),&'static str>{
 if super::group_create_store::installed(c)?{return super::group_create_store::validate_shared(c,line);}
 if !installed(c)?{let zero:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_relation_writes WHERE line_id=?1 AND additions+updates+removals=0) OR EXISTS(SELECT 1 FROM app_voice_relation_intents WHERE line_id=?1 AND additions+updates+removals=0)",[line],|r|r.get(0)).map_err(|_|STORAGE)?;return if zero{Err(STORAGE)}else{Ok(())};}
 let bad:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_group_write_operations d WHERE d.line_id=?1 AND (d.intent_id IS NULL OR NOT EXISTS(SELECT 1 FROM app_voice_relation_intents i WHERE i.id=d.intent_id AND i.line_id=d.line_id AND i.user_nonce=d.user_nonce AND i.target_kind='group' AND i.additions+i.updates+i.removals=0 AND ((i.state IN(0,2) AND i.write_id IS NULL AND d.write_id IS NULL) OR (i.state=1 AND i.write_id=d.write_id))) OR (d.write_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM app_voice_relation_writes w WHERE w.id=d.write_id AND w.line_id=d.line_id AND w.user_nonce=d.user_nonce AND w.target_kind='group' AND w.additions+w.updates+w.removals=0)))) OR EXISTS(SELECT 1 FROM app_voice_relation_writes w WHERE w.line_id=?1 AND w.additions+w.updates+w.removals=0 AND NOT EXISTS(SELECT 1 FROM app_voice_group_write_operations d WHERE d.line_id=w.line_id AND d.user_nonce=w.user_nonce AND d.write_id=w.id)) OR EXISTS(SELECT 1 FROM app_voice_relation_intents i WHERE i.line_id=?1 AND i.additions+i.updates+i.removals=0 AND NOT EXISTS(SELECT 1 FROM app_voice_group_write_operations d WHERE d.line_id=i.line_id AND d.user_nonce=i.user_nonce AND d.intent_id=i.id))",[line],|r|r.get(0)).map_err(|_|STORAGE)?;
 if bad{Err(STORAGE)}else{Ok(())}
}
pub(super) fn capture(c:&Connection,line:&str,nonce:&str,d:Option<Descriptor>,intent:Option<i64>,write:Option<i64>)->Result<(),&'static str>{
 let Some(d)=d else{return Ok(());};if d.operation==GroupWriteOperation::Create{return super::group_create_store::bind(c,line,nonce,d,intent,write);}if !installed(c)?{return Err("app_account_group_management_not_installed");}
 c.execute("INSERT INTO app_voice_group_write_operations(line_id,user_nonce,operation,properties,intent_id,write_id) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(line_id,user_nonce) DO NOTHING",params![line,nonce,d.operation.name(),d.properties as i64,intent,write]).map_err(|_|STORAGE)?;
 let row:Option<(String,i64,Option<i64>,Option<i64>)>=c.query_row("SELECT operation,properties,intent_id,write_id FROM app_voice_group_write_operations WHERE line_id=?1 AND user_nonce=?2",params![line,nonce],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_|STORAGE)?;
 let Some((op,count,old_intent,old_write))=row else{return Err(STORAGE);};if op!=d.operation.name()||count!=d.properties as i64||intent.is_some()&&old_intent!=intent||old_write.is_some()&&write.is_some()&&old_write!=write{return Err(STORAGE);}
 if let Some(write)=write{c.execute("UPDATE app_voice_group_write_operations SET write_id=?3 WHERE line_id=?1 AND user_nonce=?2 AND (write_id IS NULL OR write_id=?3)",params![line,nonce,write]).map_err(|_|STORAGE)?;}Ok(())
}
pub(super) fn get(c:&Connection,line:&str,nonce:&str,intent:Option<i64>,write:Option<i64>)->Result<Option<Descriptor>,&'static str>{
 if !installed(c)?{return super::group_create_store::get(c,line,nonce,intent,write);}let row:Option<(Option<String>,i64,Option<i64>,Option<i64>)>=c.query_row("SELECT CASE WHEN operation IN('set_properties','disband') THEN operation ELSE NULL END,properties,intent_id,write_id FROM app_voice_group_write_operations WHERE line_id=?1 AND user_nonce=?2",params![line,nonce],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(|_|STORAGE)?;
 if row.is_none(){return super::group_create_store::get(c,line,nonce,intent,write);}
 row.map(|(op,n,i,w)|{let operation=GroupWriteOperation::parse(op.as_deref().ok_or(STORAGE)?)?;if !(0..=128).contains(&n)||intent.is_some()&&intent!=i||write.is_some()&&write!=w||match operation{GroupWriteOperation::SetProperties=>n==0,GroupWriteOperation::Disband=>n!=0,GroupWriteOperation::Create=>true}{return Err(STORAGE);}Ok(Descriptor{operation,properties:n as usize})}).transpose()
}
pub(super) fn require(c:&Connection,line:&str,nonce:&str,d:Option<Descriptor>,intent:Option<i64>,write:Option<i64>)->Result<(),&'static str>{
 // Legacy membership messages never change method from a descriptor. Preserve
 // their existing hot-path SQL cost; only management needs this extra binding.
 if d.is_none(){return Ok(());}if get(c,line,nonce,intent,write)?==d{Ok(())}else{Err(STORAGE)}
}
pub(super) fn delete_account(c:&Connection,line:&str)->Result<(),&'static str>{if super::account_store::metadata(c,line)?.is_some(){return Err(STORAGE);}if installed(c)?{c.execute("DELETE FROM app_voice_group_write_operations WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}super::group_create_store::delete_account(c,line)}
