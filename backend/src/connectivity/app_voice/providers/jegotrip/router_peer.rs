//! Original endpoint/peer negotiation for the Router relay, not SDK login.
//! D496A8 -> E408B4/E40CF0 requests clientConnect2/4. E23D54/E240BC return
//! local IDs, two groups of four shorts, an optional alternate-Router hint and
//! the mandatory Common footer. A hint never authorizes an OS/TLS endpoint.
use super::{native_router::{RouterConnectError, RouterResolution}, object_reference::{ReferenceEndpoint, RouterHost},
    router_ports::{PortClock, PortOrder}, rpc_pending::RpcConnectionScope,
    rpc_stream::{Reader, Writer, RpcBytes, read_footer}, session::AttemptOwner};
use std::{fmt, sync::Arc};
use zeroize::{Zeroize, Zeroizing};

struct TargetInner {
    owner: AttemptOwner, scope: RpcConnectionScope, order: PortOrder,
    scheme: Vec<u8>, host: Vec<u8>, network: u16, node: u32, port: u16,
}
impl Drop for TargetInner { fn drop(&mut self) { self.scheme.zeroize(); self.host.zeroize(); } }
/// Clones share the original immutable selection; a new equal-looking target
/// is NOT interchangeable with a pending request's captured target identity.
#[derive(Clone)]
pub(super) struct PeerTarget(Arc<TargetInner>);
impl fmt::Debug for PeerTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PeerTarget(<redacted>)") }
}
impl PeerTarget {
    pub fn capture(owner: &AttemptOwner, scope: &RpcConnectionScope, clock: &PortClock,
        endpoint: &ReferenceEndpoint, resolution: Option<&RouterResolution>, index: usize) -> Result<Self, RouterConnectError> {
        if !clock.origin_matches(owner, scope) { return Err(RouterConnectError::Configuration); }
        let selected = match endpoint.router_host().map_err(|_| RouterConnectError::Configuration)? {
            RouterHost::Query(_) => *resolution.and_then(|value| value.hosts_for(owner, scope, endpoint))
                .and_then(|hosts| hosts.get(index)).ok_or(RouterConnectError::Configuration)?,
            RouterHost::Address(address) if resolution.is_none() && index == 0 => address,
            _ => return Err(RouterConnectError::Configuration),
        };
        let order = clock.stamp().map_err(|_| RouterConnectError::Configuration)?;
        Ok(Self(Arc::new(TargetInner { owner: owner.clone(), scope: scope.clone(), order,
            scheme: endpoint.scheme().to_vec(), host: endpoint.host().to_vec(), network: selected.network(), node: selected.node(), port: endpoint.port() })))
    }
    pub fn ids_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, clock: &PortClock) -> Option<(u16, u32, u16)> {
        if &self.0.owner != owner || !self.0.scope.matches(scope) || !self.0.scope.is_open() || !clock.owns(&self.0.order) { return None; }
        Some((self.0.network, self.0.node, self.0.port))
    }
    pub fn matches_endpoint(&self, endpoint: &ReferenceEndpoint) -> bool {
        self.0.scheme.as_slice() == endpoint.scheme() && self.0.host.as_slice() == endpoint.host() && self.0.port == endpoint.port()
            && endpoint.router_host().is_ok()
    }
    pub fn same_selection(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
}

