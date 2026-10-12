//! Framing and TLS regressions. Ephemeral test certificates are generated only
//! by the Actions test process; no private key is committed or used on a device.
use super::*;
use std::{path::PathBuf, process::{Command, Stdio}};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};
use tokio_rustls::{TlsAcceptor, rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer}};

fn sip(body: &[u8]) -> Vec<u8> {
    let mut frame = format!("MESSAGE sip:u@ims.example SIP/2.0\r\nCall-ID: test\r\nCSeq: 1 MESSAGE\r\nVia: SIP/2.0/TCP 127.0.0.1;branch=z9hG4bKx\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    frame.extend_from_slice(body); frame
}
fn extract(buffer: &mut Vec<u8>) -> Vec<u8> {
    match take_frame(buffer, false).unwrap().unwrap() { StreamFrame::Sip(frame) => frame, _ => panic!("SIP expected") }
}

#[test]
fn stream_framing_keeps_binary_bodies_and_pipelined_messages_separate() {
    let first = sip(&[0, 1, 255]); let second = sip(b"next");
    let mut buffer = first.clone(); buffer.extend_from_slice(&second);
    assert_eq!(extract(&mut buffer), first); assert_eq!(extract(&mut buffer), second); assert!(buffer.is_empty());
    for size in 0..first.len() { assert!(take_frame(&mut first[..size].to_vec(), false).unwrap().is_none()); }
}

#[test]
fn stream_framing_rejects_missing_duplicate_oversized_and_malformed_lengths() {
    let original = String::from_utf8(sip(b"")).unwrap();
    for replacement in ["Content-Length: -1", "Content-Length: +1", "Content-Length: 18446744073709551616",
        "Content-Length: 1048577", "Content-Length: 0\r\nContent-Length: 0", "Content-Length: 0\r\nl: 0", "X-No-Length: 0"] {
        let mut bytes = original.replace("Content-Length: 0", replacement).into_bytes();
        assert_eq!(take_frame(&mut bytes, false).err().unwrap().code(), code::CHANNEL_STREAM_FRAME_INVALID);
    }
    assert!(take_frame(&mut vec![b'A'; MAX_HEADER + 1], false).is_err());
}

#[test]
fn fragmented_crlf_ping_is_not_mistaken_for_unsolicited_pong() {
    let mut buffer = b"\r\n".to_vec();
    assert!(take_frame(&mut buffer, false).unwrap().is_none());
    buffer.extend_from_slice(b"\r\n");
    assert!(matches!(take_frame(&mut buffer, false).unwrap(), Some(StreamFrame::Ping)));
    let mut pong = b"\r\n".to_vec();
    assert!(matches!(take_frame(&mut pong, true).unwrap(), Some(StreamFrame::Pong)));
}

#[tokio::test]
async fn stream_partial_reads_survive_cancellation_and_eof_poisoning_is_terminal() {
    let (client, mut peer) = tokio::io::duplex(4096);
    let mut stream = SipStream::new(Box::new(client), "127.0.0.1:5060".parse().unwrap());
    let frame = sip(b"payload");
    peer.write_all(&frame[..30]).await.unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(10), stream.receive(false)).await.is_err());
    peer.write_all(&frame[30..]).await.unwrap();
    assert!(matches!(stream.receive(false).await.unwrap(), StreamFrame::Sip(received) if received == frame));
    drop(peer);
    assert_eq!(stream.receive(false).await.err().unwrap().code(), code::CHANNEL_STREAM_CLOSED);
    assert!(!stream.is_usable());
}

#[tokio::test]
async fn cancelled_partial_write_cannot_be_reused_for_another_request() {
    let (client, _peer) = tokio::io::duplex(8);
    let mut stream = SipStream::new(Box::new(client), "127.0.0.1:5060".parse().unwrap());
    assert!(tokio::time::timeout(Duration::from_millis(10), stream.send(&sip(b"payload"))).await.is_err());
    assert_eq!(stream.send(&sip(b"next")).await.unwrap_err().code(), code::CHANNEL_STREAM_CLOSED);
}

#[test]
fn tcp_responses_stay_on_the_connection_that_received_each_request() {
    let first = sip(b""); let other = String::from_utf8(first.clone()).unwrap().replace("Call-ID: test", "Call-ID: other").into_bytes();
    let response = |request: &[u8], status| format!("SIP/2.0 {status} Test\r\nCall-ID: {}\r\nCSeq: {}\r\nVia: {}\r\nContent-Length: 0\r\n\r\n",
        sip_frame::header_value(request, "Call-ID").unwrap(), sip_frame::header_value(request, "CSeq").unwrap(), sip_frame::header_value(request, "Via").unwrap()).into_bytes();
    let mut routes = ResponseRoutes::default();
    routes.received(&first, 10).unwrap(); routes.received(&other, 20).unwrap();
    assert_eq!(routes.response_connection(&response(&first, 100)).unwrap(), Some(10));
    assert_eq!(routes.response_connection(&response(&other, 200)).unwrap(), Some(20));
    assert_eq!(routes.response_connection(&response(&first, 200)).unwrap(), Some(10));
    assert!(routes.response_connection(&response(&first, 200)).is_err());
    assert_eq!(routes.response_connection(&first).unwrap(), None);
}

