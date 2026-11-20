//! Protocol SOURCE for mounted im_protocol_tests::directory; Actions execution only.
//! The fake peer speaks the current codec; this is not original SDK execution.
//! No directory DATA is an IM receipt, cursor, WRITE or SDK authorization.
use super::*;
use n::{group_directory_data as directory, rpc_im::{ImQuery, ImReplyData},
    rpc_stream::{CodecError, MAX_MAP_ENTRIES, MAX_STREAM_BYTES}};

// Local result writers and original prepare-im-read roundtrip helpers only.
fn directory_relations(accepted:bool, reported:bool, reason:&[u8])->Vec<u8>{
    let mut w=Writer::new();
    w.boolean(accepted).unwrap();w.boolean(reported).unwrap();
    for (label,flags,first,second) in [
        (b"opaque-first-label".as_slice(),0xf123_4567,b"not-a-route\0".as_slice(),b"not-a-uid".as_slice()),
        (b"opaque-second-label".as_slice(),0x8000_0000,b"Org/1-2".as_slice(),b"tcp -h foreign -p 1".as_slice()),
    ]{
        w.u32(1).unwrap();w.bytes(label).unwrap();w.u32(flags).unwrap();
        w.bytes(first).unwrap();w.bytes(second).unwrap();
        w.properties(&[RpcProperty{key:b"grant",value:b"opaque-not-authority"}]).unwrap();
    }
    w.u32(1).unwrap();w.bytes(b"opaque-name").unwrap();w.i64(i64::MIN).unwrap();
    w.bytes(reason).unwrap();
    w.properties(&[RpcProperty{key:b"token",value:b"opaque-footer-not-token"}]).unwrap();
    w.finish().expose_to_transport().to_vec()
}
fn directory_properties(accepted:bool, reason:&[u8])->Vec<u8>{
    let mut w=Writer::new();w.boolean(accepted).unwrap();
    w.properties(&[RpcProperty{key:b"P:P_MR",value:b"9223372036854775807"},
        RpcProperty{key:b"WRITE",value:b"opaque-not-permission"}]).unwrap();
    w.bytes(reason).unwrap();w.properties(&[RpcProperty{key:b"grant",value:b"opaque-footer"}]).unwrap();
    w.finish().expose_to_transport().to_vec()
}
fn directory_request(f:&mut Fixture,query:Arc<ImQuery>)->n::arc_protocol::OwnedCallPackets{
    f.backend.prepare_im_read(query,f.gate.clone(),Instant::now()+Duration::from_secs(30)).unwrap()
}
fn directory_reply(f:&mut Fixture,call_id:i32,code:u32,body:&[u8]){
    let rpc=encode_reply(call_id,code,body).unwrap();let mut packet=vec![1];
    packet.extend_from_slice(rpc.expose_to_transport());
    assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&packet,1).unwrap().is_empty());
}
fn directory_read(f:&mut Fixture,query:Arc<ImQuery>,body:&[u8])->n::rpc_im::OwnedImReply{
    let packet=directory_request(f,query);directory_reply(f,packet.call_id,0,body);
    let mut replies=f.backend.take_im_replies();assert_eq!(replies.len(),1);replies.pop().unwrap()
}

#[test]
fn directory_policy_is_independent_strict_default_false_and_preserves_old_private_bytes(){
    let old=r#"{"notification_timeout_ms":30000}"#;
    let config:NativeImConfig=serde_json::from_str(old).unwrap();
    assert!(!config.receive_info&&!config.directory_queries);
    assert_eq!(serde_json::to_string(&config).unwrap(),old);
    for (info,queries) in [(false,false),(true,false),(false,true),(true,true)]{
        let json=format!("{{\"notification_timeout_ms\":30000,\"receive_info\":{info},\"directory_queries\":{queries}}}");
        let config:NativeImConfig=serde_json::from_str(&json).unwrap();
        assert_eq!((config.receive_info,config.directory_queries),(info,queries));
        let encoded=serde_json::to_string(&config).unwrap();
        assert_eq!(encoded.contains("receive_info"),info);
        assert_eq!(encoded.contains("directory_queries"),queries);
    }
    for json in [r#"{"notification_timeout_ms":30000,"uid":"caller"}"#,
        r#"{"notification_timeout_ms":30000,"directory_queries":1}"#,
        r#"{"notification_timeout_ms":30000,"directory_queries":null}"#,
        r#"{"notification_timeout_ms":30000,"directory_queries":"true"}"#,
        r#"{"notification_timeout_ms":30000,"skip":false}"#]{
        assert!(serde_json::from_str::<NativeImConfig>(json).is_err());
    }
    for f in [Fixture::new(false),Fixture::new(true),Fixture::with_info(true,true)]{
        let stored=f.temp.database.with_conn(|c|account_store::load(c,f.run.line_id())).unwrap().unwrap();
        let before=stored.config.encode_private().unwrap();let mut config=stored.config.clone();
        if let Some(im)=config.profile.native_receivers.as_mut().unwrap().im_messages.as_mut(){
            assert!(!im.directory_queries);im.directory_queries=false;
        }
        assert_eq!(before,config.encode_private().unwrap());
        assert!(!std::str::from_utf8(&before).unwrap().contains("directory_queries"));
    }
}

