//! Durable original notification association, distinct from IM message rows,
//! SMS cursors, transport ACKs and serializable permission. No IO or worker.
use super::*;
const FULL_ASSOC:&str="app_account_im_association_full";
pub(in super::super) const MAX_ASSOC_PER_ACCOUNT:i64=1024;
pub(in super::super) const MAX_ASSOC_TOTAL:i64=4096;
const PENDING:i64=0;
const NEGATIVE:i64=1;
const CONSUMED:i64=2;
pub(super) fn initialize_schema(conn:&Connection)->Result<(),&'static str>{
    conn.execute_batch("CREATE TABLE IF NOT EXISTS app_voice_im_notifications(
        line_id TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>0),run_nonce TEXT NOT NULL CHECK(length(run_nonce)=32),
        scope_epoch TEXT NOT NULL CHECK(length(scope_epoch)=32),
        notification_id BLOB NOT NULL CHECK(typeof(notification_id)='blob' AND length(notification_id)=8),
        identity_key BLOB NOT NULL CHECK(typeof(identity_key)='blob' AND length(identity_key)=32),
        key_fingerprint BLOB NOT NULL CHECK(typeof(key_fingerprint)='blob' AND length(key_fingerprint)=32),
        notice_fingerprint BLOB NOT NULL CHECK(typeof(notice_fingerprint)='blob' AND length(notice_fingerprint)=32),
        outcome INTEGER NOT NULL CHECK(outcome IN (0,1,2)),message_id INTEGER,message_fingerprint BLOB,
        CHECK((outcome=2 AND message_id>0 AND typeof(message_fingerprint)='blob' AND length(message_fingerprint)=32)
           OR (outcome<>2 AND message_id IS NULL AND message_fingerprint IS NULL)),
        PRIMARY KEY(line_id,run_nonce,scope_epoch,notification_id),
        FOREIGN KEY(line_id) REFERENCES app_voice_accounts(line_id) ON DELETE CASCADE);
        CREATE INDEX IF NOT EXISTS idx_app_voice_im_notifications_scope ON app_voice_im_notifications(line_id,run_nonce,scope_epoch);")
        .map_err(|_|STORAGE)
}
/// Only the first current original notification can construct pending work.
/// A SQL pending row or new RPC cannot manufacture a new first deadline.
pub(in super::super) struct ImPendingAssociation{ticket:ImReplyTicket,notification_id:[u8;8]}
impl fmt::Debug for ImPendingAssociation{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImPendingAssociation(<redacted>)")}}
pub(in super::super) enum AssociationAdmission{Fresh(ImPendingAssociation),Replay(Option<CommittedIm>)}
impl fmt::Debug for AssociationAdmission{fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str("ImAssociationAdmission(<redacted>)")}}
struct Row{identity:Vec<u8>,key:Vec<u8>,notice:Vec<u8>,outcome:i64,message_id:Option<i64>,message_fingerprint:Option<Vec<u8>>}
impl ImInboxScope{
    pub(in super::super) fn open_association(&self,db:&Database,delivery:&IncomingImDelivery)->Result<AssociationAdmission,&'static str>{
        if !self.matches(delivery){return Err(SCOPE);}
        let id=delivery.notice().notification_id().ok_or(SCOPE)?.to_be_bytes();let ticket=delivery.ticket();let key=ticket.key();let fingerprint=ticket.fingerprint();
        let existing:Option<(i64,[u8;32])>=db.with_conn(|conn|{
            let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.validate(&tx)?;
            let row:Option<Row>=tx.query_row("SELECT identity_key,key_fingerprint,notice_fingerprint,outcome,message_id,message_fingerprint FROM app_voice_im_notifications WHERE line_id=?1 AND run_nonce=?2 AND scope_epoch=?3 AND notification_id=?4 AND revision=?5",
                params![self.run.line_id(),self.run.nonce(),self.binding.receipt_epoch(),id.as_slice(),self.run.revision()as i64],|r|Ok(Row{identity:r.get(0)?,key:r.get(1)?,notice:r.get(2)?,outcome:r.get(3)?,message_id:r.get(4)?,message_fingerprint:r.get(5)?})).optional().map_err(|_|STORAGE)?;
            if let Some(row)=row{
                if row.identity.as_slice()!=self.binding.identity().as_slice()||row.key.as_slice()!=key.digest().as_slice()||row.notice.as_slice()!=fingerprint.as_slice(){return Err("app_account_im_notification_conflict");}
                if row.outcome!=CONSUMED{
                    // Pending survived an interrupted admission, not its
                    // original in-memory first-deadline capability. Keep NACK.
                    if row.outcome==PENDING{tx.execute("UPDATE app_voice_im_notifications SET outcome=1 WHERE line_id=?1 AND run_nonce=?2 AND scope_epoch=?3 AND notification_id=?4 AND outcome=0",params![self.run.line_id(),self.run.nonce(),self.binding.receipt_epoch(),id.as_slice()]).map_err(|_|STORAGE)?;}
                    tx.commit().map_err(|_|STORAGE)?;return Ok(Some((0,[0;32])));
                }
                let message_id=row.message_id.ok_or(STORAGE)?;let saved: [u8;32]=row.message_fingerprint.ok_or(STORAGE)?.try_into().map_err(|_|STORAGE)?;
                // RowID or a stored boolean cannot redeem changed/deleted-and-
                // reinserted history. A normal text deletion keeps its hash.
                let same:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_messages WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4 AND box_id=?5 AND message_index=?6 AND fingerprint=?7)",
                    params![message_id,self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),key.box_id().bytes(),key.index(),saved.as_slice()],|r|r.get(0)).map_err(|_|STORAGE)?;
                tx.commit().map_err(|_|STORAGE)?;return Ok(Some(if same{(message_id,saved)}else{(0,[0;32])}));
            }
            let(local,total):(i64,i64)=tx.query_row("SELECT COALESCE(SUM(line_id=?1),0),COUNT(*) FROM app_voice_im_notifications",[self.run.line_id()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(|_|STORAGE)?;
            if local>=MAX_ASSOC_PER_ACCOUNT||total>=MAX_ASSOC_TOTAL{return Err(FULL_ASSOC);}
            tx.execute("INSERT INTO app_voice_im_notifications(line_id,revision,run_nonce,scope_epoch,notification_id,identity_key,key_fingerprint,notice_fingerprint,outcome) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,0)",
                params![self.run.line_id(),self.run.revision()as i64,self.run.nonce(),self.binding.receipt_epoch(),id.as_slice(),self.binding.identity().as_slice(),key.digest().as_slice(),fingerprint.as_slice()]).map_err(|_|STORAGE)?;
            tx.commit().map_err(|_|STORAGE)?;Ok(None)
        })?;
        Ok(match existing{
            None=>AssociationAdmission::Fresh(ImPendingAssociation{ticket:ticket.captured_copy(),notification_id:id}),
            Some((id,hash))=>AssociationAdmission::Replay((id>0).then(||self.permit(key,hash,id))),
        })
    }
    pub(in super::super) fn finish_association(&self,db:&Database,pending:&ImPendingAssociation,proof:Option<&CommittedIm>)->Result<Option<CommittedIm>,&'static str>{
        let ticket=&pending.ticket;
        if !self.gate.live()||!ticket.matches(self.binding.owner(),&self.binding.scope())||ticket.binding().receipt_epoch()!=self.binding.receipt_epoch(){return Err(SCOPE);}
        let accepted=proof.is_some_and(|p|p.authorizes(ticket,Instant::now()));
        let result=db.with_conn(|conn|{
            let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.validate(&tx)?;
            let (state,id,hash)=if let Some(proof)=proof.filter(|_|accepted){
                let same:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM app_voice_im_messages WHERE id=?1 AND line_id=?2 AND revision=?3 AND identity_key=?4 AND box_id=?5 AND message_index=?6 AND fingerprint=?7)",
                    params![proof.id,self.run.line_id(),self.run.revision()as i64,self.binding.identity().as_slice(),ticket.key().box_id().bytes(),ticket.key().index(),proof.fingerprint.as_slice()],|r|r.get(0)).map_err(|_|STORAGE)?;
                if !same{return Err("app_account_im_notification_conflict");}(CONSUMED,Some(proof.id),Some(proof.fingerprint.as_slice()))
            }else{(NEGATIVE,None,None)};
            let changed=tx.execute("UPDATE app_voice_im_notifications SET outcome=?1,message_id=?2,message_fingerprint=?3 WHERE line_id=?4 AND revision=?5 AND run_nonce=?6 AND scope_epoch=?7 AND notification_id=?8 AND identity_key=?9 AND key_fingerprint=?10 AND notice_fingerprint=?11 AND outcome=0",
                params![state,id,hash,self.run.line_id(),self.run.revision()as i64,self.run.nonce(),self.binding.receipt_epoch(),pending.notification_id.as_slice(),self.binding.identity().as_slice(),ticket.key().digest().as_slice(),ticket.fingerprint().as_slice()]).map_err(|_|STORAGE)?;
            tx.commit().map_err(|_|STORAGE)?;Ok(changed==1)
        })?;
        if !result{return Err("app_account_im_notification_conflict");}
        if accepted&&!proof.is_some_and(|p|p.authorizes(ticket,Instant::now())){
            // No await/new event can observe the provisional consumed row
            // before this post-commit deadline/revoke check. Crash loses this
            // private scope epoch, so an old row cannot acquire a fresh lease.
            db.with_conn(|conn|{
                let tx=Transaction::new_unchecked(conn,TransactionBehavior::Immediate).map_err(|_|STORAGE)?;self.validate(&tx)?;
                tx.execute("UPDATE app_voice_im_notifications SET outcome=1,message_id=NULL,message_fingerprint=NULL WHERE line_id=?1 AND run_nonce=?2 AND scope_epoch=?3 AND notification_id=?4 AND outcome=2",
                    params![self.run.line_id(),self.run.nonce(),self.binding.receipt_epoch(),pending.notification_id.as_slice()]).map_err(|_|STORAGE)?;tx.commit().map_err(|_|STORAGE)
            })?;
            return Ok(None);
        }
        Ok(proof.filter(|_|accepted).map(|p|p.captured_copy()))
    }
}
fn table_exists(conn:&Connection,name:&str)->Result<bool,&'static str>{
    conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[name],|r|r.get(0)).map_err(|_|STORAGE)
}
/// Called in the SAME transaction as RunQuiescence/OS-lock-proven release.
/// A phase, timestamp, peer ACK or abandoned consumer is not this authority.
pub(in super::super) fn retire_run_associations(conn:&Connection,run:&RunToken)->Result<(),&'static str>{
    if table_exists(conn,"app_voice_im_notifications")?{
        conn.execute("DELETE FROM app_voice_im_notifications WHERE line_id=?1 AND revision=?2 AND run_nonce=?3",
            params![run.line_id(),run.revision()as i64,run.nonce()]).map_err(|_|STORAGE)?;
    }
    Ok(())
}
/// Only explicit, already-authorized account deletion calls this after its
/// private account row was deleted in the same transaction. Never delete IM
/// message receipts to make room while the account still exists.
pub(in super::super) fn delete_account(conn:&Connection,line:&str)->Result<(),&'static str>{
    if account_store::metadata(conn,line)?.is_some(){return Err(SCOPE);}
    if table_exists(conn,"app_voice_im_notifications")?{conn.execute("DELETE FROM app_voice_im_notifications WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}
    if table_exists(conn,"app_voice_im_messages")?{conn.execute("DELETE FROM app_voice_im_messages WHERE line_id=?1",[line]).map_err(|_|STORAGE)?;}
    super::cursor::delete_account(conn,line)?;
    super::statuses::delete_account(conn,line)?;
    super::receipts::delete_account(conn,line)?;
    super::user_reads::delete_account(conn,line)?;
    Ok(())
}
