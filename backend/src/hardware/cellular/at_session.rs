//! Persistent AT command sessions for modem serial ports.
//!
//! ModemManager exposes one AT endpoint per modem, but opening and closing the
//! character device for every command is unsafe for URC-driven operations:
//! a late `+CUSD:` (or a future APDU response) can be left in the driver input
//! queue and be consumed by the next transaction.  This module keeps one
//! session per device, serializes access to it, configures the port explicitly
//! (raw, 115200 8N1), and discards a broken session after a transport error.

#[cfg(unix)]
use std::collections::HashMap;
#[cfg(unix)]
use std::sync::{Arc, Mutex, OnceLock};
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::fs::{File, OpenOptions};
#[cfg(unix)]
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};

#[cfg(unix)]
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(unix)]
const USSD_TIMEOUT: Duration = Duration::from_secs(20);
#[cfg(unix)]
const USSD_CANCEL_TIMEOUT: Duration = Duration::from_secs(3);
#[cfg(unix)]
const READ_POLL: Duration = Duration::from_millis(20);
#[cfg(unix)]
const DRAIN_GRACE: Duration = Duration::from_millis(120);
#[cfg(unix)]
const MAX_BUFFER_BYTES: usize = 32 * 1024;
#[cfg(unix)]
const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
#[cfg(unix)]
const MAX_DRAIN_READS: usize = 64;

#[cfg(unix)]
struct AtSession {
    device: String,
    port: Option<File>,
    read_buf: Vec<u8>,
    urcs: super::at_urc::UrcRouter,
}

#[cfg(unix)]
impl AtSession {
    fn new(device: &str) -> Self {
        Self {
            device: device.to_string(),
            port: None,
            read_buf: Vec::new(),
            urcs: super::at_urc::UrcRouter::default(),
        }
    }

    fn reset(&mut self) {
        self.port = None;
        self.read_buf.clear();
        self.urcs.reset_frame();
    }