#[test]
fn directory_disabled_has_no_packets_ids_budget_or_sql_and_leaves_original_scope_healthy(){
    for mut f in [Fixture::new(false),Fixture::new(true),Fixture::with_info(true,true)]{
        for query in [ImQuery::relations(None,-1).unwrap(),
            ImQuery::relations(Some(directory::GroupId::parse(b"1-2").unwrap()),0).unwrap(),
            ImQuery::group_properties(directory::GroupId::parse(b"1-2").unwrap()).unwrap()]{
            assert!(f.backend.prepare_im_read(Arc::new(query),f.gate.clone(),Instant::now()+Duration::from_secs(30)).is_err());
            assert_eq!(f.scope.native_rpc_resources(),(0,0));
        }
        assert!(f.backend.take_im_replies().is_empty());assert!(f.scope.is_open());assert_eq!(f.count(),0);
        if f.backend.captured_im_binding().is_some(){
            let packet=directory_request(&mut f,Arc::new(ImQuery::refresh(&[],-1).unwrap()));
            assert_eq!(packet.call_id,2,"directory refusal must precede original ID allocation");
            f.backend.retire_im_read(packet.call_id);
        }
        assert_eq!(f.scope.native_rpc_resources(),(0,0));
    }
}

#[test]
fn directory_default_current_user_wire_is_actual_matched_sdk_uid_not_caller_uid(){
    let mut f=Fixture::with_directory(true,false,true);
    let packet=directory_request(&mut f,Arc::new(ImQuery::relations(None,i64::MAX).unwrap()));
    let request=decode_request(&packet.packets[0].expose_to_transport()[1..]).unwrap();
    assert_eq!(request.method,directory::RELATIONS_METHOD.as_bytes());assert_eq!(request.metadata.len(),2);
    assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");
    let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"fixture-user");
    assert_eq!(r.i64().unwrap(),i64::MAX);r.finish().unwrap();
    for forged in [b"caller-uid".as_slice(),b"fixture-phone",b"fixture-auth",b"Org/1-2"]{
        assert!(directory::GroupId::parse(forged).is_err());
    }
    f.backend.retire_im_read(packet.call_id);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}

#[test]
fn directory_explicit_group_is_opaque_one_dash_two_not_org_box_or_endpoint(){
    let mut f=Fixture::with_directory(true,false,true);
    let packet=directory_request(&mut f,Arc::new(ImQuery::relations(Some(directory::GroupId::parse(b"1-2").unwrap()),-1).unwrap()));
    let request=decode_request(&packet.packets[0].expose_to_transport()[1..]).unwrap();
    let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"1-2");
    assert_eq!(r.i64().unwrap(),-1);r.finish().unwrap();
    for bad in [b"Org/1-2".as_slice(),b"P2P/1_2",b"Group:tcp -h foreign -p 1;",b"",b"01-2",b"1-0",b"1-2\0"]{
        assert!(directory::GroupId::parse(bad).is_err());
    }
    assert!(ImQuery::relations(None,-2).is_err());assert_eq!(f.backend.phase(),ArcPhase::Authenticated);
    f.backend.retire_im_read(packet.call_id);
}

