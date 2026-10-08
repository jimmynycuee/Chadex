//! Process-level checks for the cursor overlay stdout channel. These run the real
//! runner binary but only on its config-error path: no Computer action, no screen
//! access.

#![cfg(target_os = "macos")]

use std::process::{Command, Stdio};

const RUNNER: &str = env!("CARGO_BIN_EXE_chadex-runtime-runner");
const TOKEN: &str = "0123456789abcdef0123456789abcdef";

fn run_with_missing_config(extra_env: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(RUNNER);
    command
        .args(["--once", "--config", "/nonexistent/chadex-overlay-test.toml"])
        .env_remove("CHADEX_COMPUTER_OVERLAY")
        .env_remove("CHADEX_COMPUTER_OVERLAY_TOKEN")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in extra_env {
        command.env(key, value);
    }
    command.output().expect("spawn runner")
}

#[test]
fn runner_stdout_stays_empty_without_overlay_environment() {
    let output = run_with_missing_config(&[]);
    assert!(!output.status.success());
    assert!(
        output.stdout.is_empty(),
        "stdout must stay unused: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to read config"));
}

#[test]
fn runner_stdout_carries_nothing_but_events_when_the_channel_is_enabled() {
    let output = run_with_missing_config(&[
        ("CHADEX_COMPUTER_OVERLAY", "stdout-v1"),
        ("CHADEX_COMPUTER_OVERLAY_TOKEN", TOKEN),
    ]);
    assert!(!output.status.success());
    // No Computer action ran, so no event; nothing else may appear either. Logs and
    // errors must have stayed on stderr.
    assert!(
        output.stdout.is_empty(),
        "unexpected stdout: {:?}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("failed to read config"));
}

#[test]
fn invalid_overlay_environment_leaves_the_runner_working_and_stdout_unused() {
    let output = run_with_missing_config(&[
        ("CHADEX_COMPUTER_OVERLAY", "stdout-v1"),
        ("CHADEX_COMPUTER_OVERLAY_TOKEN", "not-a-token"),
    ]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}