pub(super) struct PeerArguments { pub method: &'static [u8], pub bytes: RpcBytes }
impl fmt::Debug for PeerArguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PeerArguments(<redacted>)") }
}
/// Only caller-authenticated original IDs enter here. This pure codec does not
/// allocate a call ID, select a different physical Router, or grant readiness.
pub(super) fn encode_peer_arguments(configured_ri: i32, local: (u32, u32), nonce: u32,
    peer: (u16, u32)) -> Result<PeerArguments, RouterConnectError> {
    if configured_ri == 0 || configured_ri < -999 || nonce == 0 || local.0 == 0 || local.0 > u16::MAX as u32
        || local.1 == 0 || local.1 == u32::MAX || peer.0 == 0 || peer.1 == u32::MAX
        || configured_ri > 0 && configured_ri as u32 != local.0 { return Err(RouterConnectError::Configuration); }
    let mut writer = Writer::versioned().map_err(|_| RouterConnectError::Configuration)?;
    writer.u32(local.1).map_err(|_| RouterConnectError::Configuration)?;
    writer.u32(nonce).map_err(|_| RouterConnectError::Configuration)?;
    if configured_ri > 0 { writer.u32(local.0).map_err(|_| RouterConnectError::Configuration)?; }
    writer.u32(peer.0 as u32).map_err(|_| RouterConnectError::Configuration)?;
    writer.u32(peer.1).map_err(|_| RouterConnectError::Configuration)?;
    Ok(PeerArguments { method: if configured_ri > 0 { b"clientConnect2.RouterClient.Common" } else { b"clientConnect4.RouterClient.Common" }, bytes: writer.finish() })
}

pub(super) struct PeerReply {
    pub network_words: [u16; 4], pub node_words: [u16; 4],
    pub alternate_hint: Zeroizing<Vec<u8>>,
    pub footer_properties: Vec<(Zeroizing<Vec<u8>>, Zeroizing<Vec<u8>>)>,
}
impl PartialEq for PeerReply {
    fn eq(&self, other: &Self) -> bool {
        self.network_words == other.network_words && self.node_words == other.node_words
            && self.alternate_hint.as_slice() == other.alternate_hint.as_slice()
            && self.footer_properties.len() == other.footer_properties.len()
            && self.footer_properties.iter().zip(&other.footer_properties).all(|((ak, av), (bk, bv))|
                ak.as_slice() == bk.as_slice() && av.as_slice() == bv.as_slice())
    }
}
impl Eq for PeerReply {}
impl fmt::Debug for PeerReply {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PeerReply(<redacted>)") }
}
pub(super) fn decode_peer_reply(bytes: &[u8], configured_ri: i32, expected_local: (u32, u32)) -> Result<PeerReply, RouterConnectError> {
    if configured_ri == 0 || configured_ri < -999 || expected_local.0 == 0 || expected_local.0 > u16::MAX as u32
        || expected_local.1 == 0 || expected_local.1 == u32::MAX
        || configured_ri > 0 && configured_ri as u32 != expected_local.0 { return Err(RouterConnectError::Configuration); }
    let parse = || -> Result<(bool, u32, u32, PeerReply, bool), super::rpc_stream::CodecError> {
        let mut reader = Reader::new(bytes)?;
        let accepted = reader.boolean()?;
        let network = if configured_ri < 0 { reader.u32()? } else { expected_local.0 };
        let node = reader.u32()?;
        let network_words = [reader.u16()?, reader.u16()?, reader.u16()?, reader.u16()?];
        let node_words = [reader.u16()?, reader.u16()?, reader.u16()?, reader.u16()?];
        let hint = reader.bytes()?;
        if hint.len() > 4096 { return Err(super::rpc_stream::CodecError::TooLarge); }
        let footer = read_footer(&mut reader)?; reader.finish()?;
        let footer_ok = footer.reason.is_empty();
        let reply = PeerReply { network_words, node_words, alternate_hint: Zeroizing::new(hint.to_vec()),
            footer_properties: footer.properties.iter().map(|p| (Zeroizing::new(p.key.to_vec()), Zeroizing::new(p.value.to_vec()))).collect() };
        Ok((accepted, network, node, reply, footer_ok))
    };
    let (accepted, network, node, reply, footer_ok) = parse().map_err(|_| RouterConnectError::Protocol)?;
    if !accepted || !footer_ok { return Err(RouterConnectError::Rejected); }
    // D50F5C can update native local IDs, but replacing an original live ID must
    // retire this project's scope/listens, never retarget their pending IO.
    if (network, node) != expected_local { return Err(RouterConnectError::Protocol); }
    Ok(reply)
}

