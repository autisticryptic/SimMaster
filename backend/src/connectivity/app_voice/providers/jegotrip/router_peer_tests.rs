use super::*;
use super::super::object_reference::{parse_object_reference, ObjectReference, ReferenceTarget};

const LOCAL:(u32,u32)=(12001,0x11223344);
fn owner()->AttemptOwner{
 let mut session=super::super::session::HostingSession::new("fixture-peer-line".into());
 session.import(super::super::tests::imported(),100).unwrap();session.begin_login(100).unwrap()
}
fn reference(host:&str)->ObjectReference{parse_object_reference(format!("AccountEntry:sarc -h {host} -p 98;").as_bytes()).unwrap()}
fn endpoint(value:&ObjectReference)->&ReferenceEndpoint{let ReferenceTarget::Direct(list)=value.target() else{panic!("direct reference")}; &list[0]}
fn result_vector(dynamic:bool)->Vec<u8>{
 let mut bytes=vec![0,1];if dynamic{bytes.extend_from_slice(&[0,0,0x2e,0xe1]);}
 bytes.extend_from_slice(&[0x11,0x22,0x33,0x44, 0,1,0,2,0,3,0,4, 0,5,0,6,0,7,0,8,
  0,0,0,3,b'a',0,b'b', 0,0,0,0]);bytes
}

#[test]
fn connect_two_and_four_arguments_match_independent_native_word_order(){
 let two=encode_peer_arguments(12001,LOCAL,0x55667788,(42,0x01020304)).unwrap();
 assert_eq!(two.method,b"clientConnect2.RouterClient.Common");
 assert_eq!(two.bytes.expose_to_transport(),[0,0,1,0,0,0x11,0x22,0x33,0x44,0x55,0x66,0x77,0x88,0,0,0x2e,0xe1,0,0,0,42,1,2,3,4]);
 let four=encode_peer_arguments(-1,LOCAL,0x55667788,(42,0x01020304)).unwrap();
 assert_eq!(four.method,b"clientConnect4.RouterClient.Common");
 assert_eq!(four.bytes.expose_to_transport(),[0,0,1,0,0,0x11,0x22,0x33,0x44,0x55,0x66,0x77,0x88,0,0,0,42,1,2,3,4]);
}

#[test]
fn network_only_peer_keeps_zero_remote_node_without_changing_original_local_node(){
 let args=encode_peer_arguments(12001,LOCAL,9,(42,0)).unwrap();
 assert_eq!(&args.bytes.expose_to_transport()[17..],&[0,0,0,42,0,0,0,0]);
 assert_eq!(&args.bytes.expose_to_transport()[5..9],&LOCAL.1.to_be_bytes());
}

#[test]
fn peer_arguments_reject_invalid_ids_nonce_and_configured_to_actual_ri_substitution(){
 for (configured,local,nonce,peer) in [
  (0,LOCAL,1,(42,1)),(-1000,LOCAL,1,(42,1)),(12002,LOCAL,1,(42,1)),
  (12001,LOCAL,0,(42,1)),(-1,(0,1),1,(42,1)),(-1,(65536,1),1,(42,1)),
  (-1,(1,0),1,(42,1)),(-1,(1,u32::MAX),1,(42,1)),(12001,LOCAL,1,(0,1)),(12001,LOCAL,1,(42,u32::MAX)),
 ]{assert_eq!(encode_peer_arguments(configured,local,nonce,peer).err(),Some(RouterConnectError::Configuration));}
}

#[test]
fn peer_reply_reads_both_short_groups_binary_hint_and_mandatory_footer(){
 for dynamic in [false,true]{let reply=decode_peer_reply(&result_vector(dynamic),if dynamic{-1}else{12001},LOCAL).unwrap();
  assert_eq!(reply.network_words,[1,2,3,4]);assert_eq!(reply.node_words,[5,6,7,8]);
  assert_eq!(reply.alternate_hint.as_slice(),b"a\0b");assert!(reply.footer_properties.is_empty());
 }
}

#[test]
fn every_truncated_peer_result_is_rejected_before_any_route_binding(){
 for dynamic in [false,true]{let full=result_vector(dynamic);for end in 0..full.len(){
  assert_eq!(decode_peer_reply(&full[..end],if dynamic{-1}else{12001},LOCAL).err(),Some(RouterConnectError::Protocol));
 }}
}

#[test]
fn peer_reply_cannot_change_local_network_or_node_of_existing_scope(){
 let mut node=result_vector(false);node[5]^=1;
 assert_eq!(decode_peer_reply(&node,12001,LOCAL).err(),Some(RouterConnectError::Protocol));
 let mut network=result_vector(true);network[5]^=1;
 assert_eq!(decode_peer_reply(&network,-1,LOCAL).err(),Some(RouterConnectError::Protocol));
}

