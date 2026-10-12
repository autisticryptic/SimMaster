//! One bearer/worker-local DNS transaction. TC permits exactly one TCP retry
//! with the SAME question/ID and the SAME overall deadline, never host DNS.
use std::{future::Future, net::{IpAddr, SocketAddr}, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpStream, UdpSocket}};
use crate::services::ue_worker::{UeSocket, UeSocketSpec, UeWorkerHandle};
use super::super::{errors::{code, CellularImsError}, pcscf_dns::{build_dns_query, parse_dns_response, response_is_truncated, DnsRecords}};

fn failed() -> CellularImsError { CellularImsError::new(code::RUNTIME_ALL_PCSCF_FAILED) }

pub(super) async fn query(
    local: IpAddr, remote: SocketAddr, name: &str, kind: u16,
    interface: &str, worker: &UeWorkerHandle, timeout: Duration,
) -> Result<DnsRecords, CellularImsError> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut id = [0u8; 2];
    SystemRandom::new().fill(&mut id).map_err(|_| CellularImsError::new(code::RANDOM_FAILED))?;
    let id = u16::from_ne_bytes(id);
    let query = build_dns_query(id, name, kind)?;
    let binding = worker.bind();
    let deadline = tokio::time::Instant::now() + timeout;
    let operation = async {
        if !binding.is_current() { return Err(CellularImsError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED)); }
        let spec = UeSocketSpec::udp_connected(SocketAddr::new(local, 0), remote, Some(interface.into()));
        let socket = match worker.create_socket(spec).await { Ok(UeSocket::Udp(socket)) => socket, _ => return Err(failed()) };
        if !binding.is_current() { return Err(CellularImsError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED)); }
        let result = exchange(&socket, id, name, kind, &query, || async {
            if !binding.is_current() { return Err(CellularImsError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED)); }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() { return Err(failed()); }
            let spec = UeSocketSpec::tcp_connected(SocketAddr::new(local, 0), remote,
                Some(interface.into()), remaining.as_secs().saturating_add(1));
            let socket = match worker.create_socket(spec).await { Ok(UeSocket::Tcp(socket)) => socket, _ => return Err(failed()) };
            if !binding.is_current() { return Err(CellularImsError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED)); }
            Ok(socket)
        }).await?;
        if !binding.is_current() { return Err(CellularImsError::new(code::RUNTIME_UE_WORKER_GENERATION_CHANGED)); }
        Ok(result)
    };
    tokio::time::timeout_at(deadline, operation).await.map_err(|_| failed())?
}

