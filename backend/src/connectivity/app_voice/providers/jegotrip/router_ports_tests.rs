use super::*;
use super::super::native_routing::RouteAddress;

const IDS: (u16, u32) = (12001, 0x55667788);
fn fixture_owner() -> AttemptOwner {
    let mut session = super::super::session::HostingSession::new("fixture-router-port-line".into());
    session.import(super::super::tests::imported(), 100).unwrap(); session.begin_login(100).unwrap()
}
fn setup(candidate: u16) -> (AttemptOwner, RpcConnectionScope, RouterPorts, Instant) {
    let owner = fixture_owner(); let scope = RpcConnectionScope::fresh(); let now = Instant::now();
    let clock = PortClock::for_original(owner.clone(), scope.clone());
    let ports = RouterPorts::for_original(owner.clone(), scope.clone(), clock, candidate, now).unwrap(); (owner, scope, ports, now)
}
fn reserve(ports: &RouterPorts, port: Option<u16>, now: Instant) -> PortLease {
    ports.reserve(&ports.owner, &ports.scope, ListenProtocol::Arc, port, false, now+Duration::from_secs(5), now).unwrap().unwrap()
}
// Unit-level host transition. Production entry is on_data(assignment,
// VerifiedRouterData); actual endpoint-TLS admission is covered separately.
fn feed(ports: &RouterPorts, ids: (u16, u32), route: Routing, payload: &[u8], now: Instant) -> Result<Option<PortDelivery>, PortError> {
    let order = ports.clock.stamp().unwrap(); ports.receive_routed(ids, route, payload, &order, now, now)
}
fn observe_host(ports: &RouterPorts, now: Instant) {
    assert!(feed(ports,IDS,Routing::default(),&[1],now).unwrap().is_none());
}
fn incoming(ports: &RouterPorts, port: u16, now: Instant) -> Option<PortDelivery> {
    let routing = Routing { destination: RouteAddress { port, ..RouteAddress::default() }, ..Routing::default() };
    feed(ports,IDS,routing,b"unaccepted-channel",now).unwrap()
}

#[test]
fn reservation_waits_for_received_original_host_and_does_not_publish_zero_ids() {
    let (owner,scope,ports,now) = setup(12345); let lease = reserve(&ports,None,now);
    assert_eq!(lease.inner.port,12345); assert!(lease.address_for(&owner,&scope).is_none());
    assert_eq!(ports.next_deadline(),Some(now+Duration::from_secs(5))); assert!(ports.needs_host_refresh(&owner,&scope,now));
    assert!(ports.state.lock().unwrap().addresses.is_empty());
    observe_host(&ports,now+Duration::from_secs(1));
    assert_eq!(lease.address_for(&owner,&scope),Some(LogicalAddress { network:IDS.0,node:IDS.1,port:12345 }));
    assert_eq!(ports.next_deadline(),None); assert!(!ports.needs_host_refresh(&owner,&scope,now+Duration::from_secs(1)));
}

#[test]
fn static_port_zero_or_occupied_is_rejected_without_consuming_a_generation() {
    let (owner,scope,ports,now) = setup(10000); let lease = reserve(&ports,Some(98),now);
    assert_eq!(lease.inner.generation,1);
    for (port,error) in [(0,PortError::Configuration),(98,PortError::InUse)] {
        assert_eq!(ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(port),false,now+Duration::from_secs(5),now).err(),Some(error));
    }
    assert_eq!(ports.state.lock().unwrap().next_generation,Some(2));
}

#[test]
fn dynamic_candidates_wrap_at_u16_and_skip_occupied_ports_above_native_floor() {
    let (_,_,ports,now) = setup(u16::MAX); let occupied_high = reserve(&ports,Some(u16::MAX),now);
    let occupied_low = reserve(&ports,Some(10000),now); let dynamic = reserve(&ports,None,now);
    assert_eq!(dynamic.inner.port,10001); assert_eq!(ports.state.lock().unwrap().next_port,10002);
    drop(occupied_high); drop(occupied_low);
    let (_,_,below,now) = setup(1); assert_eq!(reserve(&below,None,now).inner.port,10000);
}

