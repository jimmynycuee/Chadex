//! Own the tunnel tree behind a pipe lease. `kill_on_drop` alone cannot run
//! after SIGKILL or a helper crash; EOF still reaches this separate supervisor.

use chadex_runtime_process::ManagedChild;
use std::ffi::OsString;
use std::io::{self, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

pub fn run(mut args: impl Iterator<Item = OsString>) -> io::Result<i32> {
    let program = args
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing tunnel executable"))?;
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    let mut child = ManagedChild::spawn(&mut command)?;
    let (closed, lease) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 256];
        let mut stdin = io::stdin().lock();
        loop {
            match stdin.read(&mut buffer) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        let _ = closed.send(());
    });
    loop {
        if let Some(status) = child.try_wait()? {
            // ManagedChild also closes any remaining descendants on drop.
            return Ok(status.code().unwrap_or(1));
        }
        match lease.recv_timeout(Duration::from_millis(50)) {
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                child.terminate_tree()?;
                child.wait()?;
                return Ok(0);
            }
        }
    }
}