async fn exchange<F, Fut>(
    udp: &UdpSocket, id: u16, name: &str, kind: u16, question: &[u8], connect_tcp: F,
) -> Result<DnsRecords, CellularImsError>
where F: FnOnce() -> Fut, Fut: Future<Output=Result<TcpStream, CellularImsError>> {
    if udp.send(question).await.map_err(|_| failed())? != question.len() { return Err(failed()); }
    let mut response = vec![0u8; 65_535];
    let size = udp.recv(&mut response).await.map_err(|_| failed())?;
    let response = &response[..size];
    if !response_is_truncated(id, name, kind, response)? {
        return parse_dns_response(id, name, kind, response);
    }
    let mut tcp = connect_tcp().await?;
    tcp.write_all(&(question.len() as u16).to_be_bytes()).await.map_err(|_| failed())?;
    tcp.write_all(question).await.map_err(|_| failed())?;
    let size = usize::from(tcp.read_u16().await.map_err(|_| failed())?);
    if size < 12 { return Err(failed()); }
    let mut response = vec![0u8; size];
    tcp.read_exact(&mut response).await.map_err(|_| failed())?;
    // The full parser still rejects TC in TCP, wrong question/id/flags, bad
    // compression/lengths, unsolicited glue or malformed trailing records.
    parse_dns_response(id, name, kind, &response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};

    fn answer(id: u16, tc: bool) -> Vec<u8> {
        let mut response = build_dns_query(id, "pcscf.example", 1).unwrap();
        response[2..4].copy_from_slice(&(if tc { 0x8380u16 } else { 0x8180u16 }).to_be_bytes());
        if !tc {
            response[6..8].copy_from_slice(&1u16.to_be_bytes());
            response.extend_from_slice(&[0xc0, 12, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 192, 0, 2, 10]);
        }
        response
    }

    #[tokio::test]
    async fn dns_tc_retries_same_question_over_tcp_and_reads_fragmented_frame() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let remote = listener.local_addr().unwrap();
        let server = UdpSocket::bind(remote).await.unwrap();
        let client = UdpSocket::bind("127.0.0.1:0").await.unwrap(); client.connect(remote).await.unwrap();
        let question = build_dns_query(7, "pcscf.example", 1).unwrap();
        let expected = question.clone();
        let peer = tokio::spawn(async move {
            let mut data = [0u8; 512]; let (n, addr) = server.recv_from(&mut data).await.unwrap();
            assert_eq!(&data[..n], expected.as_slice());
            server.send_to(&answer(7, true), addr).await.unwrap();
            let (mut tcp, _) = listener.accept().await.unwrap();
            let n = tcp.read_u16().await.unwrap(); let mut data = vec![0; n as usize]; tcp.read_exact(&mut data).await.unwrap();
            assert_eq!(data, expected);
            let response = answer(7, false);
            let mut bytes = (response.len() as u16).to_be_bytes().to_vec(); bytes.extend(response);
            for byte in bytes { tcp.write_all(&[byte]).await.unwrap(); }
        });
        let records = tokio::time::timeout(Duration::from_secs(2), exchange(&client, 7, "pcscf.example", 1, &question,
            || async { TcpStream::connect(remote).await.map_err(|_| failed()) })).await.unwrap().unwrap();
        peer.await.unwrap(); assert_eq!(records.addresses, vec!["192.0.2.10".parse::<IpAddr>().unwrap()]);
    }

    #[tokio::test]
    async fn dns_untruncated_or_unassociated_udp_never_opens_tcp() {
        for (valid, tc) in [(true, false), (false, true)] {
            let server = UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let client = UdpSocket::bind("127.0.0.1:0").await.unwrap(); client.connect(server.local_addr().unwrap()).await.unwrap();
            let peer = tokio::spawn(async move { let mut data = [0; 512]; let (_, addr) = server.recv_from(&mut data).await.unwrap(); server.send_to(&answer(if valid { 7 } else { 8 }, tc), addr).await.unwrap(); });
            let result = exchange(&client, 7, "pcscf.example", 1, &build_dns_query(7, "pcscf.example", 1).unwrap(), || async { panic!("TCP must not open"); #[allow(unreachable_code)] Err(failed()) }).await;
            assert_eq!(result.is_ok(), valid); peer.await.unwrap();
        }
    }

    #[tokio::test]
    async fn dns_tcp_short_mismatched_and_still_truncated_frames_fail_without_another_retry() {
        for mode in 0..4 {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap(); let remote = listener.local_addr().unwrap();
            let server = UdpSocket::bind(remote).await.unwrap();
            let client = UdpSocket::bind("127.0.0.1:0").await.unwrap(); client.connect(remote).await.unwrap();
            let peer = tokio::spawn(async move {
                let mut data = [0; 512]; let (_, addr) = server.recv_from(&mut data).await.unwrap(); server.send_to(&answer(7, true), addr).await.unwrap();
                let (mut tcp, _) = listener.accept().await.unwrap(); let n=tcp.read_u16().await.unwrap();let mut query=vec![0;n as usize];tcp.read_exact(&mut query).await.unwrap();
                let packet = match mode { 0 => vec![0; 8], 1 => answer(8, false), 2 => answer(7, true), _ => answer(7, false) };
                let length = if mode == 3 { packet.len() + 10 } else { packet.len() };
                tcp.write_all(&(length as u16).to_be_bytes()).await.unwrap();
                tcp.write_all(&packet).await.unwrap();
            });
            let result=tokio::time::timeout(Duration::from_secs(2), exchange(&client,7,"pcscf.example",1,&build_dns_query(7,"pcscf.example",1).unwrap(),||async{TcpStream::connect(remote).await.map_err(|_|failed())})).await.unwrap();
            assert!(result.is_err()); peer.await.unwrap();
        }
    }

    #[tokio::test]
    async fn dns_tcp_attempt_shares_the_original_query_deadline() {
        let server=UdpSocket::bind("127.0.0.1:0").await.unwrap();let client=UdpSocket::bind("127.0.0.1:0").await.unwrap();client.connect(server.local_addr().unwrap()).await.unwrap();
        let peer=tokio::spawn(async move{let mut data=[0;512];let(_,addr)=server.recv_from(&mut data).await.unwrap();server.send_to(&answer(7,true),addr).await.unwrap();});
        let attempts=Arc::new(AtomicUsize::new(0));let seen=attempts.clone();
        let result=tokio::time::timeout(Duration::from_millis(30),exchange(&client,7,"pcscf.example",1,&build_dns_query(7,"pcscf.example",1).unwrap(),||async move{seen.fetch_add(1,Ordering::Relaxed);std::future::pending().await})).await;
        assert!(result.is_err());assert_eq!(attempts.load(Ordering::Relaxed),1);peer.await.unwrap();
    }
}