#[test]
fn directory_properties_serializes_only_original_one_empty_prefix_set(){
    let mut f=Fixture::with_directory(true,true,true);
    let packet=directory_request(&mut f,Arc::new(ImQuery::group_properties(directory::GroupId::parse(b"1-2").unwrap()).unwrap()));
    let request=decode_request(&packet.packets[0].expose_to_transport()[1..]).unwrap();
    assert_eq!(request.method,directory::PROPERTIES_METHOD.as_bytes());assert_eq!(request.metadata.len(),2);
    assert_eq!(request.metadata.iter().find(|p|p.key==b"id").unwrap().value,b"#Group");
    assert_eq!(request.metadata.iter().find(|p|p.key==b"token").unwrap().value,b"fixture-auth");
    let mut r=Reader::versioned(request.arguments).unwrap();assert_eq!(r.bytes().unwrap(),b"1-2");
    assert_eq!(r.u32().unwrap(),1);assert_eq!(r.bytes().unwrap(),b"");r.finish().unwrap();
    f.backend.retire_im_read(packet.call_id);assert_eq!(f.scope.native_rpc_resources(),(0,0));
}

#[test]
fn directory_reply_requires_original_owner_query_arc_gate_scope_and_unchanged_deadline(){
    let mut f=Fixture::with_directory(true,false,true);let foreign=Fixture::with_directory(true,false,true);
    let query=Arc::new(ImQuery::relations(None,-1).unwrap());let deadline=Instant::now()+Duration::from_secs(30);
    let packet=f.backend.prepare_im_read(query.clone(),f.gate.clone(),deadline).unwrap();
    let raw=reply_packet(packet.call_id,&directory_relations(true,true,b""));
    assert!(f.backend.on_plaintext_packet(&foreign.owner,&f.scope,&raw,1).unwrap().is_empty());
    assert!(f.backend.on_plaintext_packet(&f.owner,&foreign.scope,&raw,1).unwrap().is_empty());
    assert!(f.backend.take_im_replies().is_empty());
    directory_reply(&mut f,packet.call_id,0,&directory_relations(true,true,b""));
    let replies=f.backend.take_im_replies();assert_eq!(replies.len(),1);let reply=&replies[0];
    assert!(reply.matches(&f.owner,&f.scope));assert!(!reply.matches(&foreign.owner,&foreign.scope));
    assert!(Arc::ptr_eq(reply.request().query_arc(),&query));assert!(Arc::ptr_eq(&reply.request().gate(),&f.gate));
    assert!(Arc::ptr_eq(reply.request().binding(),&f.backend.captured_im_binding().unwrap()));
    assert_eq!(reply.request().deadline(),deadline);assert_eq!(reply.request().call_id(),packet.call_id);
    directory_reply(&mut f,packet.call_id,0,&directory_relations(true,true,b""));
    assert!(f.backend.take_im_replies().is_empty());
    f.gate.retire();assert!(!reply.matches(&f.owner,&f.scope));assert!(reply.result().is_err());
    assert_eq!(reply.request().deadline(),deadline);assert!(f.scope.is_open());
}

