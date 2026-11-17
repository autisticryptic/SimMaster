//! Current original actor capture only; it emits no RPC and allocates no ID.
use super::*;
impl RuntimeCommands{
    pub(in super::super) async fn original_user_entry_auth(&self,owner:&AttemptOwner,parent:&RpcConnectionScope)->Result<Box<super::super::arc_session::UserEntryAuth>,RuntimeError>{
        if !self.accepts(owner,parent){return Err(RuntimeError::Scope);}
        let(reply,result)=oneshot::channel();let submitted=self.submit(owner,parent,Operation::GetUserEntryAuth(reply)).await?;
        if !submitted.matches(owner,parent){return Err(RuntimeError::Scope);}
        let auth=result.await.map_err(|_|RuntimeError::Closed)?.ok_or(RuntimeError::Scope)?;
        if !auth.matches(owner,parent){return Err(RuntimeError::Scope);}Ok(auth)
    }
}
