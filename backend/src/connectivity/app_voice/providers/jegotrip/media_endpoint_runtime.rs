//! One original-call Media Endpoint TCP/TLS owner, independent of account IO.
//! Only isolated peers execute in Actions; no builtin target/root is supplied.
//! A positive result is Endpoint signaling, not audio or Channels readiness.
use super::{
    call_control::CallControlTicket,
    media_endpoint::{MediaEndpointPlan, MediaError, OwnedMediaEndpointReply},
    packet_transport::{OwnedWireFrames, PacketRoute},
    rpc_pending::RpcConnectionScope,
    runtime_connector::{BoxRuntimeIo, ConnectError, RuntimeConnector},
    session::AttemptOwner,
    tls_engine::{TlsEngine, TlsError, TlsParameters, TlsPhase},
    transport_packets::{encode_call_packets, CallAssembler, CallKind, TransportBytes, TLS_FRAGMENT_THRESHOLD},
};
use std::{fmt, future::{poll_fn, Future}, pin::Pin, sync::Arc, task::Poll, time::{Duration, Instant}};
use tokio::{io::{AsyncRead, AsyncWrite, ReadBuf}, sync::{oneshot, Mutex}, task::JoinHandle};
use zeroize::Zeroizing;

pub(super) struct MediaEndpointHandle {
    scope: RpcConnectionScope,
    join: Mutex<Option<JoinHandle<Result<(), MediaError>>>>,
}
impl fmt::Debug for MediaEndpointHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("MediaEndpointHandle(<redacted>)") }
}
impl Drop for MediaEndpointHandle { fn drop(&mut self) { self.scope.close(); } }
pub(super) struct MediaEndpointClosed { io_released: bool, pub reason: Option<MediaError> }
impl MediaEndpointClosed { pub(super) fn io_released(&self) -> bool { self.io_released } }
impl MediaEndpointHandle {
    pub(super) fn scope(&self) -> RpcConnectionScope { self.scope.clone() }
    /// Retain the owning JoinHandle in its mutex across await. Cancelling this
    /// future cannot detach the original owner or manufacture a release proof.
    pub(super) async fn close(&self) -> MediaEndpointClosed {
        self.scope.close();
        let mut join = self.join.lock().await;
        let Some(original) = join.as_mut() else {
            return MediaEndpointClosed { io_released: true, reason: None };
        };
        let result = original.await;
        join.take();
        MediaEndpointClosed { io_released: true, reason: match result {
            Ok(Ok(())) => None, Ok(Err(error)) => Some(error), Err(_) => Some(MediaError::Protocol),
        } }
    }
}
struct Retirement(RpcConnectionScope);
impl Drop for Retirement { fn drop(&mut self) { self.0.close(); } }

/// Dropping the semantic waiter retires its destination in every phase,
/// including connect and initial writes. Only the owning handle gives Join proof.
pub(super) struct MediaEndpointReady {
    scope: RpcConnectionScope,
    ticket: CallControlTicket,
    result: oneshot::Receiver<Result<OwnedMediaEndpointReply, MediaError>>,
    delivered: bool,
}
impl fmt::Debug for MediaEndpointReady {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("MediaEndpointReady(<redacted>)") }
}
impl Drop for MediaEndpointReady {
    fn drop(&mut self) { if !self.delivered { self.scope.close(); } }
}
impl MediaEndpointReady {
    fn delivered_result(&mut self, mut result: Result<OwnedMediaEndpointReply, MediaError>) -> Result<OwnedMediaEndpointReply, MediaError> {
        if result.is_ok() && (!self.scope.is_open() || !self.ticket.is_live()) { result = Err(MediaError::Scope); }
        if result.is_err() { self.scope.close(); }
        self.delivered = true;
        result
    }
    #[cfg(test)]
    pub(super) fn try_recv(&mut self) -> Result<Result<OwnedMediaEndpointReply, MediaError>, oneshot::error::TryRecvError> {
        match self.result.try_recv() { Ok(result) => Ok(self.delivered_result(result)), Err(error) => Err(error) }
    }
}
impl Future for MediaEndpointReady {
    type Output = Result<Result<OwnedMediaEndpointReply, MediaError>, oneshot::error::RecvError>;
    fn poll(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        match Pin::new(&mut this.result).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(result)) => Poll::Ready(Ok(this.delivered_result(result))),
            Poll::Ready(Err(error)) => { this.scope.close(); this.delivered = true; Poll::Ready(Err(error)) }
        }
    }
}
pub(super) fn spawn(plan: MediaEndpointPlan, connector: Arc<dyn RuntimeConnector>) -> (MediaEndpointHandle, MediaEndpointReady) {
    let scope = plan.scope();
    let ticket = plan.ticket();
    let actor_scope = scope.clone();
    let (completion, result) = oneshot::channel();
    let join = tokio::spawn(async move {
        let _retirement = Retirement(actor_scope);
        let mut completion = Some(completion);
        let result = run(plan, connector, &mut completion).await;
        if let Some(completion) = completion.take() {
            let _ = completion.send(Err(result.as_ref().err().copied().unwrap_or(MediaError::Closed)));
        }
        result
    });
    (MediaEndpointHandle { scope: scope.clone(), join: Mutex::new(Some(join)) },
        MediaEndpointReady { scope, ticket, result, delivered: false })
}