#[test]
fn tcp_response_routing_ignores_added_received_rport_and_ack_does_not_allocate() {
    let request = sip(b"");
    let response = format!("SIP/2.0 200 OK\r\nVia: {};received=127.0.0.1;rport=5060\r\nCall-ID: test\r\nCSeq: 1 MESSAGE\r\nContent-Length: 0\r\n\r\n",
        sip_frame::header_value(&request, "Via").unwrap());
    let mut routes = ResponseRoutes::default();
    routes.received(&request, 11).unwrap();
    assert!(routes.received(&request, 22).is_err(), "different connection cannot steal a live request");
    assert_eq!(routes.response_connection(response.as_bytes()).unwrap(), Some(11));
    for n in 0..100 {
        let ack = format!("ACK sip:u@ims.example SIP/2.0\r\nCall-ID: {n}\r\nCSeq: 1 ACK\r\nVia: SIP/2.0/TCP 127.0.0.1;branch=z9hG4bK{n}\r\nContent-Length: 0\r\n\r\n");
        routes.received(ack.as_bytes(), 11).unwrap();
    }
    assert!(routes.0.is_empty());
}

struct Certificates { dir: PathBuf, roots: rustls::RootCertStore, server: Arc<rustls::ServerConfig> }
impl Drop for Certificates { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); } }
fn certificates() -> Certificates { certificates_for("DNS:ims.example") }
fn certificates_for(san: &str) -> Certificates {
    let dir = std::env::temp_dir().join(format!("simadmin-tls-test-{}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    std::fs::create_dir(&dir).unwrap();
    let run = |args: &[&str]| {
        assert!(Command::new("openssl").args(args).current_dir(&dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .status().expect("Actions openssl").success(), "ephemeral test certificate generation");
    };
    run(&["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "key.pem", "-out", "cert.pem", "-days", "1", "-subj", "/CN=ims.example",
        "-addext", &format!("subjectAltName={san}"), "-addext", "basicConstraints=critical,CA:FALSE", "-addext", "keyUsage=critical,digitalSignature,keyEncipherment", "-addext", "extendedKeyUsage=serverAuth"]);
    run(&["x509", "-in", "cert.pem", "-outform", "DER", "-out", "cert.der"]);
    run(&["pkcs8", "-topk8", "-nocrypt", "-in", "key.pem", "-outform", "DER", "-out", "key.der"]);
    let cert = CertificateDer::from(std::fs::read(dir.join("cert.der")).unwrap());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(std::fs::read(dir.join("key.der")).unwrap()));
    let mut roots = rustls::RootCertStore::empty(); roots.add(cert.clone()).unwrap();
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(vec![cert], key).unwrap();
    Certificates { dir, roots, server: Arc::new(server) }
}

#[tokio::test]
async fn tls_validates_original_authority_and_transports_framed_sip() {
    let cert = certificates(); let listener = TcpListener::bind("127.0.0.1:0").await.unwrap(); let remote = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(cert.server.clone()); let expected = sip(b"TLS body"); let sent = expected.clone();
    let peer = tokio::spawn(async move { let (socket, _) = listener.accept().await.unwrap(); let local = socket.local_addr().unwrap();
        let tls = acceptor.accept(socket).await.unwrap(); let mut stream = SipStream::new(Box::new(tls), local);
        assert!(matches!(stream.receive(false).await.unwrap(), StreamFrame::Sip(frame) if frame == expected));
        stream.send(&sip(b"reply")).await.unwrap(); });
    let socket = TcpStream::connect(remote).await.unwrap();
    let mut stream = SipStream::tls_with_roots(socket, "ims.example", Duration::from_secs(3), cert.roots.clone()).await.unwrap();
    stream.send(&sent).await.unwrap(); assert!(matches!(stream.receive(false).await.unwrap(), StreamFrame::Sip(frame) if frame == sip(b"reply")));
    peer.await.unwrap();
}

#[tokio::test]
async fn tls_rejects_wrong_name_untrusted_chain_and_ip_without_ip_san() {
    let cert = certificates();
    for (name, trusted) in [("other.example", true), ("ims.example", false), ("127.0.0.1", true)] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap(); let remote = listener.local_addr().unwrap();
        let acceptor = TlsAcceptor::from(cert.server.clone());
        let peer = tokio::spawn(async move { let (socket, _) = listener.accept().await.unwrap(); let _ = acceptor.accept(socket).await; });
        let socket = TcpStream::connect(remote).await.unwrap();
        let roots = if trusted { cert.roots.clone() } else { rustls::RootCertStore::empty() };
        assert_eq!(SipStream::tls_with_roots(socket, name, Duration::from_secs(3), roots).await.err().unwrap().code(), code::TLS_HANDSHAKE_FAILED);
        peer.await.unwrap();
    }
}

#[tokio::test]
async fn tls_rejects_wildcard_domain_even_when_https_name_matching_would_accept_it() {
    let cert = certificates_for("DNS:*.ims.example");
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap(); let remote = listener.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(cert.server.clone());
    let peer = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let _ = acceptor.accept(socket).await;
    });
    let socket = TcpStream::connect(remote).await.unwrap();
    assert_eq!(SipStream::tls_with_roots(socket, "pcscf.ims.example", Duration::from_secs(3), cert.roots.clone())
        .await.err().unwrap().code(), code::TLS_HANDSHAKE_FAILED);
    peer.await.unwrap();
}

#[tokio::test]
async fn tls_handshake_timeout_never_sends_a_plaintext_sip_fallback() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap(); let remote = listener.local_addr().unwrap();
    let peer = tokio::spawn(async move { let (mut socket, _) = listener.accept().await.unwrap(); let mut first = [0; 5]; socket.read_exact(&mut first).await.unwrap();
        assert_eq!(first[0], 22, "TLS ClientHello, not REGISTER"); tokio::time::sleep(Duration::from_millis(150)).await; });
    let socket = TcpStream::connect(remote).await.unwrap();
    assert_eq!(SipStream::tls(socket, "ims.example", Duration::from_millis(30)).await.err().unwrap().code(), code::TLS_HANDSHAKE_TIMEOUT);
    peer.await.unwrap();
}
