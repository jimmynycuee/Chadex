//! Runner side of the Computer Use cursor overlay channel (macOS only).
//!
//! The helper starts the runner with `CHADEX_COMPUTER_OVERLAY=stdout-v1` and a
//! fresh `CHADEX_COMPUTER_OVERLAY_TOKEN`. The runner never writes to stdout
//! otherwise, so the stdout pipe becomes a private one-way event channel:
//!
//! * At startup, before any thread exists, a private `CLOEXEC` duplicate of fd 1
//!   is taken and fd 1 is re-pointed at `/dev/null`. Child processes, stray
//!   `println!` calls and inherited stdio can no longer reach the channel.
//! * Both environment variables are removed so children do not inherit the
//!   token.
//! * Action threads only encode a line and `try_send` it. A dedicated writer
//!   thread owns the blocking `write_all`; if the helper stops reading, only
//!   that thread blocks, and if the pipe breaks the thread exits and every
//!   later `emit` is a cheap drop.
//!
//! See `docs/computer-use/cursor-overlay-design.md` sections 3, 4.4, 5.4, 8.5.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};
use webcodex_computer::{ComputerOverlayEvent, ComputerOverlaySink};

pub(crate) const ENV_CHANNEL: &str = "CHADEX_COMPUTER_OVERLAY";
pub(crate) const ENV_CHANNEL_VALUE: &str = "stdout-v1";
pub(crate) const ENV_TOKEN: &str = "CHADEX_COMPUTER_OVERLAY_TOKEN";
/// Bounded queue between action threads and the writer thread.
pub(crate) const CHANNEL_CAPACITY: usize = 32;
const TOKEN_HEX_LEN: usize = 32;

static INSTALLED_SINK: OnceLock<Arc<StdoutOverlaySink>> = OnceLock::new();

/// Validated channel settings read from the environment.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OverlayChannelConfig {
    pub(crate) token: String,
}

/// Pure decision: the channel is enabled only when the channel marker is exactly
/// `stdout-v1` and the token is 32 lowercase hex characters (128 bits).
pub(crate) fn channel_config(get: impl Fn(&str) -> Option<String>) -> Option<OverlayChannelConfig> {
    if get(ENV_CHANNEL).as_deref() != Some(ENV_CHANNEL_VALUE) {
        return None;
    }
    let token = get(ENV_TOKEN)?;
    let valid = token.len() == TOKEN_HEX_LEN
        && token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    valid.then_some(OverlayChannelConfig { token })
}