#[test]
fn local_capacity_is_bounded_and_released_waiting_ports_restore_capacity() {
    let (owner,scope,ports,now) = setup(10000); let mut leases = Vec::new();
    for _ in 0..MAX_PORTS { leases.push(reserve(&ports,None,now)); }
    assert_eq!(ports.reserve(&owner,&scope,ListenProtocol::Arc,None,false,now+Duration::from_secs(5),now).err(),Some(PortError::Exhausted));
    drop(leases.pop()); let replacement = reserve(&ports,None,now);
    assert_eq!(ports.state.lock().unwrap().ports.len(),MAX_PORTS); assert_eq!(replacement.inner.generation,65);
}

#[test]
fn same_protocol_reuse_shares_one_listen_and_only_last_owner_drop_releases() {
    let (owner,scope,ports,now) = setup(10000); let first = reserve(&ports,Some(12000),now);
    let second = ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),true,now+Duration::from_secs(20),now).unwrap().unwrap();
    assert!(Arc::ptr_eq(&first.inner,&second.inner)); assert_eq!(ports.next_deadline(),Some(now+Duration::from_secs(5)));
    observe_host(&ports,now); drop(first); assert!(second.address_for(&owner,&scope).is_some());
    drop(second); let state = ports.state.lock().unwrap(); assert!(state.ports.is_empty()); assert!(state.addresses.is_empty());
}

#[test]
fn different_protocol_or_unrequested_reuse_cannot_borrow_the_original_listen() {
    let (owner,scope,ports,now) = setup(10000); let _lease = reserve(&ports,Some(12000),now);
    for (protocol,reuse) in [(ListenProtocol::Varc,true),(ListenProtocol::Arc,false)] {
        assert_eq!(ports.reserve(&owner,&scope,protocol,Some(12000),reuse,now+Duration::from_secs(5),now).err(),Some(PortError::InUse));
    }
}

#[test]
fn cancellation_before_local_ids_releases_reservation_and_late_host_cannot_restore_it() {
    let (_,_,ports,now) = setup(10000); let lease = reserve(&ports,Some(12000),now); let old = Arc::downgrade(&lease.inner);
    drop(lease); observe_host(&ports,now+Duration::from_secs(1));
    assert!(old.upgrade().is_none()); assert!(ports.state.lock().unwrap().ports.is_empty());
    assert!(incoming(&ports,12000,now+Duration::from_secs(1)).is_none());
}

#[test]
fn expiration_before_late_success_cannot_register_old_generation_or_release_new_one() {
    let (owner,scope,ports,now) = setup(10000); let old = reserve(&ports,Some(12000),now);
    let late = now+Duration::from_secs(5); observe_host(&ports,late); assert!(old.address_for(&owner,&scope).is_none());
    let replacement = reserve(&ports,Some(12000),late); assert!(replacement.address_for(&owner,&scope).is_some());
    assert_ne!(old.inner.generation,replacement.inner.generation); assert!(!old.release(&owner,&scope)); drop(old);
    assert_eq!(ports.state.lock().unwrap().addresses.values().copied().collect::<Vec<_>>(),vec![replacement.inner.generation]);
}

#[test]
fn explicit_release_is_idempotent_and_a_stale_drop_cannot_delete_the_full_address_index() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let old = reserve(&ports,Some(12000),now);
    assert!(old.release(&owner,&scope)); assert!(!old.release(&owner,&scope)); assert!(ports.state.lock().unwrap().addresses.is_empty());
    let new = reserve(&ports,Some(12000),now); drop(old);
    let address = new.address_for(&owner,&scope).unwrap(); assert_eq!(ports.state.lock().unwrap().addresses.get(&address),Some(&new.inner.generation));
}

