//! Production raw TCP selection for the explicit native endpoint-TLS Router path.
//! A socket is not TLS/Router/SDK readiness. Every endpoint/ri/trust binding is
//! validated before DNS, and the selected immutable policy stays owner/scoped.
use super::{object_reference::parse_object_reference, rpc_pending::RpcConnectionScope,
    runtime_connector::{BoxRuntimeIo, ConnectError, RuntimeIo, same_authority, validate_tls_anchor},
    session::AttemptOwner, tls_engine::TlsParameters};
use crate::platform::dns::prepare_socket_addrs;
use std::{fmt, future::{Future, poll_fn}, net::SocketAddr, sync::Arc, task::Poll, time::Duration};
use tokio::net::TcpStream;
use zeroize::Zeroizing;

pub struct RouterAnchor<'a> { pub router_id: i32, pub host: &'a str, pub port: u16,
    pub authority: &'a str, pub root: &'a str, pub enable_sni: bool }
impl fmt::Debug for RouterAnchor<'_> { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("RouterAnchor(<redacted>)")} }
struct Target { router_id:i32, host:Zeroizing<String>, port:u16,
    authority:Zeroizing<String>, root:Zeroizing<String>, enable_sni:bool, identity:Arc<()> }
impl Target { fn same_origin(&self,a:&RouterAnchor<'_>)->bool {
    self.router_id==a.router_id && self.host.as_str()==a.host && self.port==a.port
} }
pub struct RouterPolicy { reference:Zeroizing<String>, targets:Vec<Target>, identity:Arc<()> }
impl fmt::Debug for RouterPolicy {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("RouterPolicy(<redacted>)")} }
impl RouterPolicy {
    /// All anchors are installation data, NOT inferred from decoded DomainName,
    /// an IP resolution or a provider response. No endpoint is silently dropped.
    pub(super) fn supplied(reference:&str, anchors:&[RouterAnchor<'_>])->Result<Self,ConnectError> {
        if reference.is_empty() || reference.len()>65536 || reference.chars().any(char::is_control)
            || anchors.is_empty() || anchors.len()>16 {return Err(ConnectError::InvalidConfiguration);}
        let parsed=parse_object_reference(reference.as_bytes()).map_err(|_|ConnectError::UnsupportedProfile)?;
        let groups=parsed.explicit_tls_router_groups().map_err(|_|ConnectError::UnsupportedProfile)?;
        let mut targets=Vec::new(); let mut used=vec![false;anchors.len()];
        for group in groups { for endpoint in group.endpoints() {
            let host=std::str::from_utf8(endpoint.host()).map_err(|_|ConnectError::InvalidConfiguration)?;
            let matching=anchors.iter().enumerate().filter(|(_,a)| a.router_id==group.router_id() && a.host==host && a.port==endpoint.port()).collect::<Vec<_>>();
            if matching.len()!=1 {return Err(ConnectError::InvalidConfiguration);}
            let (index,anchor)=matching[0];used[index]=true;
            validate_tls_anchor(anchor.authority,anchor.root)?;
            if !same_authority(host,anchor.authority) {return Err(ConnectError::UnsupportedProfile);}
            if targets.iter().any(|t:&Target|t.same_origin(anchor)) {continue;}
            if targets.len()==16 {return Err(ConnectError::ResourceLimit);}
            targets.push(Target{router_id:anchor.router_id,host:Zeroizing::new(host.to_owned()),port:anchor.port,
                authority:Zeroizing::new(anchor.authority.to_owned()),root:Zeroizing::new(anchor.root.to_owned()),
                enable_sni:anchor.enable_sni,identity:Arc::new(())});
        }}
        if targets.is_empty() || used.iter().any(|used|!*used) {return Err(ConnectError::InvalidConfiguration);}
        Ok(Self{reference:Zeroizing::new(reference.to_owned()),targets,identity:Arc::new(())})
    }
    pub(super) fn len(&self)->usize {self.targets.len()}
    pub(super) fn tls_for(&self,index:usize,handshake:Duration,partial:Duration)->Result<TlsParameters<'_>,ConnectError> {
        let target=self.targets.get(index).ok_or(ConnectError::Scope)?;
        Ok(TlsParameters{authority:&target.authority,root_der_base64:&target.root,tls_only:true,
            enable_sni:target.enable_sni,handshake_timeout:handshake,partial_timeout:partial})
    }
    pub(super) fn router_id(&self,index:usize)->Result<i32,ConnectError> {self.targets.get(index).map(|t|t.router_id).ok_or(ConnectError::Scope)}
}

