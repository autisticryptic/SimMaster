//! Actions-only per-action admission barrier and resumable gate around the SAME
//! already connected original Router TCP owner. No socket, task, auth or grant.
use super::*;
use std::{pin::Pin, sync::{Arc, Mutex, atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering}}, task::{Context, Poll, Waker}};
#[derive(Clone, Copy)]
pub(crate) enum ClientReadTestPoint { Admission, Prefix(usize) }
pub(crate) struct ClientReadIoTest {
    point: ClientReadTestPoint, entered: AtomicBool, released: AtomicBool,
    sequence: AtomicI32, scope: Mutex<Option<RpcConnectionScope>>, changed: tokio::sync::Notify,
    wake: Mutex<Option<Waker>>, pub(crate) accepted: AtomicUsize,
    pub(crate) pauses: AtomicUsize, pub(crate) dropped: AtomicBool,
}
impl std::fmt::Debug for ClientReadIoTest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("ClientReadIoTest(<redacted>)") }
}
impl ClientReadIoTest {
    pub(crate) fn original(point: ClientReadTestPoint) -> Arc<Self> {
        Arc::new(Self { point, entered: AtomicBool::new(false), released: AtomicBool::new(false),
            sequence: AtomicI32::new(0), scope: Mutex::new(None), changed: tokio::sync::Notify::new(),
            wake: Mutex::new(None), accepted: AtomicUsize::new(0), pauses: AtomicUsize::new(0), dropped: AtomicBool::new(false) })
    }
    pub(crate) fn is_prefix(&self) -> bool { matches!(self.point, ClientReadTestPoint::Prefix(_)) }
    pub(crate) fn sequence(&self) -> i32 { self.sequence.load(Ordering::SeqCst) }
    pub(crate) fn scope(&self) -> RpcConnectionScope { self.scope.lock().unwrap().clone().expect("actual original actor scope") }
    pub(crate) async fn after_admission(&self, sequence: i32, scope: &RpcConnectionScope) {
        assert!(sequence > 0); assert_eq!(self.sequence.swap(sequence, Ordering::SeqCst), 0);
        assert!(self.scope.lock().unwrap().replace(scope.clone()).is_none());
        self.entered.store(true, Ordering::SeqCst); self.changed.notify_waiters();
        if matches!(self.point, ClientReadTestPoint::Admission) {
            loop {
                let changed = self.changed.notified(); tokio::pin!(changed); changed.as_mut().enable();
                if self.released.load(Ordering::SeqCst) || !scope.is_open() { return; }
                tokio::select! { biased; _ = scope.closed() => return, _ = changed => {} }
            }
        }
    }
    pub(crate) async fn entered(&self) {
        loop { let changed = self.changed.notified(); tokio::pin!(changed); changed.as_mut().enable();
            if self.entered.load(Ordering::SeqCst) { return; } changed.await; }
    }
    pub(crate) async fn paused(&self) {
        loop { let changed = self.changed.notified(); tokio::pin!(changed); changed.as_mut().enable();
            if self.pauses.load(Ordering::SeqCst) > 0 { return; } changed.await; }
    }
    pub(crate) fn release(&self) {
        assert!(!self.released.swap(true, Ordering::SeqCst));
        if let Some(wake) = self.wake.lock().unwrap().take() { wake.wake(); }
        self.changed.notify_waiters();
    }
}
struct ClientPrefixIo { inner: Option<BoxRuntimeIo>, test: Arc<ClientReadIoTest> }
impl Drop for ClientPrefixIo {
    fn drop(&mut self) { drop(self.inner.take()); self.test.dropped.store(true, Ordering::SeqCst); self.test.changed.notify_waiters(); }
}
impl tokio::io::AsyncRead for ClientPrefixIo {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut tokio::io::ReadBuf<'_>) -> Poll<std::io::Result<()>> {
        tokio::io::AsyncRead::poll_read(Pin::new(&mut **self.get_mut().inner.as_mut().unwrap()), cx, buf)
    }
}
impl tokio::io::AsyncWrite for ClientPrefixIo {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, bytes: &[u8]) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut(); let ClientReadTestPoint::Prefix(prefix) = this.test.point else { unreachable!() };
        let released = this.test.released.load(Ordering::SeqCst);
        let left = prefix.saturating_sub(this.test.accepted.load(Ordering::SeqCst));
        if !released && left == 0 {
            *this.test.wake.lock().unwrap() = Some(cx.waker().clone());
            this.test.pauses.fetch_add(1, Ordering::SeqCst); this.test.changed.notify_waiters();
            // Register before the second check: release cannot lose a wake.
            if !this.test.released.load(Ordering::SeqCst) { return Poll::Pending; }
        }
        let count = if this.test.released.load(Ordering::SeqCst) { bytes.len() } else { bytes.len().min(left) };
        let result = tokio::io::AsyncWrite::poll_write(Pin::new(&mut **this.inner.as_mut().unwrap()), cx, &bytes[..count]);
        if let Poll::Ready(Ok(n)) = result { this.test.accepted.fetch_add(n, Ordering::SeqCst); }
        result
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_flush(Pin::new(&mut **self.get_mut().inner.as_mut().unwrap()), cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        tokio::io::AsyncWrite::poll_shutdown(Pin::new(&mut **self.get_mut().inner.as_mut().unwrap()), cx)
    }
}
pub(crate) fn gate_client_prefix(path: &mut RouterPath, test: Arc<ClientReadIoTest>) {
    assert!(test.is_prefix()); assert!(test.scope().matches(&path.scope));
    let inner = path.io.take().expect("same already connected original TCP stream");
    path.io = Some(Box::new(ClientPrefixIo { inner: Some(inner), test }));
}