#[test]
fn directory_matched_relations_preserves_both_opaque_maps_names_time_and_no_other_grants(){
    let mut f=Fixture::with_directory(true,false,true);
    let reply=directory_read(&mut f,Arc::new(ImQuery::relations(None,-1).unwrap()),&directory_relations(true,false,b""));
    let ImReplyData::Relations(data)=reply.result().unwrap()else{panic!("original relations DATA")};
    assert!(data.accepted);assert!(!data.reported);assert_eq!(directory::record_count(&data).unwrap(),2);
    assert_eq!(data.first[0].label,b"opaque-first-label");assert_eq!(data.first[0].flags,0xf123_4567);
    assert_eq!(data.first[0].first,b"not-a-route\0");assert_eq!(data.first[0].second,b"not-a-uid");
    assert_eq!(data.second[0].label,b"opaque-second-label");assert_eq!(data.second[0].flags,0x8000_0000);
    assert_eq!(data.second[0].first,b"Org/1-2");assert_eq!(data.second[0].second,b"tcp -h foreign -p 1");
    assert_eq!(data.second[0].properties[0].value,b"opaque-not-authority");
    assert_eq!(data.names,[b"opaque-name".as_slice()]);assert_eq!(data.time,i64::MIN);
    assert_eq!(data.footer.properties[0].value,b"opaque-footer-not-token");
    assert!(n::im_inbox::CursorSession::from_refresh(f.run.clone(),f.gate.clone(),&reply).is_err());
    assert!(f.backend.prepare_im_read(Arc::new(ImQuery::set_received(n::im_data::ImBox::parse(b"Org/1-2").unwrap(),7).unwrap()),f.gate.clone(),Instant::now()+Duration::from_secs(30)).is_err());
    assert_eq!(f.count(),0);assert!(f.temp.database.get_recent_app_events(Some(f.run.line_id()),10).unwrap().is_empty());
    assert!(f.temp.database.get_sms_messages_for_channel(10,0,None,Some(f.run.line_id())).unwrap().is_empty());
    assert!(f.temp.database.get_call_history_for_line(f.run.line_id(),10,0).unwrap().is_empty());
    let cursors:i64=f.temp.database.with_conn(|c|c.query_row("SELECT COUNT(*) FROM app_voice_im_receive_cursors",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();
    assert_eq!(cursors,0);assert!(f.backend.take_received_replies().is_empty());assert!(f.backend.im_write_guard().0.is_empty());
    let printed=format!("{reply:?} {:?} {data:?}",reply.request());
    for private in ["fixture-user","fixture-auth","opaque-","Org/1-2","foreign"]{assert!(!printed.contains(private));}
}

#[test]
fn directory_false_or_reason_footer_properties_are_data_not_positive_ack(){
    for (accepted,reason) in [(false,b"".as_slice()),(true,b"opaque-refusal".as_slice())]{
        let mut f=Fixture::with_directory(true,true,true);
        let reply=directory_read(&mut f,Arc::new(ImQuery::group_properties(directory::GroupId::parse(b"1-2").unwrap()).unwrap()),&directory_properties(accepted,reason));
        let ImReplyData::GroupProperties(data)=reply.result().unwrap()else{panic!("properties DATA")};
        assert!(!data.accepted);assert_eq!(data.properties.len(),2);assert_eq!(data.footer.reason,reason);
        assert_eq!(data.footer.properties[0].value,b"opaque-footer");
        let reply=directory_read(&mut f,Arc::new(ImQuery::relations(None,-1).unwrap()),&directory_relations(accepted,true,reason));
        let ImReplyData::Relations(data)=reply.result().unwrap()else{panic!("relations DATA")};
        assert!(!data.accepted);assert!(data.reported);assert_eq!(data.footer.reason,reason);
        assert!(f.scope.is_open());assert_eq!(f.backend.phase(),ArcPhase::Authenticated);
        assert!(f.backend.take_received_replies().is_empty());assert_eq!(f.count(),0);
    }
}

#[test]
fn directory_wrong_ids_malformed_whole_maps_trailing_and_remote_versions_never_escape_positive(){
    let mut f=Fixture::with_directory(true,false,true);
    let packet=directory_request(&mut f,Arc::new(ImQuery::relations(None,-1).unwrap()));
    directory_reply(&mut f,packet.call_id+99,0,&directory_relations(true,true,b""));assert!(f.backend.take_im_replies().is_empty());
    directory_reply(&mut f,packet.call_id,0,&directory_relations(true,true,b""));assert_eq!(f.backend.take_im_replies().len(),1);
    let mut duplicate=Writer::new();duplicate.boolean(true).unwrap();duplicate.boolean(true).unwrap();duplicate.u32(2).unwrap();
    for _ in 0..2{duplicate.bytes(b"same").unwrap();duplicate.u32(0).unwrap();duplicate.bytes(b"").unwrap();duplicate.bytes(b"").unwrap();duplicate.properties(&[]).unwrap();}
    duplicate.u32(0).unwrap();duplicate.u32(0).unwrap();duplicate.i64(0).unwrap();duplicate.bytes(b"").unwrap();
    let mut duplicate_properties=Writer::new();duplicate_properties.boolean(true).unwrap();duplicate_properties.u32(2).unwrap();
    for _ in 0..2{duplicate_properties.bytes(b"same").unwrap();duplicate_properties.bytes(b"value").unwrap();}duplicate_properties.bytes(b"").unwrap();
    let duplicate=duplicate.finish().expose_to_transport().to_vec();
    let duplicate_properties=duplicate_properties.finish().expose_to_transport().to_vec();
    for properties in [false,true]{
        let valid=if properties{directory_properties(true,b"")}else{directory_relations(true,true,b"")};
        let mut cases=Vec::new();for end in 0..valid.len(){cases.push((0,valid[..end].to_vec()));}
        let mut trailing=valid.clone();trailing.push(0);cases.push((0,trailing));
        let mut wrong_stream=valid.clone();wrong_stream[0]=1;cases.push((0,wrong_stream));
        cases.push((0,if properties{duplicate_properties.clone()}else{duplicate.clone()}));
        let mut oversized=Writer::new();oversized.boolean(true).unwrap();if !properties{oversized.boolean(true).unwrap();}
        oversized.u32((MAX_MAP_ENTRIES+1)as u32).unwrap();cases.push((0,oversized.finish().expose_to_transport().to_vec()));
        for code in [1,65536,65537,u32::MAX]{cases.push((code,valid.clone()));}
        for (code,body) in cases{
            let mut f=Fixture::with_directory(true,false,true);
            let query=if properties{ImQuery::group_properties(directory::GroupId::parse(b"1-2").unwrap()).unwrap()}else{ImQuery::relations(None,-1).unwrap()};
            let packet=directory_request(&mut f,Arc::new(query));let rpc=encode_reply(packet.call_id,code,&body).unwrap();let mut raw=vec![1];raw.extend_from_slice(rpc.expose_to_transport());
            assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&raw,1).is_err());
            assert!(f.backend.take_im_replies().is_empty());assert!(!f.scope.is_open());assert_eq!(f.count(),0);
        }
    }
}