fn earlier(left: Option<Instant>, right: Option<Instant>) -> Option<Instant> {
    match (left, right) {
        (Some(a), Some(b)) => Some(a.min(b)), (Some(a), None) | (None, Some(a)) => Some(a), (None, None) => None,
    }
}
fn check_deadline(value: Option<Instant>) -> Result<(), MediaError> {
    if value.is_some_and(|deadline| Instant::now() >= deadline) { Err(MediaError::Timeout) } else { Ok(()) }
}
fn io_deadline(plan: &MediaEndpointPlan, absolute: Option<Instant>) -> Result<Option<Instant>, MediaError> {
    let local = Instant::now().checked_add(plan.profile().io_timeout).ok_or(MediaError::Configuration)?;
    Ok(earlier(absolute, Some(local)))
}
fn tls_error(error: TlsError) -> MediaError {
    match error { TlsError::Timeout => MediaError::Timeout, TlsError::Closed => MediaError::Closed, _ => MediaError::Protocol }
}
fn connect_error(error: ConnectError) -> MediaError {
    match error { ConnectError::Timeout => MediaError::Timeout, ConnectError::Scope => MediaError::Scope, _ => MediaError::Io }
}
async fn deadline(value: Option<Instant>) {
    if let Some(value) = value { tokio::time::sleep_until(value.into()).await; } else { std::future::pending::<()>().await; }
}

/// Connector implementations gate their own destination polls. Only the parent
/// call/account gate surrounds the future; recursively taking the media scope
/// gate would deadlock the production connector. TLS's original absolute clock
/// also covers DNS/connect, not a second handshake budget after connect ends.
async fn connect_original(plan: &MediaEndpointPlan, connector: &dyn RuntimeConnector, absolute: Option<Instant>) -> Result<BoxRuntimeIo, MediaError> {
    let owner = plan.owner();
    let scope = plan.scope();
    let ticket = plan.ticket();
    let input = plan.connection()?;
    let limit = io_deadline(plan, absolute)?;
    check_deadline(limit)?;
    let connect = connector.connect(owner, &scope, input);
    tokio::pin!(connect);
    let guarded = poll_fn(|cx| {
        if check_deadline(limit).is_err() { return Poll::Ready(Err(ConnectError::Timeout)); }
        match ticket.with_media_gate(|| connect.as_mut().poll(cx)) {
            Some(poll) => poll, None => Poll::Ready(Err(ConnectError::Scope)),
        }
    });
    let connected = tokio::select! { biased;
        _ = ticket.revoked() => return Err(MediaError::Scope),
        _ = scope.closed() => return Err(MediaError::Closed),
        _ = deadline(limit) => return Err(MediaError::Timeout),
        result = guarded => result.map_err(connect_error)?,
    };
    check_deadline(limit)?;
    connected.into_original_io(owner, &scope, plan.connection()?).map_err(|_| MediaError::Scope)
}

