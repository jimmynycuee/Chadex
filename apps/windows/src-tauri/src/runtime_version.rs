use chadex_runtime_process::{ManagedChild, SpawnOptions};
use std::{
    io::Read,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub async fn probe(binary: PathBuf) -> Option<String> {
    // Metadata probes have the same owned-tree guarantees as the helper.
    // There is no detached `Command::output` process during desktop shutdown.
    tauri::async_runtime::spawn_blocking(move || {
        let mut command = Command::new(binary);
        command
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = ManagedChild::spawn_with_options(
            &mut command,
            SpawnOptions {
                windows_creation_flags: 0x0800_0000,
                windows_silent_child_breakaway: false,
            },
        )
        .ok()?;
        let stdout = child.child_mut().stdout.take()?;
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(status) = child.try_wait().ok()? {
                if !status.success() || !child.try_tree_exit().ok()? {
                    return None;
                }
                let mut bytes = Vec::new();
                stdout.take(4097).read_to_end(&mut bytes).ok()?;
                if bytes.len() > 4096 {
                    return None;
                }
                return parse(&bytes);
            }
            if Instant::now() >= deadline {
                let _ = child.terminate_tree();
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    })
    .await
    .ok()
    .flatten()
}

fn parse(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut parts = text.split_whitespace();
    if parts.next()? != "chadex-runtime-cli" {
        return None;
    }
    let version = parts.next()?;
    if version.len() > 32 || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    Some(version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_version_output_is_never_exposed_as_diagnostics() {
        assert_eq!(
            parse(b"chadex-runtime-cli 0.3.2 (commit unknown)\n"),
            Some("0.3.2".into())
        );
        assert_eq!(parse(b"unexpected secret text"), None);
        assert_eq!(parse(b"chadex-runtime-cli credential-value"), None);
    }
}
