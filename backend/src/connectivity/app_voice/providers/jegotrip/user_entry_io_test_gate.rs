//! Actions-only fault gate around the SAME owned auxiliary Router TCP stream.
//! A real one-byte ciphertext prefix is accepted before the next write pauses.
//! No replacement IO, new socket, scope, task, timeout or business bytes.
use super::*;
use std::{pin::Pin,sync::{Arc,Mutex,atomic::{AtomicBool,AtomicUsize,Ordering}},task::{Context,Poll}};
pub(crate) struct AuxiliaryIoTest{pub(crate) accepted:AtomicUsize,pub(crate) pauses:AtomicUsize,pub(crate) dropped:AtomicBool,
    prefix:usize,armed:AtomicBool,scope:Mutex<Option<RpcConnectionScope>>,changed:tokio::sync::Notify}
impl AuxiliaryIoTest{
    pub(crate) fn original(prefix:usize)->Arc<Self>{Self::with_activation(prefix,true)}
    pub(crate) fn original_unarmed(prefix:usize)->Arc<Self>{Self::with_activation(prefix,false)}
    fn with_activation(prefix:usize,armed:bool)->Arc<Self>{Arc::new(Self{accepted:AtomicUsize::new(0),pauses:AtomicUsize::new(0),dropped:AtomicBool::new(false),prefix,armed:AtomicBool::new(armed),scope:Mutex::new(None),changed:tokio::sync::Notify::new()})}
    pub(crate) fn arm(&self){assert!(!self.armed.swap(true,Ordering::SeqCst));assert_eq!(self.accepted.load(Ordering::SeqCst),0);}
    pub(crate) fn scope(&self)->RpcConnectionScope{self.scope.lock().unwrap().clone().expect("actual original auxiliary scope captured")}
    pub(crate) async fn paused(&self){loop{let changed=self.changed.notified();tokio::pin!(changed);changed.as_mut().enable();if self.pauses.load(Ordering::SeqCst)>0{return;}changed.await;}}
}
struct PrefixIo{inner:Option<BoxRuntimeIo>,test:Arc<AuxiliaryIoTest>}
impl Drop for PrefixIo{fn drop(&mut self){drop(self.inner.take());self.test.dropped.store(true,Ordering::SeqCst);}}
impl tokio::io::AsyncRead for PrefixIo{
    fn poll_read(self:Pin<&mut Self>,cx:&mut Context<'_>,buf:&mut tokio::io::ReadBuf<'_>)->Poll<std::io::Result<()>>{
        tokio::io::AsyncRead::poll_read(Pin::new(&mut **self.get_mut().inner.as_mut().unwrap()),cx,buf)
    }
}
impl tokio::io::AsyncWrite for PrefixIo{
    fn poll_write(self:Pin<&mut Self>,cx:&mut Context<'_>,bytes:&[u8])->Poll<std::io::Result<usize>>{
        let this=self.get_mut();if !this.test.armed.load(Ordering::SeqCst){return tokio::io::AsyncWrite::poll_write(Pin::new(&mut **this.inner.as_mut().unwrap()),cx,bytes);}let left=this.test.prefix.saturating_sub(this.test.accepted.load(Ordering::SeqCst));
        if left==0{this.test.pauses.fetch_add(1,Ordering::SeqCst);this.test.changed.notify_waiters();return Poll::Pending;}
        let result=tokio::io::AsyncWrite::poll_write(Pin::new(&mut **this.inner.as_mut().unwrap()),cx,&bytes[..bytes.len().min(left)]);
        if let Poll::Ready(Ok(n))=result{this.test.accepted.fetch_add(n,Ordering::SeqCst);}result
    }
    fn poll_flush(self:Pin<&mut Self>,cx:&mut Context<'_>)->Poll<std::io::Result<()>>{tokio::io::AsyncWrite::poll_flush(Pin::new(&mut **self.get_mut().inner.as_mut().unwrap()),cx)}
    fn poll_shutdown(self:Pin<&mut Self>,cx:&mut Context<'_>)->Poll<std::io::Result<()>>{tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut **self.get_mut().inner.as_mut().unwrap()),cx)}
}
pub(crate) fn gate_auxiliary_prefix(path:&mut RouterPath,test:Arc<AuxiliaryIoTest>){
    assert!(test.scope.lock().unwrap().replace(path.scope.clone()).is_none());let inner=path.io.take().expect("same original physical stream");
    path.io=Some(Box::new(PrefixIo{inner:Some(inner),test}));
}