/// One partial-write/flush group uses a fixed local deadline intersected with
/// the ORIGINAL TLS/RPC deadline. Every poll rechecks it, even an immediately
/// ready write. Neither partial writes nor a queued pong reopen native time.
async fn write_frames(plan: &MediaEndpointPlan, io: &mut BoxRuntimeIo, groups: Vec<OwnedWireFrames>, absolute: Option<Instant>) -> Result<(), MediaError> {
    check_deadline(absolute)?;
    if groups.is_empty() { return Ok(()); }
    let owner = plan.owner();
    let scope = plan.scope();
    let ticket = plan.ticket();
    let limit = io_deadline(plan, absolute)?;
    let write = async {
        for group in &groups {
            let frames = group.frames_for(owner, &scope).ok_or(MediaError::Scope)?;
            for frame in frames {
                let bytes = frame.expose_to_transport();
                let mut offset = 0;
                while offset < bytes.len() {
                    let count = poll_fn(|cx| {
                        if let Err(error) = check_deadline(limit) { return Poll::Ready(Err(error)); }
                        match plan.with_io_gate(|| Pin::new(&mut *io).poll_write(cx, &bytes[offset..])) {
                            Some(poll) => poll.map(|result| result.map_err(|_| MediaError::Io)),
                            None => Poll::Ready(Err(MediaError::Scope)),
                        }
                    }).await?;
                    if count == 0 || count > bytes.len() - offset { return Err(MediaError::Io); }
                    offset += count;
                }
            }
        }
        poll_fn(|cx| {
            if let Err(error) = check_deadline(limit) { return Poll::Ready(Err(error)); }
            match plan.with_io_gate(|| Pin::new(&mut *io).poll_flush(cx)) {
                Some(poll) => poll.map(|result| result.map_err(|_| MediaError::Io)),
                None => Poll::Ready(Err(MediaError::Scope)),
            }
        }).await
    };
    tokio::select! { biased;
        _ = ticket.revoked() => Err(MediaError::Scope),
        _ = scope.closed() => Err(MediaError::Closed),
        _ = deadline(limit) => Err(MediaError::Timeout),
        result = write => result,
    }
}
async fn send_packets(plan: &MediaEndpointPlan, io: &mut BoxRuntimeIo, tls: &mut TlsEngine, packets: &[TransportBytes], rpc_deadline: Option<Instant>) -> Result<(), MediaError> {
    let absolute = earlier(tls.next_deadline(), rpc_deadline);
    check_deadline(absolute)?;
    let wire = tls.encrypt_packets(plan.owner(), &plan.scope(), packets, None, Instant::now()).map_err(tls_error)?.ok_or(MediaError::Scope)?;
    write_frames(plan, io, wire, absolute).await
}
async fn read_original(plan: &MediaEndpointPlan, io: &mut BoxRuntimeIo, bytes: &mut [u8]) -> Result<usize, MediaError> {
    poll_fn(|cx| {
        let mut buffer = ReadBuf::new(bytes);
        match plan.with_io_gate(|| Pin::new(&mut *io).poll_read(cx, &mut buffer)) {
            Some(Poll::Ready(Ok(()))) => Poll::Ready(Ok(buffer.filled().len())),
            Some(Poll::Ready(Err(_))) => Poll::Ready(Err(MediaError::Io)),
            Some(Poll::Pending) => Poll::Pending, None => Poll::Ready(Err(MediaError::Scope)),
        }
    }).await
}
/// Publication is a last original-call/destination/time gate, not the earlier
/// read's snapshot. A response read before expiry cannot become success after
/// control output blocks or cancellation wins between parse and publication.
fn publish_original(plan: &MediaEndpointPlan, completion: &mut Option<oneshot::Sender<Result<OwnedMediaEndpointReply, MediaError>>>, reply: OwnedMediaEndpointReply, absolute: Option<Instant>) -> Result<(), MediaError> {
    check_deadline(absolute)?;
    let sender = completion.take().ok_or(MediaError::Protocol)?;
    plan.with_io_gate(|| {
        check_deadline(absolute)?;
        sender.send(Ok(reply)).map_err(|_| MediaError::Closed)
    }).ok_or(MediaError::Scope)?
}