#[test]
fn queued_delivery_is_bound_to_original_generation_not_a_reused_port_lookup() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let old = reserve(&ports,Some(12000),now);
    let queued = incoming(&ports,12000,now).unwrap(); assert!(queued.for_lease(&owner,&scope,&old).is_some());
    old.release(&owner,&scope); let replacement = reserve(&ports,Some(12000),now);
    assert!(queued.for_lease(&owner,&scope,&old).is_none()); assert!(queued.for_lease(&owner,&scope,&replacement).is_none());
    drop(old); assert!(queued.lease.upgrade().is_none());
    assert!(incoming(&ports,12000,now).unwrap().for_lease(&owner,&scope,&replacement).is_some());
}

#[test]
fn queued_deliveries_do_not_hold_ports_alive_after_owner_cancellation() {
    let (_,_,ports,now) = setup(10000); observe_host(&ports,now); let lease = reserve(&ports,Some(12000),now);
    let queued = incoming(&ports,12000,now).unwrap(); drop(lease);
    assert!(queued.lease.upgrade().is_none()); assert!(ports.state.lock().unwrap().addresses.is_empty());
}

#[test]
fn implicit_receive_destination_is_original_path_local_and_missing_source_node_stays_zero() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let lease = reserve(&ports,Some(12000),now);
    let routing = Routing { source: RouteAddress { port:98, ..RouteAddress::default() },
        destination: RouteAddress { mask:3, network:77, node:99, port:12000, ..RouteAddress::default() }, ..Routing::default() };
    let delivery = feed(&ports,IDS,routing,b"channel",now).unwrap().unwrap();
    let (route,payload) = delivery.for_lease(&owner,&scope,&lease).unwrap(); assert_eq!(payload,b"channel");
    assert_eq!(route.destination.network,IDS.0); assert_eq!(route.destination.node,IDS.1);
    assert_eq!(route.source.network,IDS.0); assert_eq!(route.source.node,0); assert_eq!(route.source.port,98);
}

#[test]
fn forwarded_source_fields_are_preserved_not_promoted_to_socket_or_account_authority() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let lease = reserve(&ports,Some(12000),now);
    let routing = Routing { source: RouteAddress { mask:7, network:10,node:11,forward_network:12,forward_node:13,port:98,..RouteAddress::default() },
        destination: RouteAddress { port:12000,..RouteAddress::default() },..Routing::default() };
    let delivery = feed(&ports,IDS,routing,b"raw",now).unwrap().unwrap(); let (route,_) = delivery.for_lease(&owner,&scope,&lease).unwrap();
    assert_eq!((route.source.network,route.source.node),(10,11)); assert_eq!((route.source.forward_network,route.source.forward_node),(12,13));
}

#[test]
fn router_control_and_unknown_ports_never_become_listen_deliveries() {
    let (_,_,ports,now) = setup(10000); let _lease = reserve(&ports,Some(12000),now);
    assert!(incoming(&ports,0,now).is_none()); assert!(incoming(&ports,12001,now).is_none());
    assert_eq!(ports.state.lock().unwrap().ports.len(),1);
}

#[test]
fn foreign_owner_scope_and_reuse_calls_are_ignored_before_time_mutation() {
    let (owner,scope,ports,now) = setup(10000); let other = fixture_owner(); let other_scope = RpcConnectionScope::fresh();
    let lease = reserve(&ports,Some(12000),now);
    for (candidate,connection) in [(&other,&scope),(&owner,&other_scope)] {
        assert!(ports.reserve(candidate,connection,ListenProtocol::Arc,Some(12000),true,now,now-Duration::from_secs(1)).unwrap().is_none());
        assert!(!ports.tick(candidate,connection,now-Duration::from_secs(1)).unwrap()); assert!(!lease.release(candidate,connection));
    }
    assert_eq!(ports.state.lock().unwrap().last_now,now); assert!(scope.is_open());
}

