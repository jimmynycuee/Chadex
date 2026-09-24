#![cfg(unix)]

use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn wait(mut child: Child) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "supervisor did not finish");
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn tunnel_exit_propagates_without_waiting_for_parent_eof() {
    let child = Command::new(env!("CARGO_BIN_EXE_chadex-helper"))
        .args(["--supervise-tunnel", "/bin/sh", "-c", "exit 7"])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    assert_eq!(wait(child).code(), Some(7));
}

#[test]
fn pipe_eof_terminates_tunnel_and_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let marker = directory.path().join("escaped");
    let ready = directory.path().join("ready");
    let mut child = Command::new(env!("CARGO_BIN_EXE_chadex-helper"))
        .args([
            "--supervise-tunnel",
            "/bin/sh",
            "-c",
            "(sleep 1; touch \"$1\") & touch \"$2\"; wait",
            "fixture",
        ])
        .arg(&marker)
        .arg(&ready)
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    // Lease traffic is not a shutdown request; only EOF revokes the lease.
    child.stdin.as_mut().unwrap().write_all(b"alive").unwrap();
    assert!(child.try_wait().unwrap().is_none());
    drop(child.stdin.take());
    assert!(wait(child).success());
    thread::sleep(Duration::from_millis(1100));
    assert!(!marker.exists(), "descendant survived lease revocation");
}

#[test]
fn abrupt_owner_death_closes_lease() {
    let directory = tempfile::tempdir().unwrap();
    let ready = directory.path().join("ready");
    let marker = directory.path().join("escaped");
    // The owner is killed without destructors. Its kernel-owned pipe closes,
    // and the independently running supervisor must still reap the tree.
    let mut owner = Command::new("python3")
        .args(["-c", "import subprocess,sys,time; p=subprocess.Popen([sys.argv[1], '--supervise-tunnel', '/bin/sh', '-c', '(sleep 1; touch \"$1\") & touch \"$2\"; wait', 'fixture',sys.argv[2],sys.argv[3]],stdin=subprocess.PIPE); time.sleep(30)"])
        .arg(env!("CARGO_BIN_EXE_chadex-helper"))
        .arg(&marker)
        .arg(&ready)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(10));
    }
    owner.kill().unwrap();
    owner.wait().unwrap();
    thread::sleep(Duration::from_millis(1200));
    assert!(!marker.exists(), "tunnel survived abrupt helper death");
}
