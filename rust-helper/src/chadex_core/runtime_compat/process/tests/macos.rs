use crate::chadex_core::runtime_compat::activity::ActivityLog;
use crate::chadex_core::runtime_compat::process::{ProcessKind, ProcessPhase, ProcessSupervisor};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::process::{Child, Command as TokioCommand};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

fn unique_marker(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "chadex-runtime-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ))
}

async fn wait_for_pid(path: &Path) -> u32 {
    let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
    loop {
        if let Some(pid) = std::fs::read_to_string(path)
            .ok()
            .filter(|contents| contents.ends_with('\n'))
            .and_then(|contents| contents.trim().parse::<u32>().ok())
            .filter(|pid| *pid > 1 && i32::try_from(*pid).is_ok())
        {
            return pid;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for a complete PID record in {}",
            path.display()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn pid_marker_requires_a_complete_record() {
    let marker = unique_marker("partial-pid-record");
    let waiting = wait_for_pid(&marker);
    tokio::pin!(waiting);

    for incomplete in ["", "12"] {
        std::fs::write(&marker, incomplete).unwrap();
        assert!(tokio::time::timeout(POLL_INTERVAL * 2, &mut waiting)
            .await
            .is_err());
    }

    std::fs::write(&marker, "123\n").unwrap();
    assert_eq!(
        tokio::time::timeout(TEST_TIMEOUT, waiting).await.unwrap(),
        123
    );
    let _ = std::fs::remove_file(&marker);
}

fn process_exists(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    !matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    )
}

async fn wait_for_process_exit(pid: u32) {
    let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
    loop {
        if !process_exists(pid) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "PID {pid} did not exit before deadline"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn stop_control(mut child: Child) {
    let _ = child.start_kill();
    let _ = tokio::time::timeout(TEST_TIMEOUT, child.wait()).await;
}

#[tokio::test]
async fn owned_group_reclaims_descendants_without_touching_unrelated_process() {
    let marker = unique_marker("process-group-descendant");
    let mut owned = Command::new("/bin/sh");
    owned
        .arg("-c")
        .arg("sleep 60 & descendant=$!; printf '%s\\n' \"$descendant\" > \"$1\"; wait \"$descendant\"")
        .arg("chadex-owned-tree")
        .arg(&marker);

    let mut control = TokioCommand::new("/bin/sleep").arg("60").spawn().unwrap();
    let control_pid = control.id().unwrap();

    let mut supervisor = ProcessSupervisor::new(ActivityLog::default());
    supervisor
        .spawn_owned(ProcessKind::LocalServer, owned, false)
        .await
        .unwrap();
    let root_pid = supervisor
        .snapshot(ProcessKind::LocalServer)
        .and_then(|snapshot| snapshot.pid)
        .unwrap();

    let root_pid_i32 = i32::try_from(root_pid).unwrap();
    assert_eq!(unsafe { libc::getpgid(root_pid_i32) }, root_pid_i32);

    let descendant_pid = wait_for_pid(&marker).await;
    assert!(process_exists(descendant_pid));
    assert!(process_exists(control_pid));

    supervisor.stop(ProcessKind::LocalServer).await;

    wait_for_process_exit(root_pid).await;
    wait_for_process_exit(descendant_pid).await;
    assert!(process_exists(control_pid));
    assert!(control.try_wait().unwrap().is_none());

    stop_control(control).await;
    let _ = std::fs::remove_file(marker);
}

#[tokio::test]
async fn respawn_reclaims_descendants_from_terminal_previous_generation() {
    let marker = unique_marker("terminal-generation-descendant");
    let mut previous = Command::new("/bin/sh");
    previous
        .arg("-c")
        .arg("nohup sleep 60 >/dev/null 2>&1 & descendant=$!; printf '%s\\n' \"$descendant\" > \"$1\"; exit 0")
        .arg("chadex-terminal-generation")
        .arg(&marker);

    let mut supervisor = ProcessSupervisor::new(ActivityLog::default());
    supervisor
        .spawn_owned(ProcessKind::LocalServer, previous, false)
        .await
        .unwrap();
    let descendant_pid = wait_for_pid(&marker).await;
    assert!(process_exists(descendant_pid));

    let deadline = tokio::time::Instant::now() + TEST_TIMEOUT;
    loop {
        let terminal = supervisor
            .snapshot(ProcessKind::LocalServer)
            .is_some_and(|snapshot| {
                matches!(snapshot.phase, ProcessPhase::Exited | ProcessPhase::Failed)
            });
        if terminal {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    let mut replacement = Command::new("/bin/sleep");
    replacement.arg("60");
    supervisor
        .spawn_owned(ProcessKind::LocalServer, replacement, false)
        .await
        .unwrap();

    wait_for_process_exit(descendant_pid).await;
    supervisor.stop(ProcessKind::LocalServer).await;
    let _ = std::fs::remove_file(marker);
}

#[tokio::test]
async fn regular_tunnel_stop_delivers_stdin_eof_before_group_termination() {
    let marker = unique_marker("regular-tunnel-eof");
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("cat >/dev/null; printf 'eof\\n' > \"$1\"")
        .arg("chadex-tunnel-eof")
        .arg(&marker);

    let mut supervisor = ProcessSupervisor::new(ActivityLog::default());
    supervisor
        .spawn_owned(ProcessKind::RegularTunnel, command, false)
        .await
        .unwrap();
    supervisor.stop(ProcessKind::RegularTunnel).await;

    assert!(marker.is_file());
    let _ = std::fs::remove_file(marker);
}