enum Input { Read(Result<usize, MediaError>), Deadline, Cancelled }
async fn run(mut plan: MediaEndpointPlan, connector: Arc<dyn RuntimeConnector>, completion: &mut Option<oneshot::Sender<Result<OwnedMediaEndpointReply, MediaError>>>) -> Result<(), MediaError> {
    let owner: AttemptOwner = plan.owner().clone();
    let scope = plan.scope();
    let ticket = plan.ticket();
    let profile = plan.profile();
    let input = plan.connection()?;
    // Capture root/authority and the original handshake deadline before connect.
    let mut tls = TlsEngine::for_arc_session(owner.clone(), scope.clone(), PacketRoute::DirectTlsCarrier, TlsParameters {
        authority: input.authority, root_der_base64: input.root_der_base64, enable_sni: input.enable_sni, tls_only: true,
        handshake_timeout: profile.handshake_timeout, partial_timeout: profile.partial_timeout,
    }, Instant::now()).map_err(|_| MediaError::Configuration)?;
    let mut io = connect_original(&plan, connector.as_ref(), tls.next_deadline()).await?;
    let wire = tls.take_wire_output(&owner, &scope, None, Instant::now()).map_err(tls_error)?;
    write_frames(&plan, &mut io, wire, tls.next_deadline()).await?;
    let mut assembler = CallAssembler::default();
    let mut read_buffer = Zeroizing::new(vec![0u8; 16_384]);
    let mut submitted = false;
    let mut rpc_deadline = None;
    loop {
        if !plan.live() { return Err(MediaError::Scope); }
        let step = {
            let cancel = async {
                if let Some(sender) = completion.as_mut() { sender.closed().await; } else { std::future::pending::<()>().await; }
            };
            tokio::select! { biased;
                _ = ticket.revoked() => Input::Cancelled,
                _ = scope.closed() => Input::Cancelled,
                _ = cancel => Input::Cancelled,
                _ = deadline(earlier(tls.next_deadline(), rpc_deadline)) => Input::Deadline,
                read = read_original(&plan, &mut io, &mut read_buffer) => Input::Read(read),
            }
        };
        match step {
            Input::Cancelled => return Err(MediaError::Closed),
            Input::Deadline => return Err(MediaError::Timeout),
            Input::Read(result) => {
                let count = result?;
                if count == 0 { let _ = tls.finish_eof(&owner, &scope); return Err(MediaError::Closed); }
                check_deadline(earlier(tls.next_deadline(), rpc_deadline))?;
                let progress = tls.on_wire_bytes(&owner, &scope, &read_buffer[..count], None, Instant::now()).map_err(tls_error)?.ok_or(MediaError::Scope)?;
                write_frames(&plan, &mut io, progress.wire, earlier(tls.next_deadline(), rpc_deadline)).await?;
                if !submitted && tls.phase() == TlsPhase::Verified {
                    let packets = encode_call_packets(CallKind::Request, plan.request()?, TLS_FRAGMENT_THRESHOLD).map_err(|_| MediaError::Protocol)?;
                    rpc_deadline = Some(Instant::now().checked_add(plan.profile().rpc_timeout).ok_or(MediaError::Configuration)?);
                    send_packets(&plan, &mut io, &mut tls, &packets, rpc_deadline).await?;
                    submitted = true;
                }
                let packets = progress.plaintext.packets_for(&owner, &scope).ok_or(MediaError::Scope)?;
                for packet in packets {
                    check_deadline(earlier(tls.next_deadline(), rpc_deadline))?;
                    let Some(message) = assembler.on_packet(packet.expose_to_transport()).map_err(|_| MediaError::Protocol)? else { continue; };
                    match message.kind {
                        CallKind::Reply => {
                            if !submitted { return Err(MediaError::Protocol); }
                            if let Some(reply) = plan.on_reply(&owner, &scope, message.payload.expose_to_transport())? {
                                publish_original(&plan, completion, reply, earlier(tls.next_deadline(), rpc_deadline))?;
                                rpc_deadline = None;
                            }
                        }
                        CallKind::Ping => {
                            let pong = encode_call_packets(CallKind::Pong, &[], TLS_FRAGMENT_THRESHOLD).map_err(|_| MediaError::Protocol)?;
                            send_packets(&plan, &mut io, &mut tls, &pong, rpc_deadline).await?;
                        }
                        CallKind::Pong => {}, CallKind::Release => return Err(MediaError::Closed),
                        CallKind::Request => return Err(MediaError::Protocol),
                    }
                }
            }
        }
    }
}
#[cfg(test)]
#[path = "media_endpoint_runtime_tests.rs"]
mod tests;