/// Choose only relay records learned on the current original Router. A
/// different logical hop retains the requested peer in the forwarding fields;
/// the optional alternate-Router string is deliberately not an input here.
pub(super) fn relay_routing(local: super::router_ports::LogicalAddress, peer: (u16, u32, u16),
    reply: &PeerReply, report: Option<&super::router_control::RouterReport>, tag: u32, priority: u8)
    -> Result<super::native_routing::Routing, RouterConnectError> {
    use super::native_routing::{RouteAddress, Routing};
    if priority > 3 || local.network == 0 || local.node == 0 || local.node == u32::MAX || local.port == 0
        || peer.0 == 0 || peer.1 == u32::MAX || peer.2 == 0 { return Err(RouterConnectError::Configuration); }
    let level = priority as usize;
    let network = report.and_then(|r| r.networks.get(&(peer.0 as u32))).unwrap_or(&reply.network_words)[level];
    let node = report.and_then(|r| r.peers.get(&super::router_control::PeerWords([peer.0 as u32, peer.1, 0, 0])))
        .unwrap_or(&reply.node_words)[level];
    let mut selected = (network != u16::MAX && (peer.1 == 0 || node != u16::MAX))
        .then_some((peer.0, peer.1, network, node, u32::from(network) + if peer.1 == 0 {0} else {u32::from(node)}));
    if peer.1 != 0 {
        if let Some(report) = report {
            for (key, node_words) in &report.peers {
                let [via_network, via_node, target_network, target_node] = key.0;
                if (target_network, target_node) != (peer.0 as u32, peer.1) || via_network == 0
                    || via_network > u16::MAX as u32 || via_node == 0 || via_node == u32::MAX { continue; }
                let Some(network_words) = report.networks.get(&via_network) else { continue; };
                let (net_word, node_word) = (network_words[level], node_words[level]);
                if net_word == u16::MAX || node_word == u16::MAX { continue; }
                let cost = u32::from(net_word) + u32::from(node_word);
                if selected.is_none_or(|old| cost < old.4) {
                    selected = Some((via_network as u16, via_node, net_word, node_word, cost));
                }
            }
        }
    }
    let (network, node, network_tag, node_tag, _) = selected.ok_or(RouterConnectError::Pending)?;
    let forwarded = (network, node) != (peer.0, peer.1);
    let destination = RouteAddress { mask: u8::from(network != local.network) | (u8::from(node != 0) << 1) | (u8::from(forwarded) << 2),
        label: report.map_or(0, |r| r.levels[level]), network, network_tag, node, node_tag,
        forward_network: if forwarded {peer.0} else {0}, forward_node: if forwarded {peer.1} else {0}, port: peer.2 };
    Ok(Routing { tag, priority, source: RouteAddress { network: local.network, node: local.node, port: local.port, ..Default::default() },
        destination, ..Default::default() })
}

/// Published only after one whole matched peer-result group. The peer object
/// and its original endpoint selection remain immutable across later calls.
pub(super) struct PeerBinding { call_id: i32, target: PeerTarget, reply: PeerReply }
impl PeerBinding {
    pub fn call_id(&self) -> i32 { self.call_id }
    pub fn from_matched(call_id: i32, target: PeerTarget, reply: PeerReply) -> Self { Self { call_id, target, reply } }
    pub fn reply_for(&self, owner: &AttemptOwner, scope: &RpcConnectionScope, clock: &PortClock, target: &PeerTarget) -> Option<&PeerReply> {
        if !self.target.same_selection(target) { return None; }
        self.target.ids_for(owner, scope, clock)?; Some(&self.reply)
    }
    pub fn target(&self) -> &PeerTarget { &self.target }
}
impl fmt::Debug for PeerBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("PeerBinding(<redacted>)") }
}

#[cfg(test)]
#[path = "router_peer_tests.rs"]
mod tests;
