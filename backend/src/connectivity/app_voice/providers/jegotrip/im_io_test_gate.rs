//! Actions-only gated ORIGINAL physical IO for IM partial-write cancellation.
//! It wraps the same owned stream, never replaces a scope or opens a socket.
use super::*;
use std::{pin::Pin,sync::{Arc,atomic::{AtomicBool,AtomicUsize,Ordering}},task::{Context,Poll}};
struct GatedImIo{inner:BoxRuntimeIo,blocked:Arc<AtomicBool>,pauses:Arc<AtomicUsize>}
impl tokio::io::AsyncRead for GatedImIo{
    fn poll_read(self:Pin<&mut Self>,cx:&mut Context<'_>,buf:&mut tokio::io::ReadBuf<'_>)->Poll<std::io::Result<()>>{tokio::io::AsyncRead::poll_read(Pin::new(&mut *self.get_mut().inner),cx,buf)}
}
impl tokio::io::AsyncWrite for GatedImIo{
    fn poll_write(self:Pin<&mut Self>,cx:&mut Context<'_>,buf:&[u8])->Poll<std::io::Result<usize>>{
        let this=self.get_mut();if this.blocked.load(Ordering::SeqCst){this.pauses.fetch_add(1,Ordering::SeqCst);return Poll::Pending;}
        tokio::io::AsyncWrite::poll_write(Pin::new(&mut *this.inner),cx,buf)
    }
    fn poll_flush(self:Pin<&mut Self>,cx:&mut Context<'_>)->Poll<std::io::Result<()>>{tokio::io::AsyncWrite::poll_flush(Pin::new(&mut *self.get_mut().inner),cx)}
    fn poll_shutdown(self:Pin<&mut Self>,cx:&mut Context<'_>)->Poll<std::io::Result<()>>{tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut *self.get_mut().inner),cx)}
}
pub(crate) fn gate_original_write(path:&mut RouterPath,blocked:Arc<AtomicBool>)->Arc<AtomicUsize>{
    let inner=path.io.take().expect("same original physical fixture stream");let pauses=Arc::new(AtomicUsize::new(0));path.io=Some(Box::new(GatedImIo{inner,blocked,pauses:pauses.clone()}));pauses
}
