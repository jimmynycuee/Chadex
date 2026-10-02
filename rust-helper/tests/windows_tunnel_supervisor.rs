#![cfg(windows)]

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
};

struct TestChild(Child);

impl Drop for TestChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn ps_quote(path: &Path) -> String {
    path.to_string_lossy().replace('\'', "''")
}

fn script(path: &Path, contents: &str) {
    // Windows PowerShell 5.1 needs the BOM for non-ASCII fixture paths.
    std::fs::write(path, format!("\u{feff}{contents}")).unwrap();
}

fn supervisor(script: &Path) -> TestChild {
    TestChild(Command::new(env!("CARGO_BIN_EXE_chadex-helper"))
        .args(["--supervise-tunnel", "powershell.exe", "-NoProfile", "-NonInteractive",
            "-ExecutionPolicy", "Bypass", "-File"])
        .arg(script)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn().unwrap())
}

fn wait(child: &mut TestChild) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            return status;
        }
        assert!(Instant::now() < deadline, "supervisor did not finish");
        thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() { return false; }
    let mut exit_code = 0;
    let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
    unsafe { CloseHandle(handle) };
    assert_ne!(ok, 0, "could not query owned process");
    exit_code == 259
}

fn await_pids(marker: &Path) -> Vec<u32> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if let Ok(contents) = std::fs::read_to_string(marker) {
            if let Ok(pids) = contents.trim().split_whitespace().map(str::parse).collect::<Result<Vec<u32>, _>>() {
                if pids.len() == 2 && pids.iter().all(|pid| alive(*pid)) { return pids; }
            }
        }
        assert!(Instant::now() < deadline, "tunnel and descendant never became ready");
        thread::sleep(Duration::from_millis(20));
    }
}

fn assert_dead(pids: &[u32]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while pids.iter().any(|pid| alive(*pid)) {
        assert!(Instant::now() < deadline, "owned tunnel process survived lease revocation");
        thread::sleep(Duration::from_millis(20));
    }
}

fn tree_fixture(directory: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let leaf = directory.join("descendant 中文.ps1");
    script(&leaf, "Start-Sleep -Seconds 120");
    let marker = directory.join("ready.txt");
    let tunnel = directory.join("tunnel with spaces.ps1");
    script(&tunnel, &format!(
        "$child = Start-Process powershell.exe -ArgumentList '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"' -PassThru; [IO.File]::WriteAllText('{}', \"$PID $($child.Id)\"); Wait-Process -Id $child.Id",
        ps_quote(&leaf), ps_quote(&marker)));
    (tunnel, marker)
}

#[test]
fn windows_tunnel_exit_propagates_without_parent_eof() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = directory.path().join("exit.ps1");
    script(&fixture, "exit 7");
    let mut child = supervisor(&fixture);
    assert_eq!(wait(&mut child).code(), Some(7));
}

#[test]
fn windows_pipe_eof_terminates_tunnel_and_descendants() {
    let directory = tempfile::tempdir().unwrap();
    let (fixture, marker) = tree_fixture(directory.path());
    let mut child = supervisor(&fixture);
    let pids = await_pids(&marker);
    child.0.stdin.as_mut().unwrap().write_all(b"alive").unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    drop(child.0.stdin.take());
    assert!(wait(&mut child).success());
    assert_dead(&pids);
}

#[test]
fn windows_abrupt_owner_death_revokes_tunnel_lease() {
    let directory = tempfile::tempdir().unwrap();
    let (fixture, marker) = tree_fixture(directory.path());
    let owner_script = directory.path().join("owner.ps1");
    script(&owner_script, &format!(
        "$info = New-Object Diagnostics.ProcessStartInfo; $info.FileName = '{}'; $info.Arguments = '--supervise-tunnel powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File \"{}\"'; $info.UseShellExecute = $false; $info.RedirectStandardInput = $true; $process = [Diagnostics.Process]::Start($info); Start-Sleep -Seconds 120",
        ps_quote(Path::new(env!("CARGO_BIN_EXE_chadex-helper"))), ps_quote(&fixture)));
    let mut owner = TestChild(Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(owner_script).stdout(Stdio::null()).spawn().unwrap());
    let pids = await_pids(&marker);
    owner.0.kill().unwrap();
    owner.0.wait().unwrap();
    assert_dead(&pids);
}
