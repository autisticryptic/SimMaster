use super::*;
use super::super::{arc_session::ArcPhase, arc_protocol::ArcProtocol, native_router::{RouterAssignment,RouterConnectTracker},
 native_routing::{RouteAddress,encode_routing_body}, object_reference::{ObjectReference,ReferenceTarget,parse_object_reference},
 packet_stream::encode_packet_stream_frame, packet_transport::RouterTiming, router_ports::{ListenProtocol,RouterPorts},
 rpc_envelope::encode_reply, rpc_stream::Writer, tls_engine::{EndpointTlsEngine,VerifiedEndpointPackets},
 tls_engine::tests::{Certificates,endpoint_setup,server_output}};
use std::io::Write;
use tokio_rustls::rustls::ServerConnection;

const LOCAL_NET:u16=12001;const LOCAL_NODE:u32=0x55667788;const PORT:u16=12000;
fn endpoint(reference:&ObjectReference)->&ReferenceEndpoint{let ReferenceTarget::Direct(endpoints)=reference.target()else{panic!("direct fixture")};&endpoints[0]}
fn timing()->RouterTiming{RouterTiming{word1:1,word2:0,byte6:0}}
fn packet(sequence:u16,kind:u8,part:u8,body:&[u8])->Vec<u8>{
 let mut out=sequence.to_be_bytes().to_vec();out.extend_from_slice(&[0,1,0,0,0,kind|(part<<4)]);out.extend_from_slice(body);out
}
fn control(sequence:u16,id:i32,result:&[u8])->Vec<u8>{
 let reply=encode_reply(id,0,result).unwrap();let mut body=vec![0;12];body.push(1);body.extend_from_slice(reply.expose_to_transport());packet(sequence,1,1,&body)
}
fn channel(sequence:u16,kind:super::super::native_channel::PacketKind,gap:u8,ack:u16,delivered:u16,body:&[u8])->Vec<u8>{
 use super::super::native_channel::{encode_packet,HeaderMode,ChannelPacket};
 encode_packet(HeaderMode::Basic,&ChannelPacket{timestamp:None,sequence,kind,data_gap:gap,ack_sequence:ack,delivered_sequence:delivered,
  inclusive_ack:false,ack_mask:&[],body}).unwrap().expose_to_transport().to_vec()
}
struct Fixture{
 protocol:ArcProtocol,owner:AttemptOwner,scope:RpcConnectionScope,engine:EndpointTlsEngine,server:ServerConnection,
 tracker:RouterConnectTracker,ports:RouterPorts,assignment:RouterAssignment,lease:PortLease,peer:Option<PeerBinding>,
 reference:ObjectReference,now:Instant,sequence:u16,
}
impl Fixture{
 fn tls_input(owner:&AttemptOwner,scope:&RpcConnectionScope,engine:&mut EndpointTlsEngine,server:&mut ServerConnection,
  packets:&[Vec<u8>],now:Instant)->VerifiedEndpointPackets{
  for packet in packets{server.writer().write_all(encode_packet_stream_frame(packet).unwrap().expose_to_transport()).unwrap();}
  engine.on_wire_bytes(owner,scope,&server_output(server),now).unwrap().unwrap().plaintext
 }
 fn new()->Self{
  let cert=Certificates::generate("DNS:fixture.invalid");let(protocol,owner,scope,mut engine,mut server,now)=endpoint_setup(&cert,&tokio_rustls::rustls::version::TLS13);
  let mut tracker=RouterConnectTracker::for_original(owner.clone(),scope.clone(),LOCAL_NET as i32,7,&[],1460,10,Duration::from_secs(5),now).unwrap();
  let ports=tracker.create_ports(&owner,&scope,10000,now).unwrap().unwrap();tracker.prepare(&owner,&scope,timing(),now).unwrap();
  let mut result=Writer::new();result.boolean(true).unwrap();result.u32(LOCAL_NODE).unwrap();result.bytes(b"").unwrap();
  let input=Self::tls_input(&owner,&scope,&mut engine,&mut server,&[control(40,1,result.finish().expose_to_transport())],now);
  let assignment=tracker.on_packets(&owner,&scope,input,now).unwrap().unwrap();
  let lease=ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(PORT),false,now+Duration::from_secs(4),now).unwrap().unwrap();
  assert!(lease.address_for(&owner,&scope).is_none());
  let node=encode_routing_body(&Routing{source:RouteAddress{mask:1,network:LOCAL_NET,..Default::default()},..Default::default()},&[1]).unwrap();
  let input=Self::tls_input(&owner,&scope,&mut engine,&mut server,&[packet(41,0,0,node.expose_to_transport())],now);
  tracker.on_packets(&owner,&scope,input,now).unwrap();
  for data in tracker.take_data(&owner,&scope).unwrap(){assert!(ports.on_data(&owner,&scope,&assignment,&data,now).unwrap().is_none());}
  assert!(lease.address_for(&owner,&scope).is_some());
  let reference=parse_object_reference(b"AccountEntry:sarc -h 77.88 -p 98;").unwrap();let target=tracker.capture_peer_target(&owner,&scope,endpoint(&reference),0,now).unwrap().unwrap();
  let request=tracker.prepare_peer(&owner,&scope,&target,timing(),now+Duration::from_secs(4),now).unwrap().unwrap();
  let mut result=Writer::new();result.boolean(true).unwrap();result.u32(LOCAL_NODE).unwrap();for _ in 0..8{result.u16(2).unwrap();}
  result.bytes(b"Router:tls -h not-authorized.invalid -p 1 -ri 9;").unwrap();result.bytes(b"").unwrap();
  let input=Self::tls_input(&owner,&scope,&mut engine,&mut server,&[control(42,request.call_id(),result.finish().expose_to_transport())],now);
  tracker.on_packets(&owner,&scope,input,now).unwrap();let peer=tracker.take_peer_binding(&owner,&scope,&target).unwrap();
  Self{protocol,owner,scope,engine,server,tracker,ports,assignment,lease,peer:Some(peer),reference,now,sequence:43}
 }
 fn session(&mut self)->ChannelSession{
  let peer=self.peer.take().unwrap();let clock=self.tracker.channel_clock(&self.owner,&self.scope,&self.lease,&peer).unwrap();
  ChannelSession::for_original(self.owner.clone(),self.scope.clone(),clock,&self.lease,peer,endpoint(&self.reference),0,0,1460,
   self.now+Duration::from_secs(4),Duration::from_secs(2),self.now).unwrap()
 }
 fn deliveries(&mut self,source:RouteAddress,packets:&[Vec<u8>])->Vec<PortDelivery>{
  let mut routed=Vec::new();for bytes in packets{
   let wire=escape_channel(bytes).unwrap();let route=Routing{source,destination:RouteAddress{port:PORT,..Default::default()},..Default::default()};
   let body=encode_routing_body(&route,wire.expose_to_transport()).unwrap();routed.push(packet(self.sequence,0,1,body.expose_to_transport()));self.sequence=self.sequence.wrapping_add(1);
  }
  let input=Self::tls_input(&self.owner,&self.scope,&mut self.engine,&mut self.server,&routed,self.now);
  self.tracker.on_packets(&self.owner,&self.scope,input,self.now).unwrap();let mut deliveries=Vec::new();
  for data in self.tracker.take_data(&self.owner,&self.scope).unwrap(){if let Some(delivery)=self.ports.on_data(&self.owner,&self.scope,&self.assignment,&data,self.now).unwrap(){deliveries.push(delivery);}}
  deliveries
 }
 fn start(&mut self,session:&mut ChannelSession)->PreparedChannel{
  let output=session.next_packet(&self.owner,&self.scope,self.now).unwrap().unwrap();
  assert_eq!(output.bytes_for(&self.owner,&self.scope,session).unwrap(),[0xff,0xff,0x40,0,0,0,0,0,0,0]);
  let deliveries=self.deliveries(peer_source(),&[channel(0x9000,super::super::native_channel::PacketKind::Start,0,0x4000,0x4000,&[])]);
  let records=session.on_deliveries(&self.owner,&self.scope,&deliveries,self.now).unwrap().unwrap();
  assert!(records.records_for(&self.owner,&self.scope,session).unwrap().is_empty());assert!(session.ready_for(&self.owner,&self.scope));
  // Drain the handshake ACK so later DATA has a known non-data gap.
  session.next_packet(&self.owner,&self.scope,self.now).unwrap().unwrap();output
 }
}
fn peer_source()->RouteAddress{RouteAddress{mask:3,network:77,node:88,port:98,..Default::default()}}
fn data(sequence:u16,body:&[u8])->Vec<u8>{channel(sequence,super::super::native_channel::PacketKind::Data,0,0,0,body)}

