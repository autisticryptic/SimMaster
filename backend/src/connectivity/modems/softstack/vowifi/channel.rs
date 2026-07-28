//! VoWiFi protected SIP channel.
//!
//! The TUN/ePDG stack has already decrypted ESP before this TCP stream is
//! opened. This adapter owns TCP framing and exposes the transport-neutral
//! [`ImsChannel`] contract to shared REGISTER/MESSAGE logic.

use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};

use crate::connectivity::core::{access::ImsChannel, context::ImsRoute, sip_frame, ImsError};

const MAX_PENDING_BYTES: usize = 64 * 1024;

pub struct EpdgSipChannel {
    stream: TcpStream,
    pending: Vec<u8>,
    route: ImsRoute,
    security_verify: Option<String>,
}

impl EpdgSipChannel {
    pub fn new(
        stream: TcpStream,
        pending: Vec<u8>,
        route: ImsRoute,
        security_verify: Option<String>,
    ) -> Self {
        Self {
            stream,
            pending,
            route,
            security_verify,
        }
    }

    pub fn into_parts(self) -> (TcpStream, Vec<u8>) {
        (self.stream, self.pending)
    }
}

impl ImsChannel for EpdgSipChannel {
    async fn send_sip(&mut self, frame: &[u8]) -> Result<(), ImsError> {
        self.stream
            .write_all(frame)
            .await
            .map_err(|_| ImsError::new("ims_channel_write_failed"))?;
        self.stream
            .flush()
            .await
            .map_err(|_| ImsError::new("ims_channel_flush_failed"))
    }

    async fn recv_sip(&mut self, timeout: Duration) -> Result<Vec<u8>, ImsError> {
        if let Some(frame_len) = sip_frame::complete_frame_len(&self.pending) {
            return Ok(self.pending.drain(..frame_len).collect());
        }

        tokio::time::timeout(timeout, async {
            loop {
                let mut chunk = [0u8; 2048];
                let read = self
                    .stream
                    .read(&mut chunk)
                    .await
                    .map_err(|_| ImsError::new("ims_channel_read_failed"))?;
                if read == 0 {
                    return Err(ImsError::new("ims_channel_closed"));
                }
                self.pending.extend_from_slice(&chunk[..read]);
                if self.pending.len() > MAX_PENDING_BYTES {
                    return Err(ImsError::new("ims_channel_frame_too_large"));
                }
                if let Some(frame_len) = sip_frame::complete_frame_len(&self.pending) {
                    return Ok(self.pending.drain(..frame_len).collect());
                }
            }
        })
        .await
        .map_err(|_| ImsError::new("ims_channel_read_timeout"))?
    }

    fn route(&self) -> ImsRoute {
        self.route
    }

    fn security_verify(&self) -> Option<&str> {
        self.security_verify.as_deref()
    }
}