#[test]
fn original_clock_rollback_closes_only_its_scope_and_clears_both_indexes() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let lease = reserve(&ports,Some(12000),now);
    let other_scope = RpcConnectionScope::fresh();
    assert_eq!(ports.tick(&owner,&scope,now-Duration::from_millis(1)).err(),Some(PortError::ClockRollback));
    assert!(!scope.is_open()); assert!(other_scope.is_open()); assert!(lease.address_for(&owner,&scope).is_none());
    let state = ports.state.lock().unwrap(); assert!(state.ports.is_empty()); assert!(state.addresses.is_empty());
}

#[test]
fn host_change_invalidates_original_listens_instead_of_retargeting_them() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let lease = reserve(&ports,Some(12000),now);
    let queued = incoming(&ports,12000,now).unwrap();
    assert_eq!(feed(&ports,(IDS.0,IDS.1+1),Routing::default(),&[1],now).err(),Some(PortError::HostChanged));
    assert!(queued.for_lease(&owner,&scope,&lease).is_none()); assert!(!scope.is_open()); assert!(ports.state.lock().unwrap().addresses.is_empty());
}

#[test]
fn stale_host_observation_does_not_register_a_new_listen_until_received_refresh() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let later = now+HOST_FRESHNESS;
    assert!(ports.needs_host_refresh(&owner,&scope,later)); let lease = reserve(&ports,Some(12000),later);
    assert!(lease.address_for(&owner,&scope).is_none()); observe_host(&ports,later+Duration::from_millis(1));
    assert!(lease.address_for(&owner,&scope).is_some());
}

#[test]
fn original_wait_deadline_is_not_extended_by_same_listen_reuse_or_unrelated_ticks() {
    let (owner,scope,ports,now) = setup(10000); let first = reserve(&ports,Some(12000),now);
    let again = ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),true,now+Duration::from_secs(20),now+Duration::from_secs(4)).unwrap().unwrap();
    ports.tick(&owner,&scope,now+Duration::from_secs(5)).unwrap(); observe_host(&ports,now+Duration::from_secs(5));
    assert!(first.address_for(&owner,&scope).is_none()); assert!(again.address_for(&owner,&scope).is_none());
}

#[test]
fn allocation_generation_exhaustion_never_wraps_or_reuses_old_release_authority() {
    let (owner,scope,ports,now) = setup(10000); ports.state.lock().unwrap().next_generation = Some(u64::MAX);
    let last = reserve(&ports,Some(12000),now); assert_eq!(last.inner.generation,u64::MAX); last.release(&owner,&scope);
    assert_eq!(ports.reserve(&owner,&scope,ListenProtocol::Arc,Some(12000),false,now+Duration::from_secs(5),now).err(),Some(PortError::Exhausted));
}

#[test]
fn invalid_deadlines_do_not_allocate_ports_or_queue_callbacks() {
    let (owner,scope,ports,now) = setup(10000);
    for deadline in [now,now-Duration::from_secs(1),now+Duration::from_secs(31)] {
        assert_eq!(ports.reserve(&owner,&scope,ListenProtocol::Arc,None,false,deadline,now).err(),Some(PortError::Configuration));
    }
    assert!(ports.state.lock().unwrap().ports.is_empty()); assert_eq!(ports.state.lock().unwrap().next_generation,Some(1));
}

#[test]
fn dropping_original_table_invalidates_live_leases_without_releasing_another_table() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let lease = reserve(&ports,Some(12000),now);
    let queued = incoming(&ports,12000,now).unwrap(); let (other,other_scope,other_ports,other_now) = setup(10000);
    observe_host(&other_ports,other_now); let other_lease = reserve(&other_ports,Some(12000),other_now); drop(ports);
    assert!(!scope.is_open()); assert!(queued.for_lease(&owner,&scope,&lease).is_none());
    assert!(other_lease.address_for(&other,&other_scope).is_some()); assert!(other_scope.is_open());
}