#[test]
fn directory_retired_closed_revoked_or_expired_preadmission_cannot_charge_or_query(){
    for mode in 0..4{
        let mut f=Fixture::with_directory(true,false,true);let mut deadline=Instant::now()+Duration::from_secs(30);
        match mode{0=>f.gate.retire(),1=>f.scope.close(),2=>f.scope.mark_session_revoked(),_=>deadline=Instant::now()}
        for query in [ImQuery::relations(None,-1).unwrap(),ImQuery::group_properties(directory::GroupId::parse(b"1-2").unwrap()).unwrap()]{
            assert!(f.backend.prepare_im_read(Arc::new(query),f.gate.clone(),deadline).is_err());assert_eq!(f.scope.native_rpc_resources(),(0,0));
        }
        assert!(f.backend.take_im_replies().is_empty());assert_eq!(f.count(),0);
        if mode==3{let packet=directory_request(&mut f,Arc::new(ImQuery::relations(None,-1).unwrap()));assert_eq!(packet.call_id,2);f.backend.retire_im_read(packet.call_id);}
        if mode==0||mode==3{assert!(f.scope.is_open());}
    }
}

#[test]
fn directory_exact_private_input_reservation_cannot_be_rebranded_to_foreign_original_scope(){
    let mut f=Fixture::with_directory(true,false,true);let foreign=Fixture::with_directory(true,false,true);
    let input=foreign.scope.reserve_native_rpc(37).unwrap();
    let query=Arc::new(ImQuery::relations(None,-1).unwrap().attach_directory_input(input.clone()).unwrap());
    assert!(query.directory_input_matches(&foreign.scope));assert!(!query.directory_input_matches(&f.scope));
    assert!(f.backend.prepare_im_read(query.clone(),f.gate.clone(),Instant::now()+Duration::from_secs(30)).is_err());
    assert_eq!(f.scope.native_rpc_resources(),(0,0));assert_eq!(foreign.scope.native_rpc_resources(),(1,37));
    let budget=f.scope.reserve_native_rpc(1).unwrap();
    assert!(n::rpc_im::ImRequest::original(f.backend.captured_im_binding().unwrap(),query.clone(),f.gate.clone(),Instant::now()+Duration::from_secs(30),2,budget.clone()).is_err());
    let local=Arc::new(ImQuery::group_properties(directory::GroupId::parse(b"1-2").unwrap()).unwrap().attach_directory_input(budget.clone()).unwrap());
    assert!(n::rpc_im::ImRequest::original(f.backend.captured_im_binding().unwrap(),local.clone(),f.gate.clone(),Instant::now()+Duration::from_secs(30),2,input.clone()).is_err());
    assert!(ImQuery::relations(None,-1).unwrap().attach_directory_input(budget.clone()).unwrap().attach_directory_input(budget.clone()).is_err());
    let packet=directory_request(&mut f,local.clone());assert_eq!(packet.call_id,2);assert!(f.scope.is_open());
    f.backend.retire_im_read(packet.call_id);drop(local);drop(budget);assert_eq!(f.scope.native_rpc_resources(),(0,0));
    drop(query);assert_eq!(foreign.scope.native_rpc_resources(),(1,37));drop(input);assert_eq!(foreign.scope.native_rpc_resources(),(0,0));
}