#[test]
fn actual_endpoint_tls_assignment_peer_and_start_still_do_not_authenticate_sdk(){
 let mut f=Fixture::new();let mut s=f.session();assert!(!s.ready_for(&f.owner,&f.scope));f.start(&mut s);
 assert_eq!(f.protocol.phase(),ArcPhase::Initial);assert!(f.scope.is_open());
}

#[test]
fn actual_tls_records_decrypt_only_after_channel_order_and_keep_directions_independent(){
 let mut f=Fixture::new();let mut s=f.session();f.start(&mut s);
 let mut plain=vec![9];plain.extend_from_slice(&[0;16]);s.queue_record(&f.owner,&f.scope,&plain,f.now+Duration::from_secs(1),f.now).unwrap();
 let output=s.next_packet(&f.owner,&f.scope,f.now).unwrap().unwrap();
 use super::super::native_channel::{decode_packet,previous_data,HeaderMode,Records};
 let p=decode_packet(HeaderMode::Basic,unescape_channel(output.bytes_for(&f.owner,&f.scope,&s).unwrap()).unwrap()).unwrap();
 let encoded=Records::default().on_payload(previous_data(&p).unwrap().1).unwrap();
 assert_eq!(encoded[0].expose_to_transport(),[9,0,0,0,0,0x32,0x49,0xa2,0x34,0,0,0,0,0,0,0,0,0x10,0x92,0x34,0x87]);
 let input=f.deliveries(peer_source(),&[data(0x9001,&[0xff,7,0,0,0,1,0x53,0x2b,0xc1])]);
 let records=s.on_deliveries(&f.owner,&f.scope,&input,f.now).unwrap().unwrap();assert_eq!(records.records_for(&f.owner,&f.scope,&s).unwrap()[0].expose_to_transport(),[7,b'a',b'b',b'c']);
 let input=f.deliveries(peer_source(),&[data(0x9002,&[0xff,8,0x35])]);
 assert_eq!(s.on_deliveries(&f.owner,&f.scope,&input,f.now).unwrap().unwrap().records_for(&f.owner,&f.scope,&s).unwrap()[0].expose_to_transport(),[8,0]);
 assert_eq!(f.protocol.phase(),ArcPhase::Initial);
}