#[tokio::test]
async fn aborting_the_owning_waiter_releases_reservation_after_its_join() {
    let (_,_,ports,now) = setup(10000); let lease = reserve(&ports,Some(12000),now);
    let (ready_tx,ready_rx) = tokio::sync::oneshot::channel();
    let job = tokio::spawn(async move { let _lease = lease; ready_tx.send(()).unwrap(); std::future::pending::<()>().await; });
    ready_rx.await.unwrap(); job.abort(); assert!(job.await.unwrap_err().is_cancelled());
    observe_host(&ports,now+Duration::from_secs(1)); assert!(ports.state.lock().unwrap().ports.is_empty());
    // This tests ownership of the logical listen only; no socket was created.
}

#[test]
fn port_and_delivery_debug_redact_logical_ids_and_payload() {
    let (_,_,ports,now) = setup(10000); observe_host(&ports,now); let lease = reserve(&ports,Some(12000),now);
    let delivery = incoming(&ports,12000,now).unwrap(); let text = format!("{ports:?} {lease:?} {delivery:?}");
    for value in ["12000","55667788","unaccepted-channel"] { assert!(!text.contains(value)); }
}

#[test]
fn retained_pre_admission_data_cannot_bind_to_a_later_port_generation_even_at_same_instant() {
    let (owner,scope,ports,now) = setup(10000); observe_host(&ports,now); let old = reserve(&ports,Some(12000),now);
    let old_order = ports.clock.stamp().unwrap(); old.release(&owner,&scope);
    let new = reserve(&ports,Some(12000),now);
    let route = Routing { destination: RouteAddress { port:12000,..RouteAddress::default() },..Routing::default() };
    assert!(ports.receive_routed(IDS,route,b"old",&old_order,now,now).unwrap().is_none());
    assert!(incoming(&ports,12000,now).unwrap().for_lease(&owner,&scope,&new).is_some());
}

#[test]
fn repeated_or_delayed_receipts_cannot_renew_host_freshness_using_processing_time() {
    let (owner,scope,ports,now) = setup(10000); let order = ports.clock.stamp().unwrap();
    ports.receive_routed(IDS,Routing::default(),&[1],&order,now,now).unwrap();
    let later = now+HOST_FRESHNESS;
    assert!(ports.receive_routed(IDS,Routing::default(),&[1],&order,now,later).unwrap().is_none());
    assert_eq!(ports.state.lock().unwrap().host_seen,Some(now)); assert!(ports.needs_host_refresh(&owner,&scope,later));
    let delayed = ports.clock.stamp().unwrap(); let lease = reserve(&ports,Some(12000),later);
    ports.receive_routed(IDS,Routing::default(),&[1],&delayed,now,later).unwrap();
    assert!(lease.address_for(&owner,&scope).is_none()); assert!(ports.needs_host_refresh(&owner,&scope,later));
}

#[test]
fn one_tracker_clock_can_create_only_one_book_and_foreign_clock_cannot_relabel_a_receipt() {
    let (owner,scope,ports,now) = setup(10000);
    assert_eq!(RouterPorts::for_original(owner.clone(),scope.clone(),ports.clock.clone(),10000,now).err(),Some(PortError::InUse));
    let foreign = PortClock::for_original(owner.clone(),scope.clone()); let order = foreign.stamp().unwrap();
    assert!(ports.receive_routed(IDS,Routing::default(),&[1],&order,now,now-Duration::from_secs(1)).unwrap().is_none());
    assert!(scope.is_open()); assert!(ports.state.lock().unwrap().local.is_none());
}

#[test]
fn admission_order_exhaustion_fails_without_reusing_an_old_receipt_or_port_birth() {
    let (owner,scope,ports,now) = setup(10000); ports.clock.inner.next.store(u64::MAX,Ordering::Release);
    assert_eq!(ports.reserve(&owner,&scope,ListenProtocol::Arc,None,false,now+Duration::from_secs(5),now).err(),Some(PortError::Exhausted));
    assert!(ports.state.lock().unwrap().ports.is_empty()); assert_eq!(ports.clock.inner.next.load(Ordering::Acquire),u64::MAX);
}

