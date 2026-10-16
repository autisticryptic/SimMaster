//! Public, real AtSession boundary using an isolated pseudo-terminal. AtSession
//! is private, so a UnixStream cannot be injected here without production edits.
//! No physical port, global native fleet, MM, or external process is involved.

use super::*;
use std::{
    fs::File,
    io::{Read, Write},
    os::{fd::{AsRawFd, FromRawFd}, unix::fs::symlink},
    sync::{atomic::{AtomicU64, Ordering}, mpsc},
    time::Duration,
};

struct Pair {
    path: std::path::PathBuf,
    _slave: File,
    master: File,
}

impl Pair {
    fn new() -> Self {
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(unsafe {
            libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null(), std::ptr::null())
        }, 0);
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        // Disable echo before injecting old pending data. Production AtSession
        // still opens/configures this fake endpoint itself on first execution.
        unsafe {
            let mut termios = std::mem::zeroed::<libc::termios>();
            assert_eq!(libc::tcgetattr(slave.as_raw_fd(), &mut termios), 0);
            libc::cfmakeraw(&mut termios);
            assert_eq!(libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios), 0);
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let directory = std::env::temp_dir().join(format!("simadmin-ims-serial-{}-{nonce}-{}",
            std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("at");
        // Unique cache key avoids FD/path reuse in the real AtSession map.
        symlink(format!("/proc/self/fd/{}", slave.as_raw_fd()), &path).unwrap();
        Self { path, _slave: slave, master }
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_dir(self.path.parent().unwrap());
    }
}

fn read_command(master: &mut File) -> Vec<u8> {
    let mut command = Vec::new();
    loop {
        let mut poll = libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 2000) }, 1, "fake peer command deadline");
        let mut byte = [0];
        master.read_exact(&mut byte).unwrap();
        command.push(byte[0]);
        if byte[0] == b'\r' { return command; }
        assert!(command.len() < 128);
    }
}

fn exchange(pending: &[u8], fragments: Vec<&'static [u8]>) -> Result<String, String> {
    let mut pair = Pair::new();
    pair.master.write_all(pending).unwrap();
    let mut master = pair.master.try_clone().unwrap();
    let (done, wait) = mpsc::channel();
    let peer = std::thread::spawn(move || {
        assert_eq!(read_command(&mut master), b"AT+QIMSCFG=\"ims_status\"\r");
        for fragment in fragments {
            master.write_all(fragment).unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        // Keep the endpoint alive until the session consumes final/error or
        // reaches its own deadline; never turn peer EOF into fake completion.
        wait.recv_timeout(Duration::from_secs(3)).unwrap();
        let mut poll = libc::pollfd { fd: master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 0) }, 0, "unexpected retry/write");
    });
    let response = crate::hardware::cellular::at_session::execute_command_with_timeout(
        pair.path.to_str().unwrap(), STATUS_COMMAND, Duration::from_secs(1));
    done.send(()).unwrap();
    peer.join().unwrap();
    response
}

#[test]
fn native_ims_real_at_session_frames_fragments_urcs_and_discards_old_pending_status() {
    let output = exchange(
        b"+QIMSCFG:\"ims_status\",1,1,2,2,2,2\r\nOK\r\n",
        vec![
            b"AT+QIMSCFG=\"ims_status\"\r\n+CMTI: \"SM\",1\r\nRING\r\n",
            b"+QIMSREGSTAT: 2\r\n+QIMSSRVSTAT: 2,2,2\r\n+QIMSCFG:",
            b"\"ims_status\",0,1,1,0,1,2\r\nNO CARRIER\r\nOK\r\n",
        ],
    ).unwrap();
    assert!(output.ends_with("OK"));
    let sample = parse_status(&output).unwrap();
    assert_eq!(sample.registration, NativeImsRegistration::Registering);
    assert_eq!(sample.sms, NativeImsService::UnknownOrUnreported);
    assert_eq!(sample.ut, NativeImsService::Available);
}

#[test]
fn native_ims_real_at_session_rejects_error_after_status_without_retry() {
    assert!(exchange(&[], vec![
        b"+QIMSCFG:\"ims_status\",0,0,2,2,2,2\r\n+CME ERROR: 42\r\n",
    ]).is_err());
}

#[test]
fn native_ims_real_at_session_requires_final_completion_without_retry() {
    assert!(exchange(&[], vec![b"+QIMSCFG:\"ims_status\",0,0,2,2,2,2\r\n"]).is_err());
}