#[test]
fn peer_rejection_reason_and_trailing_bytes_do_not_become_a_route(){
 let mut rejected=result_vector(false);rejected[1]=0;
 assert_eq!(decode_peer_reply(&rejected,12001,LOCAL).err(),Some(RouterConnectError::Rejected));
 let mut reason=result_vector(false);let n=reason.len();reason[n-4..].copy_from_slice(&1u32.to_be_bytes());reason.push(b'x');
 assert_eq!(decode_peer_reply(&reason,12001,LOCAL).err(),Some(RouterConnectError::Rejected));
 let mut trailing=result_vector(false);trailing.push(1);
 assert_eq!(decode_peer_reply(&trailing,12001,LOCAL).err(),Some(RouterConnectError::Protocol));
}

#[test]
fn optional_peer_footer_properties_are_retained_privately_and_compare_exactly(){
 let mut bytes=result_vector(false);bytes.extend_from_slice(&[0,0,0,1,0,0,0,1,b'k',0,0,0,2,0,b'v']);
 let first=decode_peer_reply(&bytes,12001,LOCAL).unwrap();let second=decode_peer_reply(&bytes,12001,LOCAL).unwrap();
 assert_eq!(first,second);assert_eq!(first.footer_properties.len(),1);assert_eq!(first.footer_properties[0].1.as_slice(),b"\0v");
 let last=bytes.len()-1;bytes[last]=b'w';assert_ne!(first,decode_peer_reply(&bytes,12001,LOCAL).unwrap());
}

#[test]
fn peer_hint_is_bounded_and_ffff_cost_words_stay_explicitly_unavailable_values(){
 let mut bytes=result_vector(false);bytes[6..22].fill(0xff);
 let reply=decode_peer_reply(&bytes,12001,LOCAL).unwrap();assert_eq!(reply.network_words,[u16::MAX;4]);assert_eq!(reply.node_words,[u16::MAX;4]);
 let mut large=vec![0,1];large.extend_from_slice(&LOCAL.1.to_be_bytes());large.extend_from_slice(&[0;16]);
 large.extend_from_slice(&4097u32.to_be_bytes());large.extend_from_slice(&vec![b'x';4097]);large.extend_from_slice(&[0;4]);
 assert_eq!(decode_peer_reply(&large,12001,LOCAL).err(),Some(RouterConnectError::Protocol));
}

#[test]
fn numeric_peer_target_captures_original_endpoint_owner_scope_and_clock(){
 let original=owner();let scope=RpcConnectionScope::fresh();let clock=PortClock::for_original(original.clone(),scope.clone());let r=reference("42.7");
 let target=PeerTarget::capture(&original,&scope,&clock,endpoint(&r),None,0).unwrap();
 assert_eq!(target.ids_for(&original,&scope,&clock),Some((42,7,98)));assert!(target.matches_endpoint(endpoint(&r)));
 let other_clock=PortClock::for_original(original.clone(),scope.clone());assert!(target.ids_for(&original,&scope,&other_clock).is_none());
 assert!(target.ids_for(&owner(),&scope,&clock).is_none());assert!(target.ids_for(&original,&RpcConnectionScope::fresh(),&clock).is_none());
 scope.close();assert!(target.ids_for(&original,&scope,&clock).is_none());
}

#[test]
fn equal_looking_new_peer_target_is_not_the_pending_targets_identity(){
 let original=owner();let scope=RpcConnectionScope::fresh();let clock=PortClock::for_original(original.clone(),scope.clone());let r=reference("42.7");
 let target=PeerTarget::capture(&original,&scope,&clock,endpoint(&r),None,0).unwrap();let cloned=target.clone();
 let replacement=PeerTarget::capture(&original,&scope,&clock,endpoint(&r),None,0).unwrap();
 assert!(target.same_selection(&cloned));assert!(!target.same_selection(&replacement));
 assert!(!target.matches_endpoint(endpoint(&reference("42.8"))));
}

#[test]
fn unresolved_query_invalid_index_or_foreign_clock_cannot_manufacture_a_peer_target(){
 let original=owner();let scope=RpcConnectionScope::fresh();let clock=PortClock::for_original(original.clone(),scope.clone());
 assert!(PeerTarget::capture(&original,&scope,&clock,endpoint(&reference("arc@AccountEntry")),None,0).is_err());
 assert!(PeerTarget::capture(&original,&scope,&clock,endpoint(&reference("42.7")),None,1).is_err());
 let wrong=PortClock::for_original(owner(),scope.clone());assert!(PeerTarget::capture(&original,&scope,&wrong,endpoint(&reference("42.7")),None,0).is_err());
}

#[test]
fn peer_codec_debug_redacts_hints_words_and_endpoint_selection(){
 let reply=decode_peer_reply(&result_vector(false),12001,LOCAL).unwrap();let args=encode_peer_arguments(12001,LOCAL,9,(42,0)).unwrap();
 let text=format!("{reply:?} {args:?}");assert!(text.contains("<redacted>"));assert!(!text.contains("11223344"));assert!(!text.contains("a\\0b"));
}

