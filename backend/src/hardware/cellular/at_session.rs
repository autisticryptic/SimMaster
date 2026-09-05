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
struct AtSession {
    device: String,
    port: Option<File>,
    read_buf: Vec<u8>,
}

#[cfg(unix)]
impl AtSession {
    fn new(device: &str) -> Self {
        Self {
            device: device.to_string(),
            port: None,
            read_buf: Vec::new(),
        }
    }

    fn reset(&mut self) {
        self.port = None;
        self.read_buf.clear();
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
            lines.push(line);
            if is_final_line(trimmed) {
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
        self.read_buf.clear();
        loop {
            match self.read_available()? {
                0 => break,
                _ => continue,
            }
        }
        self.read_buf.clear();
        Ok(())
    }

    fn read_available(&mut self) -> Result<usize, String> {
        let Some(port) = self.port.as_mut() else {
            return Err("AT port is not open".to_string());
        };
        let mut buffer = [0u8; 512];
        match port.read(&mut buffer) {
            Ok(n) => {
                if n > 0 {
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
            if let Some(index) = self.read_buf.iter().position(|byte| *byte == b'\n') {
                let bytes: Vec<u8> = self.read_buf.drain(..=index).collect();
                return Ok(Some(
                    String::from_utf8_lossy(&bytes)
                        .trim_matches(['\r', '\n'])
                        .to_string(),
                ));
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

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn recognizes_final_lines_case_insensitively() {
        assert!(super::is_final_line("OK"));
        assert!(super::is_final_line("+CME ERROR: 10"));
        assert!(!super::is_final_line("+CUSD: 0,\"done\",15"));
    }
}
