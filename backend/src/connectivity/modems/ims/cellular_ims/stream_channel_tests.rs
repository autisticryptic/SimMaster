//! Real loopback TCP channels, reserved ports and reliable REGISTER driver.
//! Actions only. Worker configuration and XFRM execution remain synthetic.
use super::*;
use crate::connectivity::core::{access::ImsChannel, register::{run_register_observed, RegisterAuthenticator}, sip_frame};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn worker() -> UeWorkerBinding {
    static IDS: AtomicUsize = AtomicUsize::new(0);
    let line = format!("n6-stream-test-{}", IDS.fetch_add(1, Ordering::Relaxed));
    let namespace = crate::platform::netns::NetnsName::for_line(crate::platform::netns::DEFAULT_NAMESPACE_PREFIX, &line);
    let worker = UeWorkerHandle::for_line(&line, namespace);
    worker.enable_test_net_config().await;
    worker.bind()
}
async fn fixture() -> (CellularStreamTransport, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let target = listener.local_addr().unwrap(); let client = TcpStream::connect(target).await.unwrap();
    let (server, _) = listener.accept().await.unwrap();
    let route = ImsRoute { local_addr: client.local_addr().unwrap(), pcscf_addr: target, transport: SipTransport::Tcp };
    (CellularStreamTransport::from_tcp(client, route, worker().await).unwrap(), server)
}
async fn read_frame(socket: &mut TcpStream) -> Vec<u8> {
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut frame = Vec::new();
        loop {
            if sip_frame::complete_frame_len(&frame).is_some_and(|len| len == frame.len()) { return frame; }
            let mut byte = [0]; socket.read_exact(&mut byte).await.unwrap(); frame.push(byte[0]);
        }
    }).await.unwrap()
}
fn request(id: &str, method: &str, contact_port: u16) -> Vec<u8> {
    format!("{method} sip:ims.example SIP/2.0\r\nVia: SIP/2.0/TCP 127.0.0.1;branch=z9hG4bK{id}\r\nFrom: <sip:u@ims.example>;tag=local\r\nTo: <sip:u@ims.example>\r\nCall-ID: {id}\r\nCSeq: 1 {method}\r\nContact: <sip:u@127.0.0.1:{contact_port};transport=tcp>\r\nExpires: 600\r\nContent-Length: 0\r\n\r\n").into_bytes()
}
fn response(request: &[u8], status: u16) -> Vec<u8> {
    format!("SIP/2.0 {status} Test\r\nVia: {}\r\nCall-ID: {}\r\nCSeq: {}\r\nContact: {}\r\nExpires: 600\r\nContent-Length: 0\r\n\r\n",
        sip_frame::header_value(request,"Via").unwrap(),sip_frame::header_value(request,"Call-ID").unwrap(),
        sip_frame::header_value(request,"CSeq").unwrap(),sip_frame::header_value(request,"Contact").unwrap()).into_bytes()
}

#[tokio::test]
async fn reliable_register_uses_tcp_without_udp_retransmissions_and_keeps_binding() {
    let (stream, mut peer) = fixture().await;
    let mut channel = super::super::CellularImsSipChannel { stream: Some(stream), send_socket: None, receive_socket: None,
        reserved_send_socket: None, reserved_receive_socket: None, requeued: ImsRequeue::default(), outbound: OutboundFlow::default(),
        route: ImsRoute { local_addr: "127.0.0.1:0".parse().unwrap(), pcscf_addr: peer.local_addr().unwrap(), transport: SipTransport::Tcp },
        advertised_local_port: None, interface: Some("lo".into()), security_verify: None, staged_security: None, retired_security: None };
    channel.configure_outbound("n6-tcp-register", "urn:uuid:00000000-0000-4000-8000-000000000022", true);
    let initial = request("reg", "REGISTER", channel.route().local_addr.port());
    struct Auth;
    impl RegisterAuthenticator<super::super::CellularImsSipChannel> for Auth {
        async fn authenticated_request(&mut self, _: &[u8], _: u32) -> Result<Vec<u8>, ImsError> { Err(ImsError::new("unexpected_test_challenge")) }
    }
    let server = tokio::spawn(async move {
        let frame = read_frame(&mut peer).await;
        assert!(String::from_utf8_lossy(&frame).contains("SIP/2.0/TCP"));
        let mut extra = [0; 1];
        assert!(tokio::time::timeout(Duration::from_millis(600), peer.read(&mut extra)).await.is_err(), "no UDP Timer E on reliable TCP");
        peer.write_all(&response(&frame, 200)).await.unwrap();
    });
    let result = run_register_observed(&mut channel, &initial, &mut Auth).await.unwrap();
    channel.outbound_registered(&result.response, 600).unwrap();
    assert!(channel.reg_event_binding().is_some());
    assert_eq!(channel.route().transport, SipTransport::Tcp);
    server.await.unwrap();
}