    fn ensure_open(&mut self) -> Result<(), String> {
        if self.port.is_some() {
            return Ok(());
        }
        let port = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.device)
            .map_err(|err| format!("failed to open AT port {}: {err}", self.device))?;
        configure_port(&port)
            .map_err(|err| format!("failed to configure AT port {}: {err}", self.device))?;
        self.port = Some(port);
        Ok(())
    }

    fn execute_command(&mut self, command: &str, timeout: Duration) -> Result<String, String> {
        self.ensure_open()?;
        if let Err(err) = self.discard_pending() {
            self.reset();
            return Err(err);
        }
        if let Err(err) = self.write_command(command) {
            self.reset();
            return Err(err);
        }

        let deadline = Instant::now() + timeout;
        let mut lines = Vec::new();
        let mut response_bytes = 0usize;
        loop {
            let Some(line) = self.next_line(deadline).map_err(|err| {
                self.reset();
                err
            })?
            else {
                self.reset();
                return Err(format!("timed out waiting for AT response to {command}"));
            };
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed == command {
                continue;
            }
            if self.urcs.route(trimmed, Some(command)) {
                continue;
            }
            response_bytes = response_bytes.saturating_add(line.len());
            if response_bytes > MAX_RESPONSE_BYTES {
                self.reset();
                return Err("AT response exceeds size limit".into());
            }
            let is_final = is_final_line(trimmed);
            lines.push(line);
            if is_final {
                break;
            }
        }
        let output = lines.join("\r\n");
        if lines.iter().any(|line| is_error_line(line.trim())) {
            Err(output)
        } else {
            Ok(if output.is_empty() {
                "ok".to_string()
            } else {
                output
            })
        }
    }

    fn execute_ussd(&mut self, command: &str) -> Result<String, String> {
        self.ensure_open()?;
        if let Err(err) = self.discard_pending() {
            self.reset();
            return Err(err);
        }
        if let Err(err) = self.write_command(command) {
            self.reset();
            return Err(err);
        }

        let deadline = Instant::now() + USSD_TIMEOUT;
        let mut lines = Vec::new();
        let mut saw_cusd = false;
        let mut saw_final = false;
        let mut response_bytes = 0usize;
        loop {
            let Some(line) = self.next_line(deadline).map_err(|err| {
                self.reset();
                err
            })?
            else {
                break;
            };
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed == command {
                continue;
            }
            if self.urcs.route(trimmed, Some(command)) {
                continue;
            }
            response_bytes = response_bytes.saturating_add(line.len());
            if response_bytes > MAX_RESPONSE_BYTES {
                self.reset();
                return Err("AT response exceeds size limit".into());
            }
            if trimmed.to_ascii_uppercase().starts_with("+CUSD:") {
                saw_cusd = true;
            }
            if is_final_line(trimmed) {
                saw_final = true;
                if is_error_line(trimmed) && !saw_cusd {
                    let output = lines_with(&lines, &line);
                    self.cancel_ussd_best_effort();
                    return Err(output);
                }
            }
            lines.push(line);
            // Quectel firmware returns both pieces, but their order varies.
            // Do not stop at OK: wait for the asynchronous +CUSD URC too.
            if saw_cusd && saw_final {
                break;
            }
        }

        let output = lines.join("\r\n");
        if saw_cusd {
            return Ok(output);
        }

        // A firmware branch can omit the final response, but a bare OK is not
        // a completed USSD transaction. Give already queued bytes a short
        // grace period, then fail so the caller can report the missing URC.
        if !saw_final {
            let grace_deadline = Instant::now() + DRAIN_GRACE;
            while Instant::now() < grace_deadline {
                let _ = self.read_available().map_err(|err| {
                    self.reset();
                    err
                })?;
                std::thread::sleep(READ_POLL);
            }
        }
        // A missing +CUSD URC can leave the modem in an interactive USSD
        // state even when the command's OK was received. Explicitly close
        // that state before releasing the session, otherwise the next AT
        // command may be answered by a late URC from this transaction.
        self.cancel_ussd_best_effort();
        if output.is_empty() {
            Err("timed out waiting for +CUSD response".to_string())
        } else {
            Err(format!("timed out waiting for +CUSD response: {output}"))
        }
    }

    fn send_sms_pdu(&mut self, pdu: &str, tpdu_length: usize) -> Result<String, String> {
        if pdu.is_empty()
            || pdu.len() > 1024
            || pdu.len() % 2 != 0
            || !pdu.bytes().all(|b| b.is_ascii_hexdigit())
            || tpdu_length == 0
            || tpdu_length > 255
        {
            return Err("invalid modem SMS PDU".into());
        }
        self.execute_command("AT+CMGF=0", COMMAND_TIMEOUT)?;
        self.write_command(&format!("AT+CMGS={tpdu_length}"))?;
        let deadline = Instant::now() + Duration::from_secs(10);
        'prompt: loop {
            self.read_available()?;
            // A '>' inside a USSD/vendor line is not the CMGS prompt. Frame
            // complete lines first; recognize a prompt only at a frame start.
            if self.take_sms_prompt() {
                break;
            }
            while let Some(line) = self.pop_line() {
                if line.trim() == ">" {
                    break 'prompt;
                }
                if self.urcs.route(&line, Some("AT+CMGS")) {
                    continue;
                }
                if is_error_line(line.trim()) {
                    self.reset();
                    return Err("modem SMS prompt rejected".into());
                }
            }
            if self.take_sms_prompt() {
                break;
            }
            if Instant::now() >= deadline {
                if let Some(port) = self.port.as_mut() {
                    let _ = port.write_all(&[0x1b]);
                }
                self.reset();
                return Err("modem SMS prompt unavailable".into());
            }
            std::thread::sleep(READ_POLL);
        }
        let port = self
            .port
            .as_mut()
            .ok_or_else(|| "AT port is not open".to_string())?;
        port.write_all(pdu.as_bytes())
            .map_err(|_| "SMS PDU write failed".to_string())?;
        port.write_all(&[0x1a])
            .map_err(|_| "SMS submit write failed".to_string())?;
        port.flush()
            .map_err(|_| "SMS submit flush failed".to_string())?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut reference = None;
        loop {
            let line = match self.next_line(deadline)? {
                Some(line) => line,
                None => {
                    self.reset();
                    // Never retry automatically: the network may have accepted it.
                    return Err("SMS submission result unconfirmed".into());
                }
            };
            if self.urcs.route(line.trim(), Some("AT+CMGS")) {
                continue;
            }
            if let Some(value) = line.trim().strip_prefix("+CMGS:") {
                reference = value.trim().parse::<u32>().ok();
            }
            if is_error_line(line.trim()) {
                self.reset();
                return Err("modem rejected SMS submission".into());
            }
            if line.trim() == "OK" {
                return reference
                    .map(|r| r.to_string())
                    .ok_or_else(|| "SMS submission result unconfirmed".into());
            }
        }
    }

    fn cancel_ussd_best_effort(&mut self) {
        let _ = self.execute_command("AT+CUSD=2", USSD_CANCEL_TIMEOUT);
        self.reset();
    }

    fn write_command(&mut self, command: &str) -> Result<(), String> {
        let command = command.trim();
        if command.is_empty() || command.contains(['\r', '\n']) {
            return Err("AT command must be a single non-empty line".to_string());
        }
        let Some(port) = self.port.as_mut() else {
            return Err("AT port is not open".to_string());
        };
        port.write_all(format!("{command}\r").as_bytes())
            .map_err(|err| format!("failed to write AT command: {err}"))?;
        port.flush()
            .map_err(|err| format!("failed to flush AT command: {err}"))
    }

    fn discard_pending(&mut self) -> Result<(), String> {
        // Preserve complete URCs before discarding stale command replies.
        // A continuous URC stream must not hold the physical gate forever.
        let deadline = Instant::now() + DRAIN_GRACE;
        for _ in 0..MAX_DRAIN_READS {
            while let Some(line) = self.pop_line() {
                self.urcs.route(&line, None);
            }
            if Instant::now() >= deadline || self.read_available()? == 0 {
                break;
            }
        }
        while let Some(line) = self.pop_line() {
            self.urcs.route(&line, None);
        }
        self.read_buf.clear();
        self.urcs.reset_frame();
        Ok(())
    }

    fn poll_events(&mut self) -> Result<super::at_urc::UrcEvents, String> {
        self.ensure_open()?;
        // Same session mutex / reader as commands; no AT query or competing
        // background reader. Preserve fragmented URCs for the next poll.
        for _ in 0..MAX_DRAIN_READS {
            while let Some(line) = self.pop_line() {
                self.urcs.route(&line, None);
            }
            if self.read_available()? == 0 {
                break;
            }
        }
        while let Some(line) = self.pop_line() {
            self.urcs.route(&line, None);
        }
        Ok(self.urcs.take())
    }

    fn pop_line(&mut self) -> Option<String> {
        let index = self.read_buf.iter().position(|byte| *byte == b'\n')?;
        let bytes: Vec<u8> = self.read_buf.drain(..=index).collect();
        Some(
            String::from_utf8_lossy(&bytes)
                .trim_matches(['\r', '\n'])
                .to_string(),
        )
    }

    fn take_sms_prompt(&mut self) -> bool {
        let start = self
            .read_buf
            .iter()
            .position(|b| !matches!(b, b'\r' | b'\n' | b' '));
        if let Some(start) = start.filter(|&n| self.read_buf[n] == b'>') {
            self.read_buf.drain(..=start);
            true
        } else {
            false
        }
    }

    fn read_available(&mut self) -> Result<usize, String> {
        let Some(port) = self.port.as_mut() else {
            return Err("AT port is not open".to_string());
        };
        let mut buffer = [0u8; 512];
        match port.read(&mut buffer) {
            Ok(n) => {
                if n > 0 {
                    if self.read_buf.len().saturating_add(n) > MAX_BUFFER_BYTES {
                        return Err("AT frame exceeds size limit".into());
                    }
                    self.read_buf.extend_from_slice(&buffer[..n]);
                }
                Ok(n)
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => Ok(0),
            Err(err) => Err(format!("failed to read AT response: {err}")),
        }
    }

    fn next_line(&mut self, deadline: Instant) -> Result<Option<String>, String> {
        loop {
            if let Some(line) = self.pop_line() {
                return Ok(Some(line));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            self.read_available()?;
            if !self.read_buf.contains(&b'\n') {
                std::thread::sleep(READ_POLL);
            }
        }
    }
}

#[cfg(unix)]
fn lines_with(lines: &[String], extra: &str) -> String {
    let mut all = lines.to_vec();
    all.push(extra.to_string());
    all.join("\r\n")
}

#[cfg(unix)]
fn is_error_line(line: &str) -> bool {
    line.eq_ignore_ascii_case("ERROR")
        || line.eq_ignore_ascii_case("NO CARRIER")
        || line.eq_ignore_ascii_case("BUSY")
        || line.eq_ignore_ascii_case("NO ANSWER")
        || line.eq_ignore_ascii_case("NO DIALTONE")
        || line.eq_ignore_ascii_case("NO DIAL TONE")
        || line.to_ascii_uppercase().starts_with("+CME ERROR")
        || line.to_ascii_uppercase().starts_with("+CMS ERROR")
}

#[cfg(unix)]
fn is_final_line(line: &str) -> bool {
    line.eq_ignore_ascii_case("OK") || is_error_line(line)
}

#[cfg(unix)]
fn configure_port(port: &File) -> io::Result<()> {
    let fd = port.as_raw_fd();
    set_nonblocking(fd)?;
    unsafe {
        let mut termios = std::mem::zeroed::<libc::termios>();
        if libc::tcgetattr(fd, &mut termios) != 0 {
            return Err(io::Error::last_os_error());
        }
        libc::cfmakeraw(&mut termios);
        if libc::cfsetispeed(&mut termios, libc::B115200) != 0
            || libc::cfsetospeed(&mut termios, libc::B115200) != 0
        {
            return Err(io::Error::last_os_error());
        }
        termios.c_cflag |= libc::CLOCAL | libc::CREAD;
        termios.c_cflag &= !libc::CSTOPB;
        termios.c_cflag &= !libc::PARENB;
        termios.c_cflag &= !libc::CSIZE;
        termios.c_cflag |= libc::CS8;
        if libc::tcsetattr(fd, libc::TCSANOW, &termios) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(unix)]
fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
type SessionMap = HashMap<String, Arc<Mutex<AtSession>>>;
#[cfg(unix)]
static SESSIONS: OnceLock<Mutex<SessionMap>> = OnceLock::new();

#[cfg(unix)]
fn sessions() -> &'static Mutex<SessionMap> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(unix)]
fn session_for(device: &str) -> Arc<Mutex<AtSession>> {
    let mut sessions = sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    sessions
        .entry(device.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(AtSession::new(device))))
        .clone()
}

/// Execute an ordinary line-oriented AT command on a persistent session.
#[cfg(unix)]
pub fn execute_command(device: &str, command: &str) -> Result<String, String> {
    let session = session_for(device);
    let mut session = session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    session.execute_command(command, COMMAND_TIMEOUT)
}

#[cfg(not(unix))]
pub fn execute_command(_device: &str, _command: &str) -> Result<String, String> {
    Err("AT port access is only supported on Unix devices".to_string())
}

/// Execute an AT+CUSD transaction and wait for the asynchronous +CUSD URC.
#[cfg(unix)]
pub fn execute_command_with_timeout(
    device: &str,
    command: &str,
    timeout: Duration,
) -> Result<String, String> {
    let session = session_for(device);
    let mut session = session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    session.execute_command(
        command,
        timeout.clamp(Duration::from_secs(1), Duration::from_secs(120)),
    )
}

#[cfg(not(unix))]
pub fn execute_command_with_timeout(
    _device: &str,
    _command: &str,
    _timeout: std::time::Duration,
) -> Result<String, String> {
    Err("AT port access is only supported on Unix devices".into())
}

/// Execute an AT+CUSD transaction and wait for the asynchronous +CUSD URC.
#[cfg(unix)]
pub fn execute_ussd(device: &str, command: &str) -> Result<String, String> {
    let session = session_for(device);
    let mut session = session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    session.execute_ussd(command)
}

#[cfg(not(unix))]
pub fn execute_ussd(_device: &str, _command: &str) -> Result<String, String> {
    Err("USSD AT port access is only supported on Unix devices".to_string())
}

#[cfg(unix)]
pub fn send_sms_pdu(device: &str, pdu: &str, tpdu_length: usize) -> Result<String, String> {
    let session = session_for(device);
    let mut session = session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = session.send_sms_pdu(pdu, tpdu_length);
    if result.is_err() {
        if let Some(port) = session.port.as_mut() {
            let _ = port.write_all(&[0x1b]);
        }
        session.reset();
    }
    result
}

#[cfg(not(unix))]
pub fn send_sms_pdu(_device: &str, _pdu: &str, _tpdu_length: usize) -> Result<String, String> {
    Err("SMS AT port access is only supported on Unix devices".into())
}

/// Poll coalesced indications under the persistent port's single-reader lock.
/// Callers must already hold the native physical lease and verify port ownership.
#[cfg(unix)]
pub fn poll_events(device: &str) -> Result<super::at_urc::UrcEvents, String> {
    let session = session_for(device);
    let mut session = session
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let result = session.poll_events();
    if result.is_err() {
        session.reset();
    }
    result
}

#[cfg(not(unix))]
pub fn poll_events(_device: &str) -> Result<super::at_urc::UrcEvents, String> {
    Err("AT port access is only supported on Unix devices".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::fd::OwnedFd;
    use std::os::unix::net::UnixStream;

    fn pair() -> (AtSession, UnixStream) {
        let (port, peer) = UnixStream::pair().unwrap();
        port.set_nonblocking(true).unwrap();
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let fd: OwnedFd = port.into();
        let mut session = AtSession::new("fixture");
        session.port = Some(File::from(fd));
        (session, peer)
    }

    fn read_until(peer: &mut UnixStream, end: u8) -> Vec<u8> {
        let mut value = Vec::new();
        loop {
            let mut byte = [0];
            peer.read_exact(&mut byte).unwrap();
            if byte[0] == end {
                return value;
            }
            value.push(byte[0]);
            assert!(value.len() < 2048);
        }
    }

    #[test]
    fn unrelated_urcs_do_not_pollute_a_command_or_steal_its_final() {
        let (mut session, mut peer) = pair();
        let server = std::thread::spawn(move || {
            assert_eq!(read_until(&mut peer, b'\r'), b"AT+CSQ");
            peer.write_all(b"\r\n+CMTI: \"SM\",7\r\nNO CARRIER\r\n+CSQ: 20,99\r\nOK\r\n")
                .unwrap();
        });
        let response = session
            .execute_command("AT+CSQ", Duration::from_secs(1))
            .unwrap();
        server.join().unwrap();
        assert_eq!(response, "+CSQ: 20,99\r\nOK");
        let events = session.urcs.take();
        assert!(events.sms_stored && events.call_changed);
    }

    #[test]
    fn passive_poll_preserves_fragmented_urcs_and_does_not_send_commands() {
        let (mut session, mut peer) = pair();
        peer.write_all(b"+CMTI: \"SM\",").unwrap();
        assert!(!session.poll_events().unwrap().needs_sms_scan());
        peer.write_all(b"7\r\n+QIND: ignored-data\r\n").unwrap();
        let events = session.poll_events().unwrap();
        assert!(events.sms_stored && events.vendor);
        assert_eq!(
            session.poll_events().unwrap(),
            super::super::at_urc::UrcEvents::default()
        );
        peer.set_nonblocking(true).unwrap();
        let mut byte = [0];
        assert_eq!(
            peer.read(&mut byte).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn pending_urcs_survive_command_preflight_drain() {
        let (mut session, mut peer) = pair();
        peer.write_all(b"+CMTI: \"SM\",1\r\nOK\r\n").unwrap();
        let server = std::thread::spawn(move || {
            assert_eq!(read_until(&mut peer, b'\r'), b"AT");
            peer.write_all(b"OK\r\n").unwrap();
        });
        assert_eq!(
            session
                .execute_command("AT", Duration::from_secs(1))
                .unwrap(),
            "OK"
        );
        server.join().unwrap();
        assert!(session.urcs.take().sms_stored);
    }

    #[test]
    fn sms_prompt_is_framed_not_found_inside_unsolicited_text() {
        let mut session = AtSession::new("fixture");
        session.read_buf = b"+CUSD: 0,\"amount > 0\",15\r\n".to_vec();
        assert!(!session.take_sms_prompt());
        session.pop_line().unwrap();
        session.read_buf = b"\r\n> ".to_vec();
        assert!(session.take_sms_prompt());
        assert!(!session.take_sms_prompt());
    }

    #[test]
    fn sms_submit_interleaves_urcs_without_losing_prompt_or_reference() {
        let (mut session, mut peer) = pair();
        let server = std::thread::spawn(move || {
            assert_eq!(read_until(&mut peer, b'\r'), b"AT+CMGF=0");
            peer.write_all(b"OK\r\n").unwrap();
            assert_eq!(read_until(&mut peer, b'\r'), b"AT+CMGS=3");
            peer.write_all(b"+CUSD: 0,\"text > not prompt\",15\r\n> \r\n")
                .unwrap();
            assert_eq!(read_until(&mut peer, 0x1a), b"00010203");
            peer.write_all(b"+CMTI: \"SM\",2\r\n+CMGS: 9\r\nOK\r\n")
                .unwrap();
        });
        assert_eq!(session.send_sms_pdu("00010203", 3).unwrap(), "9");
        server.join().unwrap();
        let events = session.urcs.take();
        assert!(events.ussd && events.sms_stored);
    }

    #[test]
    fn unterminated_at_frames_are_bounded() {
        let (mut session, mut peer) = pair();
        session.read_buf.resize(MAX_BUFFER_BYTES, b'x');
        peer.write_all(b"x").unwrap();
        assert_eq!(
            session.read_available().unwrap_err(),
            "AT frame exceeds size limit"
        );
    }

    #[test]
    fn recognizes_final_lines_case_insensitively() {
        assert!(super::is_final_line("OK"));
        assert!(super::is_final_line("+CME ERROR: 10"));
        assert!(!super::is_final_line("+CUSD: 0,\"done\",15"));
    }
}
