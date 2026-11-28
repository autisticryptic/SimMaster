//! Original ARC + scheduler, with synthetic bootstrap/login and no devices or sockets.
use super::*;
use n::{arc_protocol::OwnedCallPackets, batch_scheduler::AliasWait, rpc_stream::{CodecError, RpcProperty},
    runtime_provision::ProvisionedCommonLogin};
use std::time::{Duration, Instant};

const PAIR: &[u8] = b"private-alias";
fn inputs() -> ArcLoginInputs<'static> {
    ArcLoginInputs { typed_identity:b"username:fixture-user", domain_name:b"fixture.invalid", selected_session_id:b"fixture-selected",
        phone_login_token:b"fixture-phone", application_key:b"fixture-key", domain_id:b"fixture-domain", application_id:b"fixture-app",
        device_info_cc:b"fixture-cc", client_properties:&[] }
}
fn plan(who: &AttemptOwner, dest: &RpcConnectionScope, policy: &RouterPolicy) -> ProvisionedCommonLogin {
    let mut source = ArcRpcSession::new(who.clone()); let scope = source.connection_scope();
    let (id, _) = source.prepare_bootstrap(b"fixture-key", &[]).unwrap();
    let mut w = Writer::new(); w.boolean(true).unwrap();
    let i = inputs(); w.properties(&[RpcProperty{key:b"DomainName",value:i.domain_name},RpcProperty{key:b"DomainId",value:i.domain_id},
        RpcProperty{key:b"AppId",value:i.application_id},RpcProperty{key:b"CC",value:i.device_info_cc}]).unwrap();
    w.bytes(b"DestEntry:sarc -h 42.7 -p 98;").unwrap(); for _ in 0..3 { w.bytes(b"").unwrap(); }
    let reply = packet(id, 0, w.finish().expose_to_transport());
    let Some(ArcReply::Bootstrap(matched)) = source.on_reply(who, &scope, &reply[1..]).unwrap() else { panic!("bootstrap not matched") };
    let plan = ProvisionedCommonLogin::from_reply(&matched, who, &scope, dest, policy, None, None).unwrap();
    drop(source); assert!(!scope.is_open()); plan
}
fn aliases(yes: bool, triples: &[(&[u8], &[u8], bool)], reason: &[u8]) -> Vec<u8> {
    let mut w = Writer::new(); w.boolean(yes).unwrap(); w.u32(triples.len() as u32).unwrap();
    for &(k, v, bit) in triples { w.bytes(k).unwrap(); w.bytes(v).unwrap(); w.boolean(bit).unwrap(); }
    w.bytes(reason).unwrap(); w.finish().expose_to_transport().to_vec()
}
fn single() -> Vec<u8> { aliases(true, &[(b"username", PAIR, true)], b"") }
fn end() -> Instant { Instant::now() + Duration::from_secs(10) }
struct Case { b: OfflineBackend, owner: AttemptOwner, scope: RpcConnectionScope, gate: Arc<ImReceiptGate>, clock: u64 }
impl Case {
    fn initial(entry: &[u8]) -> (Self, RouterPolicy, ProvisionedCommonLogin) {
        let owner = who(); let mut b = OfflineBackend::new(owner.clone()); let scope = b.connection_scope();
        b.claim_common_transport().unwrap(); b.bind_common_identity(entry).unwrap(); let policy = router();
        let plan = plan(&owner, &scope, &policy); (Self {b,owner,scope,gate:ImReceiptGate::fresh(),clock:0},policy,plan)
    }
    fn new() -> Self {
        let (mut f,p,plan) = Self::initial(b"DestEntry");
        let call = f.b.prepare_login_with_ue_alias_source(&plan, &p, f.gate.clone(), inputs(), &[]).unwrap();
        assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,end()).is_err());
        let mut w=Writer::new(); w.boolean(true).unwrap(); w.bytes(b"7_9").unwrap(); w.bytes(b"fixture-auth").unwrap(); w.properties(&[]).unwrap(); w.bytes(b"").unwrap();
        f.feed(&packet(call.call_id,0,w.finish().expose_to_transport()));
        assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,end()).is_err());
        f.b.configure(77,None).unwrap(); assert_eq!(f.cached(),None); f
    }
    fn begin(&mut self) -> AliasWait { self.b.begin_ue_alias_read(&self.owner,&self.scope,end()).unwrap() }
    fn swap(&mut self) -> OwnedCallPackets { self.clock+=1; self.b.prepare_swap(self.clock,&[]).unwrap() }
    fn feed(&mut self, bytes: &[u8]) -> Vec<OfflineEvent> { self.clock+=1; self.b.on_plaintext_packet(&self.owner,&self.scope,bytes,self.clock).unwrap() }
    fn result(&mut self, outgoing: &OwnedCallPackets, seq:i32, code:u32, data:&[u8]) {
        let reply = batch_result(payload(outgoing).fields,seq,code,data);
        let events = self.feed(&packet(outgoing.call_id,0,&reply));
        assert!(events.iter().all(|e| matches!(e, OfflineEvent::BatchRetirement(_))), "an alias result is not a hosting or business event");
    }
    fn read(&mut self, data:&[u8]) -> AliasWait { let wait=self.begin(); let p=self.swap(); self.result(&p,wait.sequence(),0,data); wait }
    fn cached(&self) -> Option<bool> { self.b.reported_ue_pair(&self.owner,&self.scope,b"username",PAIR) }
    fn fresh(&self, wait:&AliasWait) -> Option<bool> { wait.observed_pair(&self.owner,&self.scope,b"username",PAIR) }
}

