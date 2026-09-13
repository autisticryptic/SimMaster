//! Keep the physical QMI channel open for the entire native ownership epoch.
//!
//! `--client-no-release-cid` preserves a firmware client, not the proxy's
//! connection to the character device. When its last socket user exits,
//! qmi-proxy closes that device; BAM-DMUX firmware then invalidates the CID.
//! A proxy-open lease (without allocating any service client) prevents that
//! gap between bounded qmicli transactions. It is never silently reconnected.

#[cfg(unix)]
use std::{
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
    time::Duration,
};

#[cfg(unix)]
use super::NativeError;
#[cfg(unix)]
use crate::connectivity::modems::ims::vowifi::qmi_uim::{
    build_proxy_open_frame, decode_qmi_frame, QmiUimError,
};

#[cfg(unix)]
pub struct QmiProxyLease {
    stream: UnixStream,
}

#[cfg(unix)]
impl QmiProxyLease {
    pub fn open(device: &str) -> Result<Self, QmiUimError> {
        use std::os::linux::net::SocketAddrExt;
        use std::os::unix::net::SocketAddr;
        let address = SocketAddr::from_abstract_name(b"qmi-proxy")?;
        Self::open_stream(UnixStream::connect_addr(&address)?, device)
    }

    fn open_stream(mut stream: UnixStream, device: &str) -> Result<Self, QmiUimError> {
        stream.set_read_timeout(Some(Duration::from_secs(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(15)))?;
        stream.write_all(&build_proxy_open_frame(device, 1)?)?;
        let response = read_frame(&mut stream)?;
        let message = decode_qmi_frame(&response)?;
        if message.service != 0
            || message.client_id != 0
            || message.transaction_id != 1
            || message.message_id != 0xff00
            || response[6] != 1
        // CTL response, not an unsolicited indication.
        {
            return Err(QmiUimError::InvalidFrame);
        }
        let results = message
            .tlvs
            .iter()
            .filter(|t| t.tlv_type == 2)
            .collect::<Vec<_>>();
        if results.len() != 1 || results[0].value.len() != 4 {
            return Err(QmiUimError::InvalidFrame);
        }
        let value = &results[0].value;
        let status = u16::from_le_bytes([value[0], value[1]]);
        let error = u16::from_le_bytes([value[2], value[3]]);
        if status != 0 {
            return Err(QmiUimError::ResultFailure(error));
        }
        if error != 0 {
            return Err(QmiUimError::InvalidFrame);
        }
        stream.set_nonblocking(true)?;
        Ok(Self { stream })
    }

    /// A restarted proxy is a new ownership epoch even if /dev is unchanged.
    /// Never open a new socket and replay CIDs from the old epoch.
    pub fn verify_alive(&self) -> Result<(), NativeError> {
        let lost = || NativeError::OwnerConflict("native_qmi_proxy_epoch_lost".into());
        let mut descriptor = libc::pollfd {
            fd: self.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut descriptor, 1, 0) } < 0
            || descriptor.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0
        {
            return Err(lost());
        }
        // There are no pending commands on this socket after proxy-open.
        // Drain harmless unsolicited control notices without an unbounded
        // reader task or letting its socket buffer fill indefinitely.
        let mut stream = &self.stream;
        let mut buffer = [0u8; 4096];
        for _ in 0..16 {
            match stream.read(&mut buffer) {
                Ok(0) => return Err(lost()),
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(lost()),
            }
        }
        Err(NativeError::Unavailable(
            "native_qmi_proxy_notice_limit".into(),
        ))
    }
}

#[cfg(unix)]
fn read_frame(stream: &mut UnixStream) -> Result<Vec<u8>, QmiUimError> {
    let mut header = [0u8; 3];
    stream.read_exact(&mut header)?;
    let length = usize::from(u16::from_le_bytes([header[1], header[2]])) + 1;
    if header[0] != 1 || length < 12 {
        return Err(QmiUimError::InvalidFrame);
    }
    let mut frame = vec![0; length];
    frame[..3].copy_from_slice(&header);
    stream.read_exact(&mut frame[3..])?;
    Ok(frame)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::connectivity::modems::ims::vowifi::qmi_uim::{
        encode_qmi_message, QmiMessage, QmiTlv,
    };

    fn reply(transaction: u16) -> Vec<u8> {
        let mut frame = encode_qmi_message(&QmiMessage {
            service: 0,
            client_id: 0,
            transaction_id: transaction,
            message_id: 0xff00,
            tlvs: vec![QmiTlv {
                tlv_type: 2,
                value: vec![0; 4],
            }],
        })
        .unwrap();
        frame[3] = 0x80;
        frame[6] = 1;
        frame
    }

    #[test]
    fn proxy_lease_keeps_its_socket_open_and_detects_epoch_loss() {
        let (client, mut proxy) = UnixStream::pair().unwrap();
        proxy.write_all(&reply(1)).unwrap();
        let lease = QmiProxyLease::open_stream(client, "/dev/fixture").unwrap();
        let request = decode_qmi_frame(&read_frame(&mut proxy).unwrap()).unwrap();
        assert_eq!(request.message_id, 0xff00);
        assert_eq!(request.tlvs[0].value, b"/dev/fixture");
        lease.verify_alive().unwrap();
        proxy.write_all(b"unsolicited control notice").unwrap();
        lease.verify_alive().unwrap();
        // Even pending data must not mask a closed peer.
        proxy.write_all(b"last notice").unwrap();
        drop(proxy);
        assert_eq!(
            lease.verify_alive().unwrap_err(),
            NativeError::OwnerConflict("native_qmi_proxy_epoch_lost".into())
        );
    }

    #[test]
    fn proxy_lease_requires_its_own_open_acknowledgement() {
        let (client, mut proxy) = UnixStream::pair().unwrap();
        proxy.write_all(&reply(2)).unwrap();
        assert!(QmiProxyLease::open_stream(client, "/dev/fixture").is_err());
    }
}