#[test]
fn directory_shared_128_entries_128kib_retained_reply_survives_take_retire_close_until_last_arc_drop(){
    assert_eq!((MAX_MAP_ENTRIES,MAX_STREAM_BYTES),(128,131072));
    let mut f=Fixture::with_directory(true,false,true);let input=f.scope.reserve_native_rpc(37).unwrap();
    let query=Arc::new(ImQuery::relations(None,-1).unwrap().attach_directory_input(input.clone()).unwrap());
    let packet=directory_request(&mut f,query.clone());let pending=f.scope.native_rpc_resources();assert_eq!(pending.0,2);
    let body=directory_relations(true,true,b"");directory_reply(&mut f,packet.call_id,0,&body);
    let charged=f.scope.native_rpc_resources();assert_eq!(charged,(2,pending.1+body.len()));
    let mut replies=f.backend.take_im_replies();assert_eq!(f.scope.native_rpc_resources(),charged);
    let reply=replies.pop().unwrap();let request=reply.request().clone();let last_request=request.clone();
    let mut fillers=Vec::new();for _ in 0..126{fillers.push(f.scope.reserve_native_rpc(1).unwrap());}
    assert_eq!(f.scope.native_rpc_resources(),(128,charged.1+126));
    assert!(f.backend.prepare_im_read(Arc::new(ImQuery::relations(None,-1).unwrap()),f.gate.clone(),Instant::now()+Duration::from_secs(30)).is_err());
    let removed=fillers.pop().unwrap();drop(removed);
    let next=directory_request(&mut f,Arc::new(ImQuery::relations(None,-1).unwrap()));assert_eq!(next.call_id,3);
    f.backend.retire_im_read(next.call_id);drop(fillers);assert_eq!(f.scope.native_rpc_resources(),charged);
    f.backend.retire_im_read(packet.call_id);f.gate.retire();f.scope.close();assert_eq!(f.scope.native_rpc_resources(),charged);
    drop(reply);drop(request);drop(replies);assert_eq!(f.scope.native_rpc_resources(),charged);
    drop(last_request);assert_eq!(f.scope.native_rpc_resources(),(1,37));drop(query);assert_eq!(f.scope.native_rpc_resources(),(1,37));
    drop(input);assert_eq!(f.scope.native_rpc_resources(),(0,0));

    let mut f=Fixture::with_directory(true,false,true);let full=f.scope.reserve_native_rpc(MAX_STREAM_BYTES).unwrap();
    assert!(f.backend.prepare_im_read(Arc::new(ImQuery::relations(None,-1).unwrap()),f.gate.clone(),Instant::now()+Duration::from_secs(30)).is_err());
    assert_eq!(f.scope.native_rpc_resources(),(1,131072));assert!(f.scope.is_open());drop(full);
    let packet=directory_request(&mut f,Arc::new(ImQuery::relations(None,-1).unwrap()));assert_eq!(packet.call_id,2);
    let used=f.scope.native_rpc_resources().1;let full=f.scope.reserve_native_rpc(MAX_STREAM_BYTES-used).unwrap();
    let raw=reply_packet(packet.call_id,&directory_relations(true,true,b""));
    assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&raw,1).is_err());assert!(f.backend.take_im_replies().is_empty());
    assert!(!f.scope.is_open());assert_eq!(f.scope.native_rpc_resources(),(1,MAX_STREAM_BYTES-used));
    drop(full);assert_eq!(f.scope.native_rpc_resources(),(0,0));assert_eq!(f.count(),0);
}