#[test]
fn ue_alias_requires_matched_plan_exact_destination_router_and_unshadowed_login_before_request_id() {
    let (mut f,p,source) = Case::initial(b"DestEntry");
    let foreign_plan=plan(&f.owner,&RpcConnectionScope::fresh(),&p);
    assert!(f.b.prepare_login_with_ue_alias_source(&foreign_plan,&p,f.gate.clone(),inputs(),&[]).is_err());
    assert!(f.b.prepare_login_with_ue_alias_source(&source,&router(),f.gate.clone(),inputs(),&[]).is_err());
    for k in [b"DomainName".as_slice(), b"Arc.Realm"] {
        let properties = [RpcProperty {key:k,value:b"fixture.invalid"}];
        assert!(f.b.prepare_login_with_ue_alias_source(&source,&p,f.gate.clone(),ArcLoginInputs{client_properties:&properties,..inputs()},&[]).is_err());
    }
    assert!(f.b.prepare_login_with_ue_alias_source(&source,&p,f.gate.clone(),ArcLoginInputs{domain_name:b"wrong.invalid",..inputs()},&[]).is_err());
    assert_eq!(f.scope.native_rpc_resources(),(0,0));
    assert_eq!(f.b.prepare_login_with_ue_alias_source(&source,&p,f.gate.clone(),inputs(),&[]).unwrap().call_id,1);
    assert!(f.b.prepare_login_with_ue_alias_source(&source,&p,f.gate.clone(),inputs(),&[]).is_err());
    let (mut other,pol,plan) = Case::initial(b"WrongEntry");
    assert!(other.b.prepare_login_with_ue_alias_source(&plan,&pol,other.gate.clone(),inputs(),&[]).is_err());
    assert_eq!(other.scope.native_rpc_resources(),(0,0));
}
#[test]
fn ue_alias_ordinary_login_or_userentry_read_never_installs_snapshot() {
    let (mut b,owner,scope,policy)=current();
    let q=query(&owner,&scope,&policy,b"UserEntry"); let seq=b.queue_client_user_entry(q).unwrap();
    let call=b.prepare_swap(1,&[]).unwrap(); let words=payload(&call).fields;
    b.on_plaintext_packet(&owner,&scope,&packet(call.call_id,0,&batch_result(words,seq,0,&answer(true,&[(b"7_9",b"+12345")]))),2).unwrap();
    assert!(b.take_client_user_entry_replies().pop().unwrap().result.is_ok());
    assert!(b.begin_ue_alias_read(&owner,&scope,end()).is_err());
    assert_eq!(b.reported_ue_pair(&owner,&scope,b"phone",b"+12345"),None);
}
#[test]
fn ue_alias_emits_one_version_only_user_call_with_original_outer_auth_and_shared_sequence() {
    let mut f=Case::new(); let host=f.b.queue_hosting(f.owner.clone(),"uid:7_9",STATUS_REGISTER,&registration(),&[]).unwrap();
    let wait=f.begin(); assert_eq!(wait.sequence(),host+1); let call=f.swap(); let batch=payload(&call);
    assert_eq!(batch.requests.len(),2); assert!(call.has_business_work());
    let read=&batch.requests[&wait.sequence()]; assert_eq!(read.method,b"getRelations.User.User");
    assert_eq!(read.metadata,[RpcProperty{key:b"id",value:b"#User"}]);
    assert_eq!(read.arguments,[0,0,1,0,0]); Reader::versioned(read.arguments).unwrap().finish().unwrap();
    assert_eq!(batch.requests[&host].method,n::rpc_arguments::UPDATE_STATUS_METHOD.as_bytes());
    drop(wait);let next=f.swap();assert_eq!(payload(&next).requests.keys().copied().collect::<Vec<_>>(),[host]);
}
#[test]
fn ue_alias_empty_ack_and_unrelated_direct_or_nested_reply_cannot_replace_unknown() {
    let mut f=Case::new();let wait=f.begin();let seq=wait.sequence();let first=f.swap();
    f.feed(&packet(seq,0,&single()));assert_eq!(f.cached(),None);
    f.feed(&packet(first.call_id,0,&ack_without_result(payload(&first).fields)));
    assert_eq!(f.b.scheduler.as_ref().unwrap().history_count(),0);assert_eq!(f.fresh(&wait),None);
    let unmatched=f.swap();assert!(payload(&unmatched).requests.is_empty());f.result(&unmatched,seq+1,0,&single());assert_eq!(f.cached(),None);
    let matched=f.swap();f.result(&matched,seq,0,&single());assert_eq!(f.fresh(&wait),Some(true));
    let duplicate=f.swap();f.result(&duplicate,seq,0,&aliases(true,&[],b""));assert_eq!(f.fresh(&wait),Some(true));
}
#[test]
fn ue_alias_known_empty_only_after_confirmed_full_zero_result() {
    let mut f=Case::new();let failed=f.read(&aliases(false,&[],b""));assert_eq!(f.cached(),None);assert_eq!(f.fresh(&failed),None);
    let empty=f.read(&aliases(true,&[],b""));assert_eq!(f.fresh(&empty),Some(false));assert_eq!(f.cached(),Some(false));
    assert_eq!(empty.observed_pair(&f.owner,&f.scope,b"",PAIR),None);
    let next=f.read(&single());assert_eq!(f.fresh(&next),Some(true));assert_eq!(f.fresh(&empty),None);
}
#[test]
fn ue_alias_failed_refresh_never_uses_previous_rows_as_new_receipt() {
    let mut f=Case::new();let first=f.read(&single());assert_eq!(f.fresh(&first),Some(true));
    let mut trailing=single();trailing.extend_from_slice(&[0,0,0,0,1]);
    for (code,raw) in [(0,aliases(false,&[],b"")),(0,aliases(true,&[],b"failed")),(65537,vec![]),(0,trailing),
        (0,aliases(true,&[(b"username",PAIR,false),(b"username",PAIR,false)],b"")),(0,vec![0,1,255,255,255,255])] {
        let wait=f.begin();let p=f.swap();f.result(&p,wait.sequence(),code,&raw);
        assert!(f.scope.is_open());assert_eq!(f.cached(),Some(true));assert_eq!(f.fresh(&wait),None);assert_eq!(f.fresh(&first),None);
    }
}
#[test]
fn ue_alias_both_flag_values_are_distinct_and_preempt_visits_false_first() {
    let mut f=Case::new();let w=f.read(&aliases(true,&[(b"username",PAIR,true),(b"username",PAIR,false),(b"phone",PAIR,true)],b""));
    assert_eq!(f.fresh(&w),Some(true));f.b.preempt_ue_pair(&f.owner,&f.scope,b"username",PAIR);
    assert_eq!(f.cached(),Some(true));assert_eq!(f.fresh(&w),None);
    f.b.preempt_ue_pair(&f.owner,&f.scope,b"phone",PAIR);assert_eq!(f.b.reported_ue_pair(&f.owner,&f.scope,b"phone",PAIR),Some(false));
    assert_eq!(f.b.reported_ue_pair(&f.owner,&f.scope,b"Username",PAIR),Some(false));assert_eq!(f.b.reported_ue_pair(&f.owner,&f.scope,b"username",b"PRIVATE-ALIAS"),Some(false));
}
#[test]
fn ue_alias_preempt_invalidates_already_submitted_refresh_without_inventing_empty_snapshot() {
    let mut f=Case::new();let wait=f.begin();let call=f.swap();
    f.b.preempt_ue_pair(&f.owner,&f.scope,b"username",PAIR);f.result(&call,wait.sequence(),0,&single());assert_eq!(f.cached(),None);
    let w=f.read(&single());f.b.preempt_ue_pair(&f.owner,&f.scope,b"",PAIR);assert_eq!(f.fresh(&w),Some(true));
    let wait=f.begin();let call=f.swap();f.b.preempt_ue_pair(&f.owner,&f.scope,b"username",PAIR);f.result(&call,wait.sequence(),0,&single());assert_eq!(f.cached(),Some(false));assert_eq!(f.fresh(&wait),None);
}
#[test]
fn ue_alias_cancelled_read_and_reused_input_cannot_accept_late_old_sequence() {
    let mut f=Case::new();let first=f.begin();let old=first.sequence();let sent=f.swap();drop(first);
    let next=f.begin();assert_eq!(next.sequence(),old+1);f.result(&sent,old,0,&single());assert_eq!(f.cached(),None);
    let call=f.swap();assert!(!payload(&call).requests.contains_key(&old));f.result(&call,next.sequence(),0,&aliases(true,&[],b""));assert_eq!(f.fresh(&next),Some(false));
}
#[test]
fn ue_alias_refuses_concurrent_read_and_wrong_original_without_burning_next_id() {
    let mut f=Case::new();assert!(f.b.begin_ue_alias_read(&who(),&f.scope,end()).is_err());assert!(f.b.begin_ue_alias_read(&f.owner,&RpcConnectionScope::fresh(),end()).is_err());
    let first=f.begin();assert_eq!(first.sequence(),1);assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,end()).is_err());drop(first);
    assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,Instant::now()).is_err());
    assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,Instant::now()+Duration::from_secs(31)).is_err());
    assert_eq!(f.begin().sequence(),2);
}
#[test]
fn ue_alias_realm_invalidation_is_sticky_for_original_context_even_with_same_login_bytes() {
    let mut f=Case::new();let accepted=f.read(&single());let wait=f.begin();let sent=f.swap();
    f.b.invalidate_ue_context(&who(),&f.scope);assert_eq!(f.cached(),Some(true));
    f.b.invalidate_ue_context(&f.owner,&f.scope);assert_eq!(f.cached(),None);f.result(&sent,wait.sequence(),0,&single());assert_eq!(f.fresh(&accepted),None);
    assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,end()).is_err());assert!(f.scope.is_open());
    let mut second=Case::new();let current=second.read(&aliases(true,&[],b""));assert_eq!(second.fresh(&current),Some(false));assert_eq!(second.fresh(&wait),None);
}
#[test]
fn ue_alias_gate_or_sticky_session_revocation_disallows_prior_observation() {
    for revoke in [false,true] {let mut f=Case::new();let old=f.read(&single());let wait=f.begin();let sent=f.swap();if revoke {f.scope.mark_session_revoked();} else {f.gate.retire();}
        assert_eq!(f.cached(),None);assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,end()).is_err());
        if !revoke {f.result(&sent,wait.sequence(),0,&single());}assert_eq!(f.fresh(&old),None);assert_eq!(f.fresh(&wait),None);
    }
}
#[test]
fn ue_alias_stale_reply_after_absolute_deadline_does_not_disconnect_or_install_rows() {
    let mut f=Case::new();let deadline=Instant::now()+Duration::from_secs(1);let wait=f.b.begin_ue_alias_read(&f.owner,&f.scope,deadline).unwrap();let sent=f.swap();
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));f.result(&sent,wait.sequence(),0,&single());
    assert_eq!(f.cached(),None);assert_eq!(f.fresh(&wait),None);assert!(f.scope.is_open());
}
#[test]
fn ue_alias_unregistration_and_logout_invalidate_before_late_read() {
    for unhost in [true,false]{let mut f=Case::new();let _known=f.read(&single());let wait=f.begin();let old=f.swap();
        if unhost {let seq=f.b.queue_hosting(f.owner.clone(),"uid:7_9",STATUS_UNREGISTER,&registration(),&[]).unwrap();let sent=f.swap();assert_eq!(payload(&sent).requests.keys().copied().collect::<Vec<_>>(),[seq]);f.result(&old,wait.sequence(),0,&single());}
        else {let logout=f.b.prepare_logout(&[]).unwrap();assert_ne!(logout.call_id,old.call_id);}
        assert_eq!(f.cached(),None);assert_eq!(f.fresh(&wait),None);assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,end()).is_err());
    }
}
#[test]
fn ue_alias_partially_moved_packets_keep_original_capacity_until_last_drop() {
    let mut f=Case::new();let wait=f.begin();let mut call=f.swap();let packets=std::mem::take(&mut call.packets);drop(wait);f.b.close();drop(call);
    let held=f.scope.native_rpc_resources();assert!(held.0>=3 && held.1>0);assert!(packets.iter().all(|p|!p.expose_to_transport().is_empty()));drop(packets);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}
