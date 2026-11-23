//! Original Actions-only in-memory TLS exchanges; no target keys or live Provider.
use super::*;
use crate::connectivity::app_voice::providers::jegotrip::tls_failure_detail::TlsFailureReason as R;
use base64::Engine;
use DiagnosticCertificates as Certificates;

/// New diagnostics-only fixture. The original certificate generator and every
/// old test remain unchanged. Different keys must also have different issuer
/// subjects to exercise UnknownIssuer instead of real signature rejection.
pub(crate) struct DiagnosticCertificates(super::Certificates);
impl std::ops::Deref for DiagnosticCertificates {
    type Target=super::Certificates;
    fn deref(&self)->&Self::Target{&self.0}
}
impl DiagnosticCertificates {
    pub(crate) fn generate(san:&str)->Self{
        let mut original=super::Certificates::generate(san);
        let subject=format!("/CN={}",original.directory.file_name().unwrap().to_str().unwrap());
        let run=|arguments:&[&str]|{
            assert!(std::process::Command::new("openssl").args(arguments).current_dir(&original.directory)
                .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().expect("Actions openssl").success());
        };
        run(&["req","-x509","-key","key.pem","-out","cert.pem","-days","1","-subj",&subject,
            "-addext",&format!("subjectAltName={san}"),"-addext","basicConstraints=critical,CA:FALSE",
            "-addext","keyUsage=critical,digitalSignature,keyEncipherment","-addext","extendedKeyUsage=serverAuth"]);
        run(&["x509","-in","cert.pem","-outform","DER","-out","cert.der"]);
        let der=std::fs::read(original.directory.join("cert.der")).unwrap();
        original.root=STANDARD.encode(&der);original.certificate=CertificateDer::from(der);Self(original)
    }
}