#[test]
fn directory_read_is_not_received_write_sms_completion_or_unknown_sql_history_resolution(){
    let mut f=Fixture::with_directory(true,false,true);
    // Build the original durable gap locally; do not borrow sibling helpers.
    let mut w=Writer::new();w.boolean(true).unwrap();w.u32(1).unwrap();w.bytes(b"1_2").unwrap();
    w.properties(&[RpcProperty{key:n::im_status::PEER_READ,value:b"0"},RpcProperty{key:n::im_status::LATEST,value:b"7"}]).unwrap();
    w.i64(1).unwrap();w.bytes(b"").unwrap();
    let refresh=directory_read(&mut f,Arc::new(ImQuery::refresh(&[],-1).unwrap()),w.finish().expose_to_transport());
    let mut cursor=n::im_inbox::CursorSession::from_refresh(f.run.clone(),f.gate.clone(),&refresh).unwrap();
    cursor.observe_refresh(&f.temp.database,&refresh).unwrap();let plan=cursor.next_gap(&f.temp.database).unwrap().unwrap();
    let mut w=Writer::new();w.boolean(true).unwrap();w.u32(0).unwrap();w.bytes(b"").unwrap();
    let gap=directory_read(&mut f,plan.query(),w.finish().expose_to_transport());let span=cursor.commit_gap(&f.temp.database,&plan,&gap).unwrap();
    let n::im_inbox::ReceivedAdmission::Fresh(permit)=n::im_inbox::begin_received(f.temp.database.clone(),&span,&n::im_receipt_policy::NativeImReceiptConfig{set_received:true}).unwrap()else{panic!("original received permit")};
    let write=f.backend.prepare_received_write(permit.clone(),Instant::now()+Duration::from_secs(30)).unwrap();
    assert!(permit.begin_write(write.call_id));assert!(permit.submitted());
    let policy=n::im_send_policy::NativeImSendConfig{send_text:true,send_info:true,auth_country_code:None};
    let n::im_inbox::PreparedImSend::Fresh(intent)=n::im_inbox::ImSendAction::original(f.temp.database.clone(),f.run.clone(),f.gate.clone(),f.backend.captured_im_binding().unwrap(),f.backend.captured_send_identity().unwrap(),b"1_2",n::im_inbox::ImSendMessage::text("unknown outbound").unwrap(),&"a".repeat(32),Instant::now()+Duration::from_secs(30),&policy).unwrap()else{panic!("original unknown outbox intent")};
    let packet=directory_request(&mut f,Arc::new(ImQuery::relations(None,-1).unwrap()));
    f.backend.retire_received_write(packet.call_id); // Wrong family must not retire this READ.
    let message=f.message(91,b"P2P/1_2",99,r#"{"text":"unconsumed"}"#);
    let ImReplySource::Direct(ticket)=message.ticket().source()else{panic!("original incoming ticket")};
    assert!(f.backend.complete_direct_message(ticket,true).is_err());
    f.backend.complete_im(message.ticket(),None).unwrap();assert_eq!(f.count(),0);
    directory_reply(&mut f,packet.call_id,0,&directory_relations(true,true,b""));
    let replies=f.backend.take_im_replies();assert_eq!(replies.len(),1);
    assert!(f.backend.take_received_replies().is_empty());assert!(f.backend.take_im_send_replies().is_empty());
    assert!(n::im_inbox::CursorSession::from_refresh(f.run.clone(),f.gate.clone(),&replies[0]).is_err());
    let state:(i64,Option<i32>,i64)=f.temp.database.with_conn(|c|c.query_row("SELECT state,request_id,target_index FROM app_voice_im_read_receipts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|"fixture")).unwrap();
    assert_eq!(state,(0,Some(write.call_id),7));
    let outbox:(i64,Option<i32>,Option<i64>)=f.temp.database.with_conn(|c|c.query_row("SELECT state,request_id,message_index FROM app_voice_im_outbox",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(|_|"fixture")).unwrap();
    assert_eq!(outbox,(0,None,None));assert!(f.temp.database.get_recent_app_events(Some(f.run.line_id()),10).unwrap().is_empty());
    // A received-WRITE bool body is not a directory result, even for its exact ID.
    let packet=directory_request(&mut f,Arc::new(ImQuery::group_properties(directory::GroupId::parse(b"1-2").unwrap()).unwrap()));
    let mut w=Writer::new();w.boolean(true).unwrap();w.bytes(b"").unwrap();
    let raw=reply_packet(packet.call_id,w.finish().expose_to_transport());
    assert!(f.backend.on_plaintext_packet(&f.owner,&f.scope,&raw,1).is_err());
    assert!(f.backend.take_im_replies().is_empty());assert!(f.backend.take_received_replies().is_empty());
    let state:i64=f.temp.database.with_conn(|c|c.query_row("SELECT state FROM app_voice_im_read_receipts",[],|r|r.get(0)).map_err(|_|"fixture")).unwrap();assert_eq!(state,0);
    drop(intent);permit.cancel();
}