pub struct ConnectedRouterIo { owner:AttemptOwner, scope:RpcConnectionScope,
    policy:Arc<()>, target:Arc<()>, index:usize, io:Option<BoxRuntimeIo> }
impl fmt::Debug for ConnectedRouterIo {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("ConnectedRouterIo(<redacted>)")} }
impl ConnectedRouterIo {
    pub(super) fn original<I:RuntimeIo+'static>(owner:&AttemptOwner,scope:&RpcConnectionScope,policy:&RouterPolicy,index:usize,io:I)->Result<Self,ConnectError> {
        let target=policy.targets.get(index).ok_or(ConnectError::Scope)?;
        if !scope.is_open() {return Err(ConnectError::Scope);}
        Ok(Self{owner:owner.clone(),scope:scope.clone(),policy:policy.identity.clone(),target:target.identity.clone(),index,io:Some(Box::new(io))})
    }
    pub(super) fn into_original(mut self,owner:&AttemptOwner,scope:&RpcConnectionScope,policy:&RouterPolicy)->Result<(usize,BoxRuntimeIo),ConnectError> {
        if &self.owner!=owner || !self.scope.matches(scope) || !scope.is_open() || !Arc::ptr_eq(&self.policy,&policy.identity)
            || !policy.targets.get(self.index).is_some_and(|t|Arc::ptr_eq(&self.target,&t.identity)) {return Err(ConnectError::Scope);}
        let io=self.io.take().ok_or(ConnectError::Scope)?;Ok((self.index,io))
    }
}
impl Drop for ConnectedRouterIo {fn drop(&mut self) {if self.io.is_some() {self.scope.close();}}}

pub(super) async fn poll_original<F:Future>(scope:&RpcConnectionScope,future:F)->Result<F::Output,ConnectError> {
    tokio::pin!(future);
    poll_fn(|cx|match scope.with_io_gate(||future.as_mut().poll(cx)) {Some(poll)=>poll.map(Ok),None=>Poll::Ready(Err(ConnectError::Scope))}).await
}
pub(super) async fn poll_before<F:Future>(scope:&RpcConnectionScope,future:F,deadline:std::time::Instant)->Result<F::Output,ConnectError> {
    tokio::pin!(future);
    poll_original(scope,poll_fn(|cx| {
        if std::time::Instant::now()>=deadline {Poll::Ready(Err(ConnectError::Timeout))}
        else {future.as_mut().poll(cx).map(Ok)}
    })).await?
}
pub struct RouterTcpConnector {timeout:Duration}
impl fmt::Debug for RouterTcpConnector {fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result {f.write_str("RouterTcpConnector(<redacted>)")} }
impl RouterTcpConnector {
    pub(super) fn new(timeout:Duration)->Result<Self,ConnectError> {
        if timeout.is_zero() || timeout>Duration::from_secs(10) {return Err(ConnectError::InvalidConfiguration);}Ok(Self{timeout})
    }
    pub(super) async fn connect(&self,owner:&AttemptOwner,scope:&RpcConnectionScope,policy:&RouterPolicy)->Result<ConnectedRouterIo,ConnectError> {
        if !scope.is_open() {return Err(ConnectError::Scope);}
        let deadline=std::time::Instant::now().checked_add(self.timeout).ok_or(ConnectError::InvalidConfiguration)?;
        let group=async {
            let mut attempted=Vec::<(i32,SocketAddr)>::new(); let mut resolved=0usize;
            for (index,target) in policy.targets.iter().enumerate() {
                let lookup=prepare_socket_addrs(&target.host,target.port).map_err(|_|ConnectError::Io)?;
                let addresses=poll_before(scope,lookup.resolve(),deadline).await?.map_err(|_|ConnectError::Io)?;
                for address in addresses {
                    resolved+=1;if resolved>64 {return Err(ConnectError::ResourceLimit);}
                    // Do not merge equal OS targets under different original ri.
                    if attempted.contains(&(target.router_id,address)) {continue;}
                    if attempted.len()==16 {return Err(ConnectError::ResourceLimit);}attempted.push((target.router_id,address));
                    if let Ok(io)=poll_before(scope,TcpStream::connect(address),deadline).await? {
                        io.set_nodelay(true).map_err(|_|ConnectError::Io)?;
                        return ConnectedRouterIo::original(owner,scope,policy,index,io);
                    }
                }
            }
            Err(ConnectError::Io)
        };
        tokio::select! {biased; _=scope.closed()=>Err(ConnectError::Scope),
            result=tokio::time::timeout_at(deadline.into(),group)=>result.unwrap_or(Err(ConnectError::Timeout))}
    }
}

#[cfg(test)]
#[path="router_connector_tests.rs"]
mod tests;