#[tokio::test]
async fn tcp_security_activation_consumes_exact_reserved_ports_and_routes_server_responses() {
    let (mut stream, _old_peer) = fixture().await;
    let (client_port, server_port) = stream.reserve_for_test().await;
    assert!(TcpStream::connect(SocketAddr::from(([127,0,0,1], server_port))).await.is_err(),
        "port_us must be reserved but not accepting plaintext before SA activation");
    assert_eq!(stream.reserve_send(client_port, server_port).await.unwrap(), client_port);
    assert_eq!(stream.reserve_receive(server_port).await.unwrap(), server_port);
    let remote_server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let remote_client = TcpSocket::new_v4().unwrap(); remote_client.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let peer_client_addr = remote_client.local_addr().unwrap();
    let route = ImsRoute { local_addr: SocketAddr::from(([127,0,0,1], client_port)), pcscf_addr: remote_server.local_addr().unwrap(), transport: SipTransport::Tcp };
    stream.activate(route, SocketAddr::from(([127,0,0,1], server_port)), peer_client_addr, Some("verified-sa".into())).await.unwrap();
    let (mut server, source) = remote_server.accept().await.unwrap(); assert_eq!(source.port(), client_port);
    assert_eq!(stream.route().local_addr.port(), server_port);
    assert_eq!(stream.send_route().local_addr.port(), client_port);
    let mut incoming = remote_client.connect(SocketAddr::from(([127,0,0,1], server_port))).await.unwrap();
    let notify = request("incoming", "NOTIFY", server_port); incoming.write_all(&notify).await.unwrap();
    assert_eq!(stream.receive(Duration::from_secs(2), true).await.unwrap(), notify);
    stream.send(&response(&notify, 200)).await.unwrap();
    assert!(read_frame(&mut incoming).await.starts_with(b"SIP/2.0 200"));
    assert!(tokio::time::timeout(Duration::from_millis(30), server.read(&mut [0; 1])).await.is_err(), "response must not use the other TCP connection");
    stream.send(&request("out", "REGISTER", server_port)).await.unwrap();
    assert!(read_frame(&mut server).await.starts_with(b"REGISTER "));
}

#[tokio::test]
async fn tcp_failed_challenge_rolls_back_streams_headers_and_listener_without_rebinding() {
    let (mut stream, old_peer) = fixture().await;
    let old_route = stream.route(); let old_id = stream.current.streams.client.id();
    let (client_port, server_port) = stream.reserve_for_test().await;
    let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let route = ImsRoute { local_addr: SocketAddr::from(([127,0,0,1], client_port)), pcscf_addr: server.local_addr().unwrap(), transport: SipTransport::Tcp };
    stream.activate(route, SocketAddr::from(([127,0,0,1], server_port)), "127.0.0.1:5099".parse().unwrap(), Some("new-verify".into())).await.unwrap();
    let (_new_peer, _) = server.accept().await.unwrap();
    assert!(stream.staged.is_some()); stream.rollback();
    assert_eq!(stream.route(), old_route); assert_eq!(stream.current.streams.client.id(), old_id);
    assert!(stream.security_verify().is_none());
    let mut old_peer = old_peer; let frame = request("still-old", "OPTIONS", old_route.local_addr.port());
    stream.send(&frame).await.unwrap(); assert_eq!(read_frame(&mut old_peer).await, frame);
}