/// Take a private `CLOEXEC` copy of `stdout_fd` and point `stdout_fd` at
/// `/dev/null`. Returns the private channel. On error nothing is changed except
/// possibly an extra private fd that is closed on drop.
pub(crate) fn isolate_stdout_channel(stdout_fd: RawFd) -> io::Result<File> {
    // F_DUPFD_CLOEXEC with a floor of 3 never lands on stdio and is not
    // inherited across exec.
    let duplicated = unsafe { libc::fcntl(stdout_fd, libc::F_DUPFD_CLOEXEC, 3) };
    if duplicated < 0 {
        return Err(io::Error::last_os_error());
    }
    let channel = unsafe { File::from_raw_fd(duplicated) };
    let null = OpenOptions::new().write(true).open("/dev/null")?;
    // dup2 clears CLOEXEC on the target, so ordinary stdio inheritance keeps
    // working (children simply write into /dev/null).
    if unsafe { libc::dup2(null.as_raw_fd(), stdout_fd) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(channel)
}

/// Non-blocking overlay sink backed by a bounded queue and one writer thread.
pub(crate) struct StdoutOverlaySink {
    tx: SyncSender<Vec<u8>>,
    token: String,
    /// Serializes seq allocation with `try_send` so lines are queued in seq order.
    seq: Mutex<u64>,
    dropped: AtomicU64,
    /// Kept so tests can observe writer shutdown; production never joins it.
    #[cfg_attr(not(test), allow(dead_code))]
    writer: Mutex<Option<JoinHandle<()>>>,
}

impl StdoutOverlaySink {
    pub(crate) fn spawn<W: Write + Send + 'static>(writer: W, token: String) -> io::Result<Self> {
        Self::spawn_with_capacity(writer, token, CHANNEL_CAPACITY)
    }

    pub(crate) fn spawn_with_capacity<W: Write + Send + 'static>(
        mut writer: W,
        token: String,
        capacity: usize,
    ) -> io::Result<Self> {
        let (tx, rx) = sync_channel::<Vec<u8>>(capacity);
        let handle = std::thread::Builder::new()
            .name("chadex-computer-overlay".to_string())
            .spawn(move || {
                for line in rx {
                    if writer.write_all(&line).is_err() || writer.flush().is_err() {
                        break;
                    }
                }
                // Dropping `rx` here makes every later `try_send` report Disconnected.
            })?;
        Ok(Self {
            tx,
            token,
            seq: Mutex::new(0),
            dropped: AtomicU64::new(0),
            writer: Mutex::new(Some(handle)),
        })
    }

    /// Events dropped because the queue was full, the writer was gone or the
    /// line could not be encoded within the size bound.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn writer_finished(&self) -> bool {
        self.writer
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(JoinHandle::is_finished))
            .unwrap_or(true)
    }

    #[cfg(test)]
    pub(crate) fn join_writer(&self) {
        let handle = self.writer.lock().ok().and_then(|mut guard| guard.take());
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

impl ComputerOverlaySink for StdoutOverlaySink {
    fn emit(&self, event: ComputerOverlayEvent) {
        let mut seq = match self.seq.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *seq += 1;
        let Some(line) = event.encode_line(&self.token, *seq, now_unix_ms()) else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        match self.tx.try_send(line) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

/// Called first thing in `run_cli`, before any thread is created (the
/// environment mutation below is only sound while the process is single
/// threaded). Returns whether the overlay channel is active.
///
/// Both environment variables are always removed so no child process inherits
/// the token, even when the channel ends up disabled.
pub(crate) fn install_from_process_env() -> bool {
    let config = channel_config(|name| std::env::var(name).ok());
    std::env::remove_var(ENV_CHANNEL);
    std::env::remove_var(ENV_TOKEN);
    let Some(config) = config else {
        return false;
    };
    let Ok(channel) = isolate_stdout_channel(1) else {
        return false;
    };
    let Ok(sink) = StdoutOverlaySink::spawn(channel, config.token) else {
        return false;
    };
    INSTALLED_SINK.set(Arc::new(sink)).is_ok()
}

/// The sink installed by [`install_from_process_env`], if any.
pub(crate) fn installed_sink() -> Option<Arc<dyn ComputerOverlaySink>> {
    INSTALLED_SINK
        .get()
        .map(|sink| Arc::clone(sink) as Arc<dyn ComputerOverlaySink>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Read;
    use std::os::fd::IntoRawFd;
    use std::process::Command;
    use std::time::{Duration, Instant};
    use webcodex_computer::{OverlayAction, OverlayOutcome, OverlayTarget};

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn will_act() -> ComputerOverlayEvent {
        ComputerOverlayEvent::WillAct {
            action_id: 1,
            action: OverlayAction::Click,
            target: OverlayTarget::point(10.0, 20.0),
            display: None,
            key: None,
            ttl_ms: 2000,
        }
    }

    fn pipe() -> (File, File) {
        let mut fds = [0 as RawFd; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe {
            (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1]))
        }
    }

    #[test]
    fn channel_config_requires_exact_marker_and_hex_token() {
        assert_eq!(
            channel_config(env_of(&[(ENV_CHANNEL, "stdout-v1"), (ENV_TOKEN, TOKEN)])),
            Some(OverlayChannelConfig {
                token: TOKEN.to_string()
            })
        );
        for pairs in [
            vec![],
            vec![(ENV_TOKEN, TOKEN)],
            vec![(ENV_CHANNEL, "stdout-v1")],
            vec![(ENV_CHANNEL, "stdout-v2"), (ENV_TOKEN, TOKEN)],
            vec![(ENV_CHANNEL, "1"), (ENV_TOKEN, TOKEN)],
            vec![(ENV_CHANNEL, "stdout-v1"), (ENV_TOKEN, "")],
            vec![(ENV_CHANNEL, "stdout-v1"), (ENV_TOKEN, "abc")],
            vec![
                (ENV_CHANNEL, "stdout-v1"),
                (ENV_TOKEN, "0123456789ABCDEF0123456789ABCDEF"),
            ],
            vec![
                (ENV_CHANNEL, "stdout-v1"),
                (ENV_TOKEN, "0123456789abcdef0123456789abcdeg"),
            ],
            vec![
                (ENV_CHANNEL, "stdout-v1"),
                (ENV_TOKEN, "0123456789abcdef0123456789abcdef0"),
            ],
        ] {
            assert_eq!(channel_config(env_of(&pairs)), None, "{pairs:?}");
        }
    }

    #[test]
    fn sink_writes_one_json_line_per_event_in_sequence() {
        let (mut reader, writer) = pipe();
        let sink = StdoutOverlaySink::spawn(writer, TOKEN.to_string()).unwrap();
        sink.emit(will_act());
        sink.emit(ComputerOverlayEvent::Finished {
            action_id: 1,
            outcome: OverlayOutcome::Succeeded,
        });
        // Dropping the sender closes the queue; the writer drains it and exits,
        // which closes the pipe's write end and lets us read to EOF.
        let StdoutOverlaySink { tx, writer, .. } = sink;
        drop(tx);
        writer.lock().unwrap().take().unwrap().join().unwrap();

        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(first["phase"], "will_act");
        assert_eq!(first["token"], TOKEN);
        assert_eq!(first["seq"], 1);
        assert_eq!(second["phase"], "finished");
        assert_eq!(second["seq"], 2);
        assert_eq!(second["outcome"], "succeeded");
    }

    #[test]
    fn emit_never_blocks_when_nobody_reads_the_pipe() {
        let (_reader, writer) = pipe();
        let sink = StdoutOverlaySink::spawn(writer, TOKEN.to_string()).unwrap();
        let started = Instant::now();
        let mut slowest = Duration::ZERO;
        for _ in 0..10_000 {
            let call = Instant::now();
            sink.emit(will_act());
            slowest = slowest.max(call.elapsed());
        }
        let total = started.elapsed();
        assert!(sink.dropped() > 0, "a full queue must drop events");
        // Generous bounds: the contract is "does not wait for the peer"; a blocked
        // write would hang the loop for the full test timeout instead.
        assert!(slowest < Duration::from_millis(250), "slowest emit {slowest:?}");
        assert!(total < Duration::from_secs(5), "10k emits took {total:?}");
        assert!(!sink.writer_finished(), "writer should be stuck on the full pipe");
    }

    #[test]
    fn emit_stays_immediate_after_the_reader_goes_away() {
        let (reader, writer) = pipe();
        let sink = StdoutOverlaySink::spawn(writer, TOKEN.to_string()).unwrap();
        drop(reader);
        // First emit wakes the writer, which hits EPIPE and exits.
        sink.emit(will_act());
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sink.writer_finished() {
            assert!(Instant::now() < deadline, "writer did not stop after EPIPE");
            std::thread::sleep(Duration::from_millis(5));
        }
        let before = sink.dropped();
        let started = Instant::now();
        for _ in 0..1_000 {
            sink.emit(will_act());
        }
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(sink.dropped(), before + 1_000);
        sink.join_writer();
    }

    #[test]
    fn oversized_lines_are_dropped_not_sent() {
        let (_reader, writer) = pipe();
        let sink = StdoutOverlaySink::spawn(writer, "x".repeat(2000)).unwrap();
        sink.emit(will_act());
        assert_eq!(sink.dropped(), 1);
    }

    #[test]
    fn children_cannot_reach_the_channel_through_stdout_or_inheritance() {
        let (mut reader, writer) = pipe();
        // Stand-in for fd 1: any fd number >= 3 that currently points at the pipe.
        let fake_stdout = writer.try_clone().unwrap().into_raw_fd();
        let original = writer;
        let channel = isolate_stdout_channel(fake_stdout).unwrap();
        let channel_fd = channel.as_raw_fd();
        assert_ne!(channel_fd, fake_stdout);
        // Close every other writer so the channel is the only way into the pipe.
        drop(original);

        // A child that writes to the inherited "stdout" lands in /dev/null.
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("echo leak >&{fake_stdout}"))
            .status()
            .unwrap();
        assert!(status.success(), "the redirected stdout must stay writable");
        // The private channel fd is not inherited at all.
        let status = Command::new("sh")
            .arg("-c")
            .arg(format!("echo leak2 >&{channel_fd}"))
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(!status.success(), "the channel fd must be CLOEXEC");

        // The channel itself still works.
        let mut channel = channel;
        channel.write_all(b"ok\n").unwrap();
        drop(channel);
        unsafe { libc::close(fake_stdout) };
        let mut text = String::new();
        reader.read_to_string(&mut text).unwrap();
        assert_eq!(text, "ok\n");
    }

    #[test]
    fn isolating_an_invalid_fd_fails_without_side_effects() {
        assert!(isolate_stdout_channel(987_654).is_err());
    }
}
