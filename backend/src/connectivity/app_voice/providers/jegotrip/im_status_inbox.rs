//! Private original flat status consumption. Each property has its own time:
//! a partial update must not erase a different property or regress its value.
//! This proof acknowledges only a Status.Update holder, never IM body/readiness.
use super::*;
use super::super::im_status_delivery::{IncomingImStatus,ImStatusTicket};
const MAX_STATUS_PER_ACCOUNT:i64=128;
const MAX_STATUS_TOTAL:i64=4096;
const MAX_PROPERTIES:i64=128;
const STATUS_FULL:&str="app_account_im_status_full";
pub(in super::super) fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_im_status_observations(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        label BLOB NOT NULL CHECK(typeof(label)='blob' AND length(label) BETWEEN 1 AND 508),
        update_time INTEGER NOT NULL CHECK(update_time>=-1),
        body BLOB NOT NULL CHECK(typeof(body)='blob' AND length(body)<=65536),
        fingerprint BLOB NOT NULL CHECK(typeof(fingerprint)='blob' AND length(fingerprint)=32),
        observed_run TEXT NOT NULL,observed_epoch TEXT NOT NULL,
        PRIMARY KEY(line_id,revision,identity_key,label));
        CREATE TABLE IF NOT EXISTS app_voice_im_status_properties(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),
        identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        label BLOB NOT NULL CHECK(typeof(label)='blob' AND length(label) BETWEEN 1 AND 508),
        status_key BLOB NOT NULL CHECK(typeof(status_key)='blob' AND length(status_key)<=65536),
        status_value BLOB NOT NULL CHECK(typeof(status_value)='blob' AND length(status_value)<=65536),
        update_time INTEGER NOT NULL CHECK(update_time>=-1),
        PRIMARY KEY(line_id,revision,identity_key,label,status_key));") .map_err(|_|STORAGE)
}
pub(in super::super) fn delete_account(conn:&Connection,line:&str)->Result<(),&'static str>{
    if account_store::metadata(conn,line)?.is_some(){return Err(SCOPE);}
    for table in ["app_voice_im_status_properties","app_voice_im_status_observations"]{
        let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[table],|r|r.get(0)).map_err(|_|STORAGE)?;
        if exists{conn.execute(&format!("DELETE FROM {table} WHERE line_id=?1"),[line]).map_err(|_|STORAGE)?;}
    }Ok(())
}
/// Constructed only after current scoped SQL processing below. No public/raw
/// constructor, deserialization or stored bool can mint this reply capability.
pub struct CommittedImStatus{binding:Arc<ImBinding>,gate:Arc<ImReceiptGate>,fingerprint:[u8;32]}
impl fmt::Debug for CommittedImStatus{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("CommittedImStatus(<redacted>)")}}
impl CommittedImStatus{
    pub(in super::super) fn authorizes(&self,ticket:&ImStatusTicket,now:Instant)->bool{
        self.gate.live()&&now<ticket.deadline()&&ticket.matches(self.binding.owner(),&self.binding.scope())
            &&Arc::ptr_eq(ticket.binding(),&self.binding)&&ticket.fingerprint()==&self.fingerprint
    }
    pub(in super::super) fn gate(&self)->Arc<ImReceiptGate>{self.gate.clone()}
    pub(in super::super) fn captured_copy(&self)->Self{Self{binding:self.binding.clone(),gate:self.gate.clone(),fingerprint:self.fingerprint}}
}
fn retained(conn:&Connection)->Result<i64,&'static str>{
    conn.query_row("SELECT (SELECT COALESCE(SUM(length(body)+length(label)),0) FROM app_voice_im_status_observations)+(SELECT COALESCE(SUM(length(status_key)+length(status_value)),0) FROM app_voice_im_status_properties)",[],|r|r.get(0)).map_err(|_|STORAGE)
}
impl ImInboxScope{
    pub(in super::super) fn commit_status(&self,db:&Database,delivery:&IncomingImStatus)->Result<CommittedImStatus,&'static str>{
        let notice=delivery.notice();let current=||self.gate.live()&&delivery.matches(self.binding.owner(),&self.binding.scope())&&Arc::ptr_eq(notice.binding(),&self.binding)&&Instant::now()<delivery.ticket().deadline();
        if !current(){return Err(SCOPE);}let decoded=notice.status().map_err(|_|INVALID)?;
        db.with_conn(|conn|{let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.validate(&tx)?;if !current(){return Err(SCOPE);}
            let saved:Option<i64>=tx.query_row("SELECT update_time FROM app_voice_im_status_observations WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND label=?4",
                params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),notice.label()],|r|r.get(0)).optional().map_err(|_|STORAGE)?;
            if saved.is_none(){
                let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_im_status_observations",[self.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;
                if local>=MAX_STATUS_PER_ACCOUNT||total>=MAX_STATUS_TOTAL{return Err(STATUS_FULL);}
                tx.execute("INSERT INTO app_voice_im_status_observations(line_id,revision,identity_key,label,update_time,body,fingerprint,observed_run,observed_epoch) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),notice.label(),notice.time(),notice.body(),notice.fingerprint().as_slice(),self.run.nonce(),self.binding.receipt_epoch()]).map_err(|_|STORAGE)?;
            }else if saved.is_some_and(|time|time<notice.time()){
                tx.execute("UPDATE app_voice_im_status_observations SET update_time=?5,body=?6,fingerprint=?7,observed_run=?8,observed_epoch=?9 WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND label=?4",
                    params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),notice.label(),notice.time(),notice.body(),notice.fingerprint().as_slice(),self.run.nonce(),self.binding.receipt_epoch()]).map_err(|_|STORAGE)?;
            }
            for p in &decoded.entry.properties{
                let saved:Option<(i64,Vec<u8>)>=tx.query_row("SELECT update_time,status_value FROM app_voice_im_status_properties WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND label=?4 AND status_key=?5",
                    params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),notice.label(),p.key],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|_|STORAGE)?;
                if let Some((time,value))=saved{
                    let value=Zeroizing::new(value);
                    if time==notice.time()&&value.as_slice()!=p.value{return Err("app_account_im_status_conflict");}
                    if time>=notice.time(){continue;}
                    tx.execute("UPDATE app_voice_im_status_properties SET status_value=?6,update_time=?7 WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND label=?4 AND status_key=?5",
                        params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),notice.label(),p.key,p.value,notice.time()]).map_err(|_|STORAGE)?;
                }else{
                    let count:i64=tx.query_row("SELECT COUNT(*) FROM app_voice_im_status_properties WHERE line_id=?1 AND revision=?2 AND identity_key=?3 AND label=?4",
                        params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),notice.label()],|r|r.get(0)).map_err(|_|STORAGE)?;
                    if count>=MAX_PROPERTIES{return Err(STATUS_FULL);}
                    tx.execute("INSERT INTO app_voice_im_status_properties(line_id,revision,identity_key,label,status_key,status_value,update_time) VALUES(?1,?2,?3,?4,?5,?6,?7)",
                        params![self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),notice.label(),p.key,p.value,notice.time()]).map_err(|_|STORAGE)?;
                }
            }
            if retained(&tx)?>MAX_TOTAL_BYTES{return Err(STATUS_FULL);}if !current(){return Err(SCOPE);}tx.commit().map_err(|_|STORAGE)
        })?;
        if !current(){return Err(SCOPE);}
        Ok(CommittedImStatus{binding:self.binding.clone(),gate:self.gate.clone(),fingerprint:*notice.fingerprint()})
    }
}
