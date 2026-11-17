//! Parent and auxiliary scope gates around every nonblocking physical poll.
//! Router owns the outer auxiliary gate; this wrapper takes ONLY the parent
//! gate. Re-taking the child gate here would deadlock its non-reentrant mutex.
//! Parent revocation prevents even a previously borrowed RPC token's late IO.
use super::{rpc_pending::RpcConnectionScope,runtime_connector::BoxRuntimeIo};
use std::{io,pin::Pin,task::{Context,Poll},time::Instant};
use tokio::io::{AsyncRead,AsyncWrite,ReadBuf};
pub(super) struct UserEntryIo{inner:BoxRuntimeIo,parent:RpcConnectionScope,auxiliary:RpcConnectionScope,deadline:Instant}
impl UserEntryIo{
    pub(super) fn original(inner:BoxRuntimeIo,parent:RpcConnectionScope,auxiliary:RpcConnectionScope,deadline:Instant)->io::Result<Self>{
        if parent.matches(&auxiliary)||!parent.is_open()||parent.session_revoked()||!auxiliary.is_open()||Instant::now()>=deadline{return Err(closed());}
        Ok(Self{inner,parent,auxiliary,deadline})
    }
    fn current(&self)->bool{self.parent.is_open()&&!self.parent.session_revoked()&&self.auxiliary.is_open()&&Instant::now()<self.deadline}
}
fn closed()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"original UserEntry owner retired")}
impl AsyncRead for UserEntryIo{
    fn poll_read(mut self:Pin<&mut Self>,cx:&mut Context<'_>,buf:&mut ReadBuf<'_>)->Poll<io::Result<()>>{
        let parent=self.parent.clone();
        parent.with_io_gate(||if self.current(){Pin::new(self.inner.as_mut()).poll_read(cx,buf)}else{Poll::Ready(Err(closed()))}).unwrap_or_else(||Poll::Ready(Err(closed())))
    }
}
impl AsyncWrite for UserEntryIo{
    fn poll_write(mut self:Pin<&mut Self>,cx:&mut Context<'_>,buf:&[u8])->Poll<io::Result<usize>>{
        let parent=self.parent.clone();
        parent.with_io_gate(||if self.current(){Pin::new(self.inner.as_mut()).poll_write(cx,buf)}else{Poll::Ready(Err(closed()))}).unwrap_or_else(||Poll::Ready(Err(closed())))
    }
    fn poll_flush(mut self:Pin<&mut Self>,cx:&mut Context<'_>)->Poll<io::Result<()>>{
        let parent=self.parent.clone();
        parent.with_io_gate(||if self.current(){Pin::new(self.inner.as_mut()).poll_flush(cx)}else{Poll::Ready(Err(closed()))}).unwrap_or_else(||Poll::Ready(Err(closed())))
    }
    fn poll_shutdown(mut self:Pin<&mut Self>,cx:&mut Context<'_>)->Poll<io::Result<()>>{
        let parent=self.parent.clone();
        parent.with_io_gate(||if self.current(){Pin::new(self.inner.as_mut()).poll_shutdown(cx)}else{Poll::Ready(Err(closed()))}).unwrap_or_else(||Poll::Ready(Err(closed())))
    }
}