fn local_address()->super::super::router_ports::LogicalAddress{super::super::router_ports::LogicalAddress{network:12001,node:0x11223344,port:12000}}

#[test]
fn relay_network_only_route_matches_native_implicit_source_and_mixed_width_destination(){
 let reply=decode_peer_reply(&result_vector(false),12001,LOCAL).unwrap();
 let route=relay_routing(local_address(),(42,0,98),&reply,None,0xaabbccdd,0).unwrap();
 assert_eq!((route.source.mask,route.destination.mask),(0,1));
 let wire=super::super::native_routing::encode_routing_body(&route,&[0x99]).unwrap();
 assert_eq!(wire.expose_to_transport(),[0xaa,0xbb,0xcc,0xdd,0,0x20,0,0,0x2e,0xe0,0,42,0,1,0,98,0x99]);
}

#[test]
fn relay_node_route_preserves_full_node_and_its_own_auxiliary_word(){
 let reply=decode_peer_reply(&result_vector(false),12001,LOCAL).unwrap();
 let route=relay_routing(local_address(),(42,7,98),&reply,None,0,0).unwrap();
 assert_eq!(route.destination.mask,3);assert_eq!((route.destination.network_tag,route.destination.node,route.destination.node_tag),(1,7,5));
 let same_network=relay_routing(local_address(),(12001,7,98),&reply,None,0,0).unwrap();
 assert_eq!(same_network.destination.mask,2);assert_eq!(same_network.source.port,12000);
}

#[test]
fn reported_forward_hop_keeps_original_target_and_never_uses_alternate_hint_as_authority(){
 let reply=decode_peer_reply(&result_vector(false),12001,LOCAL).unwrap();
 let mut report=super::super::router_control::RouterReport{levels:[11;4],..Default::default()};
 report.networks.insert(5,[2;4]);report.peers.insert(super::super::router_control::PeerWords([5,9,42,7]),[1;4]);
 let route=relay_routing(local_address(),(42,7,98),&reply,Some(&report),0,0).unwrap();
 assert_eq!((route.destination.mask,route.destination.network,route.destination.node),(7,5,9));
 assert_eq!((route.destination.forward_network,route.destination.forward_node,route.destination.port),(42,7,98));
 assert_eq!((route.destination.network_tag,route.destination.node_tag,route.destination.label),(2,1,11));assert!(route.source_ip.is_none());
}

#[test]
fn unavailable_or_unrelated_route_reports_cannot_select_an_unconfirmed_peer(){
 let reply=decode_peer_reply(&result_vector(false),12001,LOCAL).unwrap();let mut report=super::super::router_control::RouterReport::default();
 report.networks.insert(42,[u16::MAX;4]);report.networks.insert(5,[1;4]);
 report.peers.insert(super::super::router_control::PeerWords([5,9,43,7]),[1;4]);
 assert_eq!(relay_routing(local_address(),(42,7,98),&reply,Some(&report),0,0).err(),Some(RouterConnectError::Pending));
 report.peers.insert(super::super::router_control::PeerWords([65536,9,42,7]),[1;4]);report.networks.insert(65536,[1;4]);
 assert_eq!(relay_routing(local_address(),(42,7,98),&reply,Some(&report),0,0).err(),Some(RouterConnectError::Pending));
 assert_eq!(relay_routing(local_address(),(42,7,98),&reply,None,0,4).err(),Some(RouterConnectError::Configuration));
}

#[test]
fn all_ffff_node_costs_are_unused_for_network_service_but_never_a_valid_node_route(){
 let mut bytes=result_vector(false);bytes[14..22].fill(0xff);let reply=decode_peer_reply(&bytes,12001,LOCAL).unwrap();
 assert!(relay_routing(local_address(),(42,0,98),&reply,None,0,0).is_ok());
 assert_eq!(relay_routing(local_address(),(42,7,98),&reply,None,0,0).err(),Some(RouterConnectError::Pending));
}

#[test]
fn peer_binding_requires_original_target_identity_clock_and_live_scope(){
 let original=owner();let scope=RpcConnectionScope::fresh();let clock=PortClock::for_original(original.clone(),scope.clone());let r=reference("42.7");
 let target=PeerTarget::capture(&original,&scope,&clock,endpoint(&r),None,0).unwrap();
 let other=PeerTarget::capture(&original,&scope,&clock,endpoint(&r),None,0).unwrap();
 let binding=PeerBinding::from_matched(2,target.clone(),decode_peer_reply(&result_vector(false),12001,LOCAL).unwrap());
 assert_eq!(binding.call_id(),2);assert!(binding.reply_for(&original,&scope,&clock,&target).is_some());
 assert!(binding.reply_for(&original,&scope,&clock,&other).is_none());scope.close();assert!(binding.reply_for(&original,&scope,&clock,&target).is_none());
}