#[tokio::test]
async fn tcp_committed_rollover_keeps_old_connections_for_delayed_frames() {
    let (mut stream, _plain_peer) = fixture().await;
    let (client, server_port) = stream.reserve_for_test().await;
    let first_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let route = ImsRoute { local_addr: SocketAddr::from(([127,0,0,1],client)), pcscf_addr: first_listener.local_addr().unwrap(), transport: SipTransport::Tcp };
    stream.activate(route, SocketAddr::from(([127,0,0,1],server_port)), "127.0.0.1:5100".parse().unwrap(), Some("first-sa".into())).await.unwrap();
    let (mut first_peer, _) = first_listener.accept().await.unwrap(); stream.commit();
    let original_listener = stream.current.listener.as_ref().unwrap().clone();
    let (next_client, next_server) = stream.reserve_for_test().await;
    assert_eq!(server_port, next_server);
    assert!(matches!(stream.reserved_receive.as_ref(), Some(ReservedReceive::Existing(listener)) if Arc::ptr_eq(&original_listener, listener)));
    let next_listener=TcpListener::bind("127.0.0.1:0").await.unwrap();
    stream.activate(ImsRoute { local_addr: SocketAddr::from(([127,0,0,1],next_client)), pcscf_addr: next_listener.local_addr().unwrap(), transport: SipTransport::Tcp },
        SocketAddr::from(([127,0,0,1],server_port)), "127.0.0.1:5101".parse().unwrap(), Some("second-sa".into())).await.unwrap();
    let (mut next_peer, _) = next_listener.accept().await.unwrap(); stream.commit();
    assert_eq!(stream.security_verify(), Some("second-sa")); assert!(stream.retired.is_some());
    let delayed=request("delayed", "OPTIONS", server_port); first_peer.write_all(&delayed).await.unwrap();
    assert_eq!(stream.receive(Duration::from_secs(2),true).await.unwrap(), delayed);
    stream.send(&response(&delayed,200)).await.unwrap(); assert!(read_frame(&mut first_peer).await.starts_with(b"SIP/2.0 200"));
    stream.discard_retired(); let fresh=request("current", "OPTIONS", server_port); next_peer.write_all(&fresh).await.unwrap();
    assert_eq!(stream.receive(Duration::from_secs(2),true).await.unwrap(),fresh);
}

#[tokio::test]
async fn wrong_peer_connection_burst_yields_without_invalidating_the_owned_flow() {
    let (mut stream, _old_peer) = fixture().await;
    let (client, server_port) = stream.reserve_for_test().await;
    let remote = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let expected = TcpSocket::new_v4().unwrap(); expected.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    stream.activate(ImsRoute { local_addr: SocketAddr::from(([127,0,0,1], client)), pcscf_addr: remote.local_addr().unwrap(), transport: SipTransport::Tcp },
        SocketAddr::from(([127,0,0,1], server_port)), expected.local_addr().unwrap(), Some("verified".into())).await.unwrap();
    let (mut primary, _) = remote.accept().await.unwrap();
    let peers = tokio::spawn(async move {
        let mut peers = Vec::new();
        for _ in 0..8 { peers.push(TcpStream::connect(SocketAddr::from(([127,0,0,1], server_port))).await.unwrap()); }
        peers
    });
    assert_eq!(stream.receive(Duration::from_secs(3), true).await.unwrap_err().code(), code::CHANNEL_READ_TIMEOUT);
    let _peers = peers.await.unwrap();
    assert_eq!(stream.security_verify(), Some("verified"));
    assert!(stream.current.streams.client.is_usable());
    let valid = request("after-burst", "OPTIONS", server_port);
    primary.write_all(&valid).await.unwrap();
    assert_eq!(stream.receive(Duration::from_secs(2), true).await.unwrap(), valid);
}

#[tokio::test]
async fn tcp_listener_rejects_a_different_remote_tuple_without_borrowing_its_frames() {
    let (mut stream, _old_peer) = fixture().await; let (client, server_port)=stream.reserve_for_test().await;
    let remote=TcpListener::bind("127.0.0.1:0").await.unwrap();
    stream.activate(ImsRoute {local_addr:SocketAddr::from(([127,0,0,1],client)),pcscf_addr:remote.local_addr().unwrap(),transport:SipTransport::Tcp},
        SocketAddr::from(([127,0,0,1],server_port)),"127.0.0.1:5999".parse().unwrap(),Some("verified".into())).await.unwrap();
    let (mut primary, _)=remote.accept().await.unwrap(); let mut wrong=TcpStream::connect(SocketAddr::from(([127,0,0,1],server_port))).await.unwrap();
    wrong.write_all(&request("wrong", "NOTIFY",server_port)).await.unwrap();
    let correct=request("correct", "OPTIONS",server_port); primary.write_all(&correct).await.unwrap();
    assert_eq!(stream.receive(Duration::from_secs(2),true).await.unwrap(),correct);
    assert!(stream.current.streams.server.is_none());
}
