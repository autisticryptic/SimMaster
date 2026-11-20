//! Actions-only exact decoded factory-input ownership; canonical DATA API stays unchanged.
use super::*;
#[test]
fn factory_input_keeps_option_order_port_spelling_outer_whitespace_and_semicolon_presence(){
    for raw in [b"\t path/#entry:tcp -p 00443 -h fixture.invalid \r\n".as_slice(),b"path/#entry:tcp -h fixture.invalid -p 443;"]{
        let r=parse_object_reference(raw).unwrap();assert_eq!(r.original_factory_input(),raw);
        assert_eq!(r.encode_native().unwrap().expose_to_backend(),b"path/#entry:tcp -h fixture.invalid -p 443;");
        assert_eq!(r.direct_raw_tcp().unwrap()[0].port(),443);assert_eq!(r.identity(),b"path/#entry");
    }
}
#[test]
fn factory_input_preserves_adapter_locator_grouped_dollars_and_identity_components_without_routes(){
    for raw in [b" facet/[uid:7_9@fixture.invalid]@{[a$b]}$<locator:{c$d}> ".as_slice(),b"category/entry",b"entry@adapter"]{
        let r=parse_object_reference(raw).unwrap();assert_eq!(r.original_factory_input(),raw);
        assert_eq!(r.direct_raw_tcp().err(),Some(ReferenceError::NoDirectEndpoints));assert!(!r.components().is_empty());
        assert!(!format!("{r:?}").contains("entry"));
    }
}
#[test]
fn factory_input_is_owned_not_borrowed_from_mutable_caller_and_accounting_does_not_encode(){
    let mut input=b"id:tcp -p 00443 -h fixture.invalid".to_vec();let r=parse_object_reference(&input).unwrap();
    let expected=input.clone();input.fill(b'x');assert_eq!(r.original_factory_input(),expected);
    let parsed=2*b"id".len()+b"tcp".len()+b"fixture.invalid".len();assert_eq!(r.retained_bytes(),expected.len()+parsed);
    assert_eq!(r.retained_bytes(),r.retained_bytes());assert_eq!(r.original_factory_input(),expected);
}
#[test]
fn factory_input_preserves_each_endpoint_index_and_router_literal_without_authority_inference(){
    let raw=b"Router:tls -ri 0145 -p 00443 -h first.invalid;tls -p 00444 -h second.invalid -ri +202";
    let r=parse_object_reference(raw).unwrap();assert_eq!(r.original_factory_input(),raw);
    let ReferenceTarget::Direct(endpoints)=r.target()else{panic!("direct DATA")};assert_eq!(endpoints.len(),2);
    assert_eq!((endpoints[0].host(),endpoints[0].port(),endpoints[0].router_id()),(b"first.invalid".as_slice(),443,Some(101)));
    assert_eq!((endpoints[1].host(),endpoints[1].port(),endpoints[1].router_id()),(b"second.invalid".as_slice(),444,Some(202)));
    assert!(r.direct_raw_tcp().is_err());assert_eq!(r.explicit_tls_router_groups().unwrap().len(),2);
}
#[test]
fn factory_input_retained_bytes_charge_raw_and_all_parsed_payload_for_each_target_kind(){
    for raw in [b"a/b".as_slice(),b"a/b@adapter$locator",b"a/b:tcp -h fixture.invalid -p 1;"]{
        let r=parse_object_reference(raw).unwrap();let target=match r.target(){ReferenceTarget::IdentityOnly=>0,ReferenceTarget::Adapter{adapter,locator}=>adapter.len()+locator.as_ref().map_or(0,Vec::len),ReferenceTarget::Direct(e)=>e.iter().map(|p|p.scheme().len()+p.host().len()).sum()};
        assert_eq!(r.retained_bytes(),raw.len()+r.identity().len()+r.components().iter().map(Vec::len).sum::<usize>()+target);
        assert!(r.retained_bytes()>=raw.len());assert_eq!(r.original_factory_input(),raw);
    }
}
#[test]
fn factory_input_retention_does_not_accept_invalid_overbound_or_unimplemented_reference_shapes(){
    for raw in [b"id\0hidden".as_slice(),b"id@a$l$x",b"id:tcp -h fixture.invalid -p 443 -unknown value;",b"id:tcp -h fixture.invalid -p 65536;",b"[unclosed"]{assert!(parse_object_reference(raw).is_err());}
    assert_eq!(parse_object_reference(&vec![b'x';MAX_FIELD_BYTES+1]).err(),Some(ReferenceError::TooLarge));
}