#[test]
fn pre_claim_tls_delivery_cannot_initialise_a_later_session_even_at_same_instant(){
 let mut f=Fixture::new();let before=f.deliveries(peer_source(),&[channel(0x9000,super::super::native_channel::PacketKind::Start,0,0x4000,0x4000,&[])]);
 let mut s=f.session();s.next_packet(&f.owner,&f.scope,f.now).unwrap();
 assert!(s.on_deliveries(&f.owner,&f.scope,&before,f.now-Duration::from_secs(1)).unwrap().is_none());assert!(!s.ready_for(&f.owner,&f.scope));
 assert!(s.tick(&f.owner,&f.scope,f.now).unwrap());
}

#[test]
fn foreign_source_node_network_or_port_is_ignored_before_clock_and_record_parser(){
 let mut f=Fixture::new();let mut s=f.session();let malformed=vec![0xff];
 for source in [RouteAddress{node:89,..peer_source()},RouteAddress{network:78,..peer_source()},RouteAddress{port:99,..peer_source()}]{
  let input=f.deliveries(source,&[malformed.clone()]);
  assert!(s.on_deliveries(&f.owner,&f.scope,&input,f.now-Duration::from_secs(1)).unwrap().is_none());assert!(f.scope.is_open());
 }
 assert!(s.tick(&f.owner,&f.scope,f.now).unwrap());
}

#[test]
fn forwarded_original_source_matches_target_without_replacing_tls_authority(){
 let mut f=Fixture::new();let mut s=f.session();s.next_packet(&f.owner,&f.scope,f.now).unwrap();
 let source=RouteAddress{mask:7,network:10,node:11,forward_network:77,forward_node:88,port:98,..Default::default()};
 let input=f.deliveries(source,&[channel(0x9000,super::super::native_channel::PacketKind::Start,0,0x4000,0x4000,&[])]);
 s.on_deliveries(&f.owner,&f.scope,&input,f.now).unwrap();assert!(s.ready_for(&f.owner,&f.scope));assert_eq!(s.remote,LogicalAddress{network:77,node:88,port:98});
}

#[test]
fn later_bad_channel_in_original_tls_group_suppresses_all_plain_records_and_revokes_queued_output(){
 let mut f=Fixture::new();let mut s=f.session();let old=f.start(&mut s);
 let input=f.deliveries(peer_source(),&[data(0x9001,&[0xff,7,0,0,0,1,0x53,0x2b,0xc1]),data(0x9002,&[0])]);
 assert!(s.on_deliveries(&f.owner,&f.scope,&input,f.now).is_err());assert!(!f.scope.is_open());assert!(old.bytes_for(&f.owner,&f.scope,&s).is_none());
}

#[test]
fn malformed_first_security_record_closes_without_resetting_seed_on_next_record(){
 let mut f=Fixture::new();let mut s=f.session();f.start(&mut s);let input=f.deliveries(peer_source(),&[data(0x9001,&[0xff,7,0,0,0])]);
 assert_eq!(s.on_deliveries(&f.owner,&f.scope,&input,f.now).err(),Some(SessionError::Record(TransportError::Truncated)));assert!(!f.scope.is_open());
}

