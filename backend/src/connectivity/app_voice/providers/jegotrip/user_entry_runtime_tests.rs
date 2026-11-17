//! Actions-only bounded original auxiliary ownership and physical poll gates.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip as n;
use std::{sync::{Arc,atomic::{AtomicBool,AtomicUsize,Ordering}},time::{Duration,Instant},io,pin::Pin,task::{Context,Poll}};
use tokio::io::{AsyncRead,AsyncWrite,ReadBuf,AsyncWriteExt};
#[test]
fn user_entry_runtime_slots_keep_positive_monotonic_ids_and_never_reuse_after_drop(){
    let who=owner();let parent=RpcConnectionScope::fresh();let book=n::user_entry_ownership::UserEntryBook::original(who.clone(),parent.clone());let end=Instant::now()+Duration::from_secs(30);
    let a=book.acquire(&who,&parent,end).unwrap();assert_eq!(a.id(),1);let child=a.scope();drop(a);assert!(!child.is_open());let b=book.acquire(&who,&parent,end).unwrap();assert_eq!(b.id(),2);assert_eq!(book.active(),1);drop(b);assert_eq!(book.active(),0);assert!(parent.is_open());
}
#[test]
fn user_entry_runtime_foreign_parent_owner_or_expired_request_allocates_no_slot(){
    let who=owner();let parent=RpcConnectionScope::fresh();let book=n::user_entry_ownership::UserEntryBook::original(who.clone(),parent.clone());let end=Instant::now()+Duration::from_secs(30);
    assert!(book.acquire(&owner(),&parent,end).is_err());assert!(book.acquire(&who,&RpcConnectionScope::fresh(),end).is_err());assert!(book.acquire(&who,&parent,Instant::now()).is_err());parent.mark_session_revoked();assert!(book.acquire(&who,&parent,end).is_err());assert_eq!(book.active(),0);
}
#[test]
fn user_entry_runtime_pending_bytes_and_four_slots_include_inflight_until_owner_drop(){
    let who=owner();let parent=RpcConnectionScope::fresh();let book=n::user_entry_ownership::UserEntryBook::original(who.clone(),parent.clone());let end=Instant::now()+Duration::from_secs(30);
    let mut leases=Vec::new();for _ in 0..4{leases.push(book.acquire(&who,&parent,end).unwrap());}assert!(book.acquire(&who,&parent,end).is_err());leases[0].arm(65536).unwrap();leases[1].arm(65536).unwrap();assert_eq!(book.retained_bytes(),131072);assert!(leases[0].arm(1).is_err());assert!(leases[2].arm(1).is_err());leases.remove(0);assert_eq!(book.retained_bytes(),65536);leases[1].arm(1).unwrap();drop(leases);assert_eq!(book.retained_bytes(),0);assert_eq!(book.active(),0);
}
#[tokio::test]
async fn user_entry_runtime_retire_is_not_release_and_original_stop_waits_for_owned_drop(){
    let who=owner();let parent=RpcConnectionScope::fresh();let book=n::user_entry_ownership::UserEntryBook::original(who.clone(),parent.clone());let lease=book.acquire(&who,&parent,Instant::now()+Duration::from_secs(30)).unwrap();let child=lease.scope();
    let mut stopping=Box::pin(book.retired_and_empty());assert!(tokio::time::timeout(Duration::from_millis(20),&mut stopping).await.is_err());assert!(!child.is_open());assert_eq!(book.active(),1);assert!(parent.is_open());drop(lease);tokio::time::timeout(Duration::from_secs(1),stopping).await.unwrap();assert_eq!(book.active(),0);assert!(book.acquire(&who,&parent,Instant::now()+Duration::from_secs(1)).is_err());
}
#[test]
fn user_entry_runtime_true_restart_cannot_rebind_old_lease_to_new_parent(){
    let who=owner();let first=RpcConnectionScope::fresh();let old=n::user_entry_ownership::UserEntryBook::original(who.clone(),first.clone());let lease=old.acquire(&who,&first,Instant::now()+Duration::from_secs(30)).unwrap();old.retire();first.close();let next=RpcConnectionScope::fresh();let fresh=n::user_entry_ownership::UserEntryBook::original(who.clone(),next.clone());assert!(!lease.matches(&who,&next));assert!(fresh.acquire(&who,&next,Instant::now()+Duration::from_secs(1)).is_ok());drop(lease);assert_eq!(old.active(),0);
}
struct Physical{polls:Arc<AtomicUsize>,dropped:Arc<AtomicBool>}
impl Drop for Physical{fn drop(&mut self){self.dropped.store(true,Ordering::SeqCst);}}
impl AsyncRead for Physical{fn poll_read(self:Pin<&mut Self>,_:&mut Context<'_>,_:&mut ReadBuf<'_>)->Poll<io::Result<()>>{self.polls.fetch_add(1,Ordering::SeqCst);Poll::Pending}}
impl AsyncWrite for Physical{
    fn poll_write(self:Pin<&mut Self>,_:&mut Context<'_>,b:&[u8])->Poll<io::Result<usize>>{self.polls.fetch_add(1,Ordering::SeqCst);Poll::Ready(Ok(b.len()))}
    fn poll_flush(self:Pin<&mut Self>,_:&mut Context<'_>)->Poll<io::Result<()>>{self.polls.fetch_add(1,Ordering::SeqCst);Poll::Ready(Ok(()))}
    fn poll_shutdown(self:Pin<&mut Self>,_:&mut Context<'_>)->Poll<io::Result<()>>{self.polls.fetch_add(1,Ordering::SeqCst);Poll::Ready(Ok(()))}
}
#[tokio::test]
async fn user_entry_runtime_outer_router_child_gate_and_parent_gate_do_not_reenter_mutex(){
    let parent=RpcConnectionScope::fresh();let child=RpcConnectionScope::fresh();let polls=Arc::new(AtomicUsize::new(0));let dropped=Arc::new(AtomicBool::new(false));let mut io=n::user_entry_io::UserEntryIo::original(Box::new(Physical{polls:polls.clone(),dropped:dropped.clone()}),parent.clone(),child.clone(),Instant::now()+Duration::from_secs(30)).unwrap();
    let write=n::router_connector::poll_original(&child,io.write(b"one original poll"));let count=tokio::time::timeout(Duration::from_secs(1),write).await.unwrap().unwrap().unwrap();assert_eq!(count,17);assert_eq!(polls.load(Ordering::SeqCst),1);parent.mark_session_revoked();let e=n::router_connector::poll_original(&child,io.write(b"must not poll physical IO")).await.unwrap().unwrap_err();assert_eq!(e.kind(),io::ErrorKind::PermissionDenied);assert_eq!(polls.load(Ordering::SeqCst),1);assert!(parent.is_open());drop(io);assert!(dropped.load(Ordering::SeqCst));
}
#[tokio::test]
async fn user_entry_runtime_parent_close_blocks_all_physical_poll_kinds_without_closing_another_scope(){
    let parent=RpcConnectionScope::fresh();let child=RpcConnectionScope::fresh();let polls=Arc::new(AtomicUsize::new(0));let dropped=Arc::new(AtomicBool::new(false));let mut io=n::user_entry_io::UserEntryIo::original(Box::new(Physical{polls:polls.clone(),dropped:dropped.clone()}),parent.clone(),child.clone(),Instant::now()+Duration::from_secs(30)).unwrap();parent.close();assert!(io.write(b"no output").await.is_err());assert!(io.flush().await.is_err());assert!(io.shutdown().await.is_err());let mut buffer=[0;1];assert!(tokio::io::AsyncReadExt::read(&mut io,&mut buffer).await.is_err());assert_eq!(polls.load(Ordering::SeqCst),0);assert!(child.is_open());drop(io);assert!(dropped.load(Ordering::SeqCst));
}
#[test]
fn user_entry_runtime_debug_never_exports_owner_recipient_reference_or_sdk_material(){
    let who=owner();let parent=RpcConnectionScope::fresh();let gate=n::im_inbox::ImReceiptGate::fresh();let action=n::user_entry_action::UserEntryAction::original(who.clone(),parent.clone(),gate,Instant::now()+Duration::from_secs(30),b"[phone:+12345@fixture.invalid]").unwrap();let book=n::user_entry_ownership::UserEntryBook::original(who.clone(),parent.clone());let lease=book.acquire(&who,&parent,action.deadline()).unwrap();let text=format!("{action:?} {book:?} {lease:?}");for secret in ["fixture","12345","owner","scope"]{assert!(!text.contains(secret));}
}
#[test]
fn user_entry_runtime_typed_original_timeouts_survive_every_transport_error_boundary(){
    use n::{user_entry_rpc::{self,UserEntryError},common_runtime::CommonIoError,netstream_runtime::NetstreamError,router_runtime::RouterIoError,runtime_connector::ConnectError,tls_engine::TlsError,channel_session::SessionError,reliable_channel::ChannelError};
    for e in [CommonIoError::Timeout,CommonIoError::Netstream(NetstreamError::Timeout),CommonIoError::Netstream(NetstreamError::Router(RouterIoError::Timeout)),CommonIoError::Netstream(NetstreamError::Router(RouterIoError::Connect(ConnectError::Timeout))),CommonIoError::Netstream(NetstreamError::Router(RouterIoError::Tls(TlsError::Timeout))),CommonIoError::Netstream(NetstreamError::Session(SessionError::Channel(ChannelError::Timeout)))]{assert_eq!(user_entry_rpc::common_error(e),UserEntryError::Timeout);}
}
#[test]
fn user_entry_runtime_io_scope_protocol_or_tls_failure_does_not_become_timeout_or_retry(){
    use n::{user_entry_rpc::{self,UserEntryError},common_runtime::CommonIoError,netstream_runtime::NetstreamError,router_runtime::RouterIoError,runtime_connector::ConnectError,tls_engine::TlsError};
    for e in [CommonIoError::Scope,CommonIoError::Configuration,CommonIoError::Netstream(NetstreamError::Router(RouterIoError::Io)),CommonIoError::Netstream(NetstreamError::Router(RouterIoError::Connect(ConnectError::Io))),CommonIoError::Netstream(NetstreamError::Router(RouterIoError::Tls(TlsError::HandshakeFailed)))]{assert_eq!(user_entry_rpc::common_error(e),UserEntryError::Transport);}
}