fn carrier_failure(cert:&Certificates,authority:&str,root:&str,reason:R){
    let original=owner();let mut protocol=ArcProtocol::new(original.clone());let scope=protocol.connection_scope();let now=Instant::now();
    let mut engine=protocol.take_tls_engine(PacketRoute::DirectTlsCarrier,parameters(authority,root),now).unwrap();let mut server=cert.server(&rustls::version::TLS13);
    assert_eq!(engine.encrypt_packets(&original,&scope,&encode_call_packets(CallKind::Ping,&[],TLS_FRAGMENT_THRESHOLD).unwrap(),None,now).err(),Some(TlsError::NotReady));
    assert_eq!(handshake(&mut engine,&original,&scope,&mut server,now),Err(TlsError::HandshakeFailed));
    assert_eq!(scope.tls_detail_for(&original),Some(reason));assert_eq!(engine.phase(),TlsPhase::Closed);assert_eq!(protocol.phase(),ArcPhase::Closed);assert!(server_plaintext(&mut server).is_empty());
    assert!(scope.tls_detail_for(&owner()).is_none());assert!(RpcConnectionScope::fresh().tls_detail_for(&original).is_none());assert_eq!(scope.native_rpc_resources(),(0,0));
}
fn endpoint_failure(cert:&Certificates,authority:&str,root:&str,reason:R){
    let original=owner();let scope=RpcConnectionScope::fresh();let now=Instant::now();let mut engine=EndpointTlsEngine::for_original(original.clone(),scope.clone(),parameters(authority,root),now).unwrap();let mut server=cert.server(&rustls::version::TLS13);
    assert_eq!(engine.encrypt_packets(&original,&scope,&[TransportBytes::from_bytes(vec![7])],now).err(),Some(TlsError::NotReady));
    assert_eq!(endpoint_handshake(&mut engine,&original,&scope,&mut server,now),Err(TlsError::HandshakeFailed));assert_eq!(scope.tls_detail_for(&original),Some(reason));
    assert_eq!(engine.phase(),TlsPhase::Closed);assert!(!scope.is_open());assert!(server_plaintext(&mut server).is_empty());assert!(scope.tls_detail_for(&owner()).is_none());
}
#[test]
fn carrier_tls_detail_separates_real_chain_name_ip_and_exact_san_rejections(){
    let cert=Certificates::generate("DNS:fixture.invalid");let other=Certificates::generate("DNS:fixture.invalid");
    carrier_failure(&cert,"other.invalid",&cert.root,R::NameMismatch);carrier_failure(&cert,"127.0.0.1",&cert.root,R::NameMismatch);carrier_failure(&cert,"fixture.invalid",&other.root,R::UnknownIssuer);
    let wildcard=Certificates::generate("DNS:*.fixture.invalid");carrier_failure(&wildcard,"host.fixture.invalid",&wildcard.root,R::ExactNameMissing);
}
#[test]
fn endpoint_router_tls_detail_separates_real_chain_name_ip_and_exact_san_rejections(){
    let cert=Certificates::generate("DNS:fixture.invalid");let other=Certificates::generate("DNS:fixture.invalid");
    endpoint_failure(&cert,"other.invalid",&cert.root,R::NameMismatch);endpoint_failure(&cert,"127.0.0.1",&cert.root,R::NameMismatch);endpoint_failure(&cert,"fixture.invalid",&other.root,R::UnknownIssuer);
    let wildcard=Certificates::generate("DNS:*.fixture.invalid");endpoint_failure(&wildcard,"host.fixture.invalid",&wildcard.root,R::ExactNameMissing);
}
#[test]
fn carrier_tls_detail_records_malformed_split_cipher_only_after_original_header_validation(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let mut protocol=ArcProtocol::new(original.clone());let scope=protocol.connection_scope();let now=Instant::now();let mut engine=protocol.take_tls_engine(PacketRoute::DirectTlsCarrier,parameters("fixture.invalid",&cert.root),now).unwrap();
    let first=frame_cipher(&[0xff,3,3,0],512);assert!(engine.on_wire_bytes(&original,&scope,&first,None,now).unwrap().unwrap().plaintext.packets_for(&original,&scope).unwrap().is_empty());assert_eq!(scope.tls_detail_for(&original),None);
    assert_eq!(engine.on_wire_bytes(&original,&scope,&frame_cipher(&[1,0],512),None,now).err(),Some(TlsError::HandshakeFailed));assert_eq!(scope.tls_detail_for(&original),Some(R::MalformedRecord));assert!(!scope.is_open());
    assert!(engine.on_wire_bytes(&original,&scope,&[],None,now).unwrap().is_none());assert_eq!(scope.tls_detail_for(&original),Some(R::MalformedRecord));
}
#[test]
fn endpoint_router_tls_detail_records_malformed_split_cipher_without_plaintext_or_retry(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let scope=RpcConnectionScope::fresh();let now=Instant::now();let mut engine=EndpointTlsEngine::for_original(original.clone(),scope.clone(),parameters("fixture.invalid",&cert.root),now).unwrap();
    assert!(engine.on_wire_bytes(&original,&scope,&[0xff,3,3,0],now).unwrap().unwrap().plaintext.packets_for(&original,&scope).unwrap().is_empty());assert_eq!(scope.tls_detail_for(&original),None);
    assert_eq!(engine.on_wire_bytes(&original,&scope,&[1,0],now).err(),Some(TlsError::HandshakeFailed));assert_eq!(scope.tls_detail_for(&original),Some(R::MalformedRecord));assert!(!scope.is_open());
    assert!(engine.on_wire_bytes(&original,&scope,&[21,3,3,0,2,2,40],now).unwrap().is_none());assert_eq!(scope.tls_detail_for(&original),Some(R::MalformedRecord));
}
#[test]
fn carrier_tls_detail_real_peer_alert_and_invalid_handshake_are_not_raw_error_strings(){
    let cert=Certificates::generate("DNS:fixture.invalid");
    for(raw,reason)in [(&[21,3,3,0,2,2,40][..],R::PeerAlert),(&[22,3,3,0,4,0xff,0,0,0][..],R::PeerProtocol)]{
        let original=owner();let mut protocol=ArcProtocol::new(original.clone());let scope=protocol.connection_scope();let now=Instant::now();let mut engine=protocol.take_tls_engine(PacketRoute::DirectTlsCarrier,parameters("fixture.invalid",&cert.root),now).unwrap();engine.take_wire_output(&original,&scope,None,now).unwrap();
        assert_eq!(engine.on_wire_bytes(&original,&scope,&frame_cipher(raw,512),None,now).err(),Some(TlsError::HandshakeFailed));assert_eq!(scope.tls_detail_for(&original),Some(reason));assert_eq!(engine.phase(),TlsPhase::Closed);
    }
}
#[test]
fn endpoint_router_tls_detail_real_peer_alert_and_invalid_handshake_keep_original_rejection(){
    let cert=Certificates::generate("DNS:fixture.invalid");
    for(raw,reason)in [(&[21,3,3,0,2,2,40][..],R::PeerAlert),(&[22,3,3,0,4,0xff,0,0,0][..],R::PeerProtocol)]{
        let original=owner();let scope=RpcConnectionScope::fresh();let now=Instant::now();let mut engine=EndpointTlsEngine::for_original(original.clone(),scope.clone(),parameters("fixture.invalid",&cert.root),now).unwrap();engine.take_wire_output(&original,&scope,now).unwrap();
        assert_eq!(engine.on_wire_bytes(&original,&scope,raw,now).err(),Some(TlsError::HandshakeFailed));assert_eq!(scope.tls_detail_for(&original),Some(reason));assert!(!scope.is_open());
    }
}
#[test]
fn carrier_tls_detail_corrupted_authenticated_cipher_never_delivers_any_step_plaintext(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(protocol,original,scope,mut engine,mut server,now)=setup(&cert,&rustls::version::TLS13);assert_eq!(scope.tls_detail_for(&original),None);
    server.writer().write_all(&[2]).unwrap();let mut cipher=server_output(&mut server);*cipher.last_mut().unwrap()^=1;
    assert_eq!(engine.on_wire_bytes(&original,&scope,&frame_cipher(&cipher,512),None,now).err(),Some(TlsError::HandshakeFailed));assert_eq!(scope.tls_detail_for(&original),Some(R::DecryptionFailed));assert_eq!(protocol.phase(),ArcPhase::Closed);
}
#[test]
fn endpoint_router_tls_detail_corrupted_authenticated_cipher_closes_without_endpoint_packets(){
    let cert=Certificates::generate("DNS:fixture.invalid");let(protocol,original,scope,mut engine,mut server,now)=endpoint_setup(&cert,&rustls::version::TLS13);assert_eq!(scope.tls_detail_for(&original),None);
    server.writer().write_all(encode_packet_stream_frame(&[7]).unwrap().expose_to_transport()).unwrap();let mut cipher=server_output(&mut server);*cipher.last_mut().unwrap()^=1;
    assert_eq!(engine.on_wire_bytes(&original,&scope,&cipher,now).err(),Some(TlsError::HandshakeFailed));assert_eq!(scope.tls_detail_for(&original),Some(R::DecryptionFailed));assert_eq!(protocol.phase(),ArcPhase::Closed);
}
#[test]
fn carrier_tls_detail_stale_owner_scope_fixed_deadline_and_last_drop_do_not_fabricate_category(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let mut protocol=ArcProtocol::new(original.clone());let scope=protocol.connection_scope();let now=Instant::now();let mut engine=protocol.take_tls_engine(PacketRoute::DirectTlsCarrier,parameters("fixture.invalid",&cert.root),now).unwrap();let reservation=scope.reserve_native_rpc(100).unwrap();let last=reservation.clone();
    let malformed=frame_cipher(&[0xff,3,3,0,1,0],512);assert!(engine.on_wire_bytes(&owner(),&scope,&malformed,None,now-Duration::from_secs(10)).unwrap().is_none());assert!(engine.on_wire_bytes(&original,&RpcConnectionScope::fresh(),&malformed,None,now).unwrap().is_none());assert_eq!(scope.tls_detail_for(&original),None);
    engine.on_wire_bytes(&original,&scope,&frame_cipher(&[22,3,3,0],512),None,now).unwrap();engine.on_wire_bytes(&original,&scope,&frame_cipher(&[1],512),None,now+Duration::from_millis(999)).unwrap();
    assert_eq!(engine.tick(&original,&scope,now+Duration::from_secs(1)),Err(TlsError::Timeout));assert_eq!(scope.tls_detail_for(&original),None);assert_eq!(scope.native_rpc_resources(),(1,100));drop(reservation);assert_eq!(scope.native_rpc_resources(),(1,100));drop(last);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn endpoint_router_tls_detail_stale_owner_scope_fixed_deadline_and_last_drop_do_not_fabricate_category(){
    let cert=Certificates::generate("DNS:fixture.invalid");let original=owner();let scope=RpcConnectionScope::fresh();let now=Instant::now();let mut engine=EndpointTlsEngine::for_original(original.clone(),scope.clone(),parameters("fixture.invalid",&cert.root),now).unwrap();let reservation=scope.reserve_native_rpc(100).unwrap();let last=reservation.clone();
    assert!(engine.on_wire_bytes(&owner(),&scope,&[0xff,3,3,0,1,0],now-Duration::from_secs(10)).unwrap().is_none());assert!(engine.on_wire_bytes(&original,&RpcConnectionScope::fresh(),&[0xff,3,3,0,1,0],now).unwrap().is_none());assert_eq!(scope.tls_detail_for(&original),None);
    engine.on_wire_bytes(&original,&scope,&[22,3,3,0],now).unwrap();engine.on_wire_bytes(&original,&scope,&[1],now+Duration::from_millis(999)).unwrap();assert_eq!(engine.tick(&original,&scope,now+Duration::from_secs(1)),Err(TlsError::Timeout));
    assert_eq!(scope.tls_detail_for(&original),None);assert_eq!(scope.native_rpc_resources(),(1,100));drop(reservation);assert_eq!(scope.native_rpc_resources(),(1,100));drop(last);assert_eq!(scope.native_rpc_resources(),(0,0));
}
#[test]
fn carrier_tls_detail_tls12_tls13_success_sni_and_drop_never_claim_a_failure_or_sdk_grant(){
    let cert=Certificates::generate("DNS:fixture.invalid");
    for version in [&rustls::version::TLS12,&rustls::version::TLS13]{let(protocol,original,scope,engine,server,_)=setup(&cert,version);assert_eq!(scope.tls_detail_for(&original),None);assert_eq!(engine.phase(),TlsPhase::Verified);assert_eq!(server.server_name(),Some("fixture.invalid"));assert_eq!(protocol.phase(),ArcPhase::Initial);drop(engine);assert!(!scope.is_open());assert_eq!(scope.tls_detail_for(&original),None);}
}
#[test]
fn endpoint_router_tls_detail_tls12_tls13_success_sni_and_drop_never_claim_a_failure_or_sdk_grant(){
    let cert=Certificates::generate("DNS:fixture.invalid");
    for version in [&rustls::version::TLS12,&rustls::version::TLS13]{let(protocol,original,scope,engine,server,_)=endpoint_setup(&cert,version);assert_eq!(scope.tls_detail_for(&original),None);assert_eq!(engine.phase(),TlsPhase::Verified);assert_eq!(server.server_name(),Some("fixture.invalid"));assert_eq!(protocol.phase(),ArcPhase::Initial);drop(engine);assert!(!scope.is_open());assert_eq!(scope.tls_detail_for(&original),None);}
}
#[test]
fn carrier_tls_detail_same_issuer_wrong_key_is_real_bad_signature_not_unknown_issuer(){
    let cert=DiagnosticCertificates(super::Certificates::generate("DNS:fixture.invalid"));let other=DiagnosticCertificates(super::Certificates::generate("DNS:fixture.invalid"));
    carrier_failure(&cert,"fixture.invalid",&other.root,R::BadSignature);
}
#[test]
fn endpoint_router_tls_detail_same_issuer_wrong_key_is_real_bad_signature_not_unknown_issuer(){
    let cert=DiagnosticCertificates(super::Certificates::generate("DNS:fixture.invalid"));let other=DiagnosticCertificates(super::Certificates::generate("DNS:fixture.invalid"));
    endpoint_failure(&cert,"fixture.invalid",&other.root,R::BadSignature);
}
