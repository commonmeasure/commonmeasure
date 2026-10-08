//! A controlling terminal with a deadline: an unattended prompt fails the test.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub fn run(
    command: &mut Command,
    terminal_stdin: bool,
    answer: &[u8],
    timeout: Duration,
) -> (ExitStatus, String) {
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: openpty writes two valid descriptors to these local integers.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        },
        0
    );
    // SAFETY: openpty succeeded; each descriptor is owned once here.
    let (mut master, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
    for file in [&master, &slave] {
        // SAFETY: both descriptors remain open; do not leak them across exec.
        assert_eq!(
            unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
            0
        );
    }
    command
        .stdin(if terminal_stdin {
            Stdio::from(slave.try_clone().unwrap())
        } else {
            Stdio::null()
        })
        .stdout(slave.try_clone().unwrap())
        .stderr(slave);
    // SAFETY: setsid and ioctl change only the child's terminal state; no
    // allocation or shared locks are used between fork and exec.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(1, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    // Command retains its configured slave descriptors after spawn.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    master.write_all(answer).unwrap();
    let reader = std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            match master.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => output.extend_from_slice(&buffer[..n]),
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => panic!("read terminal: {error}"),
            }
        }
        String::from_utf8_lossy(&output).into_owned()
    });
    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break (status, false);
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            break (child.wait().unwrap(), true);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let output = reader.join().unwrap();
    assert!(
        !timed_out,
        "command did not finish within {timeout:?} on a terminal: {output}"
    );
    (status, output)
}