#[test]
fn cancellation_revokes_prepared_and_previously_completed_groups_and_releases_original_port(){
 let mut f=Fixture::new();let mut s=f.session();let old=f.start(&mut s);
 let input=f.deliveries(peer_source(),&[data(0x9001,&[0xff,7,0,0,0,1,0x53,0x2b,0xc1])]);
 let records=s.on_deliveries(&f.owner,&f.scope,&input,f.now).unwrap().unwrap();assert!(records.records_for(&f.owner,&f.scope,&s).is_some());
 s.close();assert!(old.bytes_for(&f.owner,&f.scope,&s).is_none());assert!(records.records_for(&f.owner,&f.scope,&s).is_none());assert!(f.lease.address_for(&f.owner,&f.scope).is_none());
}

#[test]
fn wrong_owner_scope_or_other_session_cannot_read_an_original_capability_brand(){
 let mut f=Fixture::new();let mut s=f.session();let output=f.start(&mut s);let mut other=Fixture::new();let other_session=other.session();
 assert!(output.bytes_for(&other.owner,&f.scope,&s).is_none());assert!(output.bytes_for(&f.owner,&other.scope,&s).is_none());
 assert!(output.bytes_for(&f.owner,&f.scope,&other_session).is_none());
 assert!(!s.queue_record(&other.owner,&f.scope,b"invalid",f.now,f.now-Duration::from_secs(1)).unwrap());
 assert!(f.scope.is_open());assert!(s.tick(&f.owner,&f.scope,f.now).unwrap());
}

#[test]
fn released_logical_port_revokes_channel_capabilities_before_physical_owner_drop(){
 let mut f=Fixture::new();let mut s=f.session();let output=f.start(&mut s);
 assert!(f.lease.release(&f.owner,&f.scope));assert!(f.scope.is_open());assert!(output.bytes_for(&f.owner,&f.scope,&s).is_none());
 assert!(!s.ready_for(&f.owner,&f.scope));drop(s);assert!(!f.scope.is_open());
 // The fixture owns no physical socket; physical Drop/Join needs runtime tests.
}

#[test]
fn original_peer_clock_and_endpoint_are_required_before_channel_claim(){
 let mut f=Fixture::new();let peer=f.peer.take().unwrap();let clock=PortClock::for_original(f.owner.clone(),f.scope.clone());
 assert!(ChannelSession::for_original(f.owner.clone(),f.scope.clone(),clock,&f.lease,peer,endpoint(&f.reference),0,0,1460,
  f.now+Duration::from_secs(1),Duration::from_secs(1),f.now).is_err());assert!(f.scope.is_open());assert!(f.lease.address_for(&f.owner,&f.scope).is_some());
}

#[test]
fn session_and_capability_debug_redact_peer_route_cipher_and_payload(){
 let mut f=Fixture::new();let mut s=f.session();let output=f.start(&mut s);let text=format!("{s:?} {output:?}");
 for value in ["not-authorized","55667788","12000","AccountEntry"]{assert!(!text.contains(value));}assert!(text.contains("<redacted>"));
}

#[test]
fn prepared_io_carries_original_hard_deadline_not_a_new_caller_or_soft_retry_time(){
 let mut f=Fixture::new();let mut s=f.session();f.start(&mut s);let end=f.now+Duration::from_secs(1);
 s.queue_record(&f.owner,&f.scope,&[1,b'x'],end,f.now).unwrap();let output=s.next_packet(&f.owner,&f.scope,f.now).unwrap().unwrap();
 let (_,io_end)=output.for_io(&f.owner,&f.scope,&s,f.now,f.now+Duration::from_secs(20)).unwrap().unwrap();assert_eq!(io_end,end);
 assert_eq!(s.bound_io_deadline(&f.owner,&f.scope,f.now+Duration::from_secs(20)),Some(end));
 assert_eq!(output.for_io(&f.owner,&f.scope,&s,end,f.now+Duration::from_secs(20)).err(),Some(SessionError::Channel(ChannelError::Timeout)));
 assert!(output.for_io(&f.owner,&RpcConnectionScope::fresh(),&s,end,f.now+Duration::from_secs(20)).unwrap().is_none());
}

#[test]
fn scoped_record_group_retains_original_receive_time_after_processing_delay(){
 let mut f=Fixture::new();let mut s=f.session();f.start(&mut s);
 let input=f.deliveries(peer_source(),&[data(0x9001,&[0xff,7,0,0,0,1,0x53,0x2b,0xc1])]);
 let records=s.on_deliveries(&f.owner,&f.scope,&input,f.now+Duration::from_secs(1)).unwrap().unwrap();
 assert_eq!(records.received_for(&f.owner,&f.scope,&s),Some(f.now));assert!(records.received_for(&f.owner,&RpcConnectionScope::fresh(),&s).is_none());
 s.close();assert!(records.received_for(&f.owner,&f.scope,&s).is_none());
}