#[test]
fn channel_claim_requires_registered_original_port_and_excludes_retained_pre_claim_input(){
 let (owner,scope,ports,now)=setup(10000);let lease=reserve(&ports,Some(12000),now);
 assert!(lease.claim_channel(&owner,&scope,&ports.clock).unwrap().is_none());observe_host(&ports,now);
 let before=incoming(&ports,12000,now).unwrap();let claim=lease.claim_channel(&owner,&scope,&ports.clock).unwrap().unwrap();
 assert!(before.for_channel(&owner,&scope,&claim).is_none());let after=incoming(&ports,12000,now).unwrap();
 let (_,bytes,received)=after.for_channel(&owner,&scope,&claim).unwrap();assert_eq!(bytes,b"unaccepted-channel");assert_eq!(received,now);
 assert!(after.for_channel(&fixture_owner(),&scope,&claim).is_none());
}

#[test]
fn a_shared_listen_generation_cannot_create_a_second_or_restarted_channel(){
 let (owner,scope,ports,now)=setup(10000);observe_host(&ports,now);let lease=reserve(&ports,Some(12000),now);let shared=lease.clone();
 let claim=lease.claim_channel(&owner,&scope,&ports.clock).unwrap().unwrap();
 assert_eq!(shared.claim_channel(&owner,&scope,&ports.clock).err(),Some(PortError::InUse));drop(claim);
 assert!(lease.address_for(&owner,&scope).is_none());assert!(shared.claim_channel(&owner,&scope,&ports.clock).unwrap().is_none());
 assert!(scope.is_open());
}

#[test]
fn foreign_clock_owner_or_scope_cannot_consume_original_channel_claim(){
 let (owner,scope,ports,now)=setup(10000);observe_host(&ports,now);let lease=reserve(&ports,Some(12000),now);
 let clock=PortClock::for_original(owner.clone(),scope.clone());let other=fixture_owner();let other_scope=RpcConnectionScope::fresh();
 assert!(lease.claim_channel(&owner,&scope,&clock).unwrap().is_none());assert!(lease.claim_channel(&other,&scope,&ports.clock).unwrap().is_none());
 assert!(lease.claim_channel(&owner,&other_scope,&ports.clock).unwrap().is_none());assert!(!lease.inner.channel_claimed.load(Ordering::Acquire));
 assert!(lease.claim_channel(&owner,&scope,&ports.clock).unwrap().is_some());
}

#[test]
fn old_channel_claim_and_transmit_permit_cannot_release_or_enter_a_reused_port(){
 let (owner,scope,ports,now)=setup(10000);observe_host(&ports,now);let lease=reserve(&ports,Some(12000),now);
 let claim=lease.claim_channel(&owner,&scope,&ports.clock).unwrap().unwrap();let permit=claim.permit(&owner,&scope,&ports.clock).unwrap();
 let old=incoming(&ports,12000,now).unwrap();claim.release();let new=reserve(&ports,Some(12000),now);let fresh=new.claim_channel(&owner,&scope,&ports.clock).unwrap().unwrap();
 assert!(old.for_channel(&owner,&scope,&fresh).is_none());assert!(permit.address_for(&owner,&scope).is_none());drop(claim);drop(lease);
 assert!(fresh.address_for(&owner,&scope).is_some());assert!(incoming(&ports,12000,now).unwrap().for_channel(&owner,&scope,&fresh).is_some());
}

#[test]
fn channel_claim_order_exhaustion_burns_generation_without_reusing_old_admission(){
 let (owner,scope,ports,now)=setup(10000);observe_host(&ports,now);let lease=reserve(&ports,Some(12000),now);
 ports.clock.inner.next.store(u64::MAX,Ordering::Release);
 assert_eq!(lease.claim_channel(&owner,&scope,&ports.clock).err(),Some(PortError::Exhausted));
 assert_eq!(lease.claim_channel(&owner,&scope,&ports.clock).err(),Some(PortError::InUse));
 assert_eq!(ports.clock.inner.next.load(Ordering::Acquire),u64::MAX);
}