#[test]
fn ue_alias_full_shared_budget_refuses_result_without_replacing_previous_or_closing() {
    let mut f=Case::new();let _previous=f.read(&single());let wait=f.begin();let call=f.swap();let remaining=128*1024-f.scope.native_rpc_resources().1;let held=f.scope.reserve_native_rpc(remaining).unwrap();
    f.result(&call,wait.sequence(),0,&aliases(true,&[],b""));assert_eq!(f.fresh(&wait),None);assert_eq!(f.cached(),Some(true));assert!(f.scope.is_open());drop(held);
    let mut slots=Vec::new();while let Ok(s)=f.scope.reserve_native_rpc(1){slots.push(s);}assert!(f.b.begin_ue_alias_read(&f.owner,&f.scope,end()).is_err());drop(slots);assert_eq!(f.begin().sequence(),wait.sequence()+1);
}
#[test]
fn ue_alias_budget_refusal_before_encoding_preserves_queued_hosting() {
    let mut f=Case::new();let host=f.b.queue_hosting(f.owner.clone(),"uid:7_9",STATUS_REGISTER,&registration(),&[]).unwrap();let wait=f.begin();
    let held=f.scope.reserve_native_rpc(128*1024-f.scope.native_rpc_resources().1).unwrap();f.clock+=1;assert!(f.b.prepare_swap(f.clock,&[]).is_err());assert!(f.scope.is_open());
    drop(held);drop(wait);let p=f.swap();assert_eq!(payload(&p).requests.keys().copied().collect::<Vec<_>>(),[host]);
}
#[test]
fn ue_alias_entire_inbound_group_finishes_before_fresh_view_is_published() {
    for later_bad in [true,false] {let mut f=Case::new();let wait=f.begin();let call=f.swap();let bytes=packet(call.call_id,0,&batch_result(payload(&call).fields,wait.sequence(),0,&single()));
        let owner=f.owner.clone();let scope=f.scope.clone();let result=f.b.with_alias_group(&owner,&scope,|backend,staged|{
            let events=backend.receive_packet(&owner,&scope,&bytes,5,staged)?;assert!(staged.is_some());
            assert_eq!(wait.observed_pair(&owner,&scope,b"username",PAIR),None);assert_eq!(backend.reported_ue_pair(&owner,&scope,b"username",PAIR),None);
            if later_bad {backend.receive_packet(&owner,&scope,&[1,0xff],5,staged)?;}Ok(events)
        });assert_eq!(result.is_err(),later_bad);assert_eq!(f.fresh(&wait),if later_bad{None}else{Some(true)});
    }
}
#[test]
fn ue_alias_codec_validates_all_prefixes_footer_keys_and_trailing_bytes_and_refunds() {
    let scope=RpcConnectionScope::fresh();let bare=single();let mut full=bare.clone();full.extend_from_slice(&[0,0,0,1, 0,0,0,1,b'x', 0,0,0,1,b'y']);
    for i in 0..=full.len(){let result=n::ue_alias_data::AliasRows::decode(&scope,0,&full[..i]);assert_eq!(result.is_ok(),i==bare.len()||i==full.len());drop(result);assert_eq!(scope.native_rpc_resources(),(0,0));}
    let mut duplicate=bare.clone();duplicate.extend_from_slice(&[0,0,0,2]);duplicate.extend_from_slice(&full[bare.len()+4..]);duplicate.extend_from_slice(&full[bare.len()+4..]);
    assert_eq!(n::ue_alias_data::AliasRows::decode(&scope,0,&duplicate).unwrap_err(),CodecError::DuplicateKey);full.push(1);assert_eq!(n::ue_alias_data::AliasRows::decode(&scope,0,&full).unwrap_err(),CodecError::TrailingData);
    let rows=n::ue_alias_data::AliasRows::decode(&scope,0,&single()).unwrap();assert_eq!(format!("{rows:?}"),"AliasRows(<redacted>)");drop(rows);assert_eq!(scope.native_rpc_resources(),(0,0));
}
