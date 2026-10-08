//! Helper side of the Computer Use cursor overlay (see
//! `docs/computer-use/cursor-overlay-design.md`, sections 2, 3.5, 4.4, 6).
//!
//! The local Runner writes overlay events to its stdout pipe. The process
//! supervisor drains that pipe into a bounded `MachineEventReceiver`; this hub
//! validates every event (fail closed), drops anything the App has not asked
//! for, and forwards the rest to the bridge writer through a bounded channel.
//!
//! Nothing here may block the Runner, the MCP ingress or a bridge request:
//! the forwarder only ever `try_send`s, and an App that stops reading costs
//! counters, not latency.

use crate::chadex_core::runtime_compat::process::MachineEventReceiver;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub(crate) const OVERLAY_EVENT_NAME: &str = "computer_overlay";
pub(crate) const OVERLAY_SCHEMA: &str = "chadex.computer_overlay.v1";
/// Environment contract with `chadex-runtime-runner` (see its `computer_overlay.rs`).
pub(crate) const ENV_CHANNEL: &str = "CHADEX_COMPUTER_OVERLAY";
pub(crate) const ENV_CHANNEL_VALUE: &str = "stdout-v1";
pub(crate) const ENV_TOKEN: &str = "CHADEX_COMPUTER_OVERLAY_TOKEN";

/// Longest accepted runner line, newline included.
const MAX_EVENT_BYTES: usize = 1024;
/// Frames waiting for the bridge stdout writer.
pub(crate) const OUT_CAPACITY: usize = 32;
const MAX_COORDINATE: f64 = 100_000.0;
const MAX_EXTENT: f64 = 32_768.0;
const MAX_TTL_MS: u32 = 10_000;
const MAX_KEY_MODIFIERS: usize = 4;
/// `v` field of helper -> App frames.
const FRAME_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Wire model (runner -> helper). Closed vocabularies, no unknown fields.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    WillAct,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Move,
    Click,
    Press,
    Focus,
    Scroll,
    Input,
    Key,
    Activate,
    // Reserved: the runtime has no such actions yet, the App must handle them.
    Drag,
    Wheel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Space {
    MacosCgGlobalPt,
    WindowsVirtualScreenPx,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Succeeded,
    Failed,
    NotStarted,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum KeyName {
    Enter,
    Escape,
    Tab,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    PageUp,
    PageDown,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Modifier {
    Shift,
    Control,
    Option,
    Command,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Key {
    name: KeyName,
    modifiers: Vec<Modifier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Target {
    Point {
        x: f64,
        y: f64,
    },
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    Path {
        from: Point,
        to: Point,
    },
    #[serde(rename = "none")]
    Nothing,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bounds {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Display {
    id: u32,
    bounds: Bounds,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEvent {
    event: String,
    schema: String,
    token: String,
    seq: u64,
    phase: Phase,
    action_id: u64,
    #[serde(default)]
    action: Option<Action>,
    #[serde(default)]
    target: Option<Target>,
    #[serde(default)]
    space: Option<Space>,
    #[serde(default)]
    display: Option<Display>,
    #[serde(default)]
    key: Option<Key>,
    #[serde(default)]
    ttl_ms: Option<u32>,
    #[serde(default)]
    outcome: Option<Outcome>,
    emitted_at_ms: u64,
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Why a runner line was refused. Each variant has its own counter bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DropReason {
    /// Not JSON of the expected shape, unknown field, or unknown enum value.
    Malformed,
    /// Right shape but wrong `event`/`schema`, or fields that do not fit the phase.
    Schema,
    Token,
    Range,
    Sequence,
    Pairing,
    TooLarge,
}

/// Per-runner validation state: one instance lives as long as one runner process.
pub(crate) struct RunnerStream {
    token: String,
    last_seq: Option<u64>,
    open_action: Option<u64>,
}

impl RunnerStream {
    pub(crate) fn new(token: String) -> Self {
        Self {
            token,
            last_seq: None,
            open_action: None,
        }
    }

    /// After a machine-event overflow some events are gone, so an open action
    /// can no longer be paired.
    fn forget_open_action(&mut self) {
        self.open_action = None;
    }

    /// Validate one runner event and return the helper -> App `data` object.
    /// State only advances for accepted events.
    pub(crate) fn validate(&mut self, value: &Value) -> Result<Value, DropReason> {
        let encoded_len = serde_json::to_vec(value)
            .map(|bytes| bytes.len() + 1)
            .map_err(|_| DropReason::Malformed)?;
        if encoded_len > MAX_EVENT_BYTES {
            return Err(DropReason::TooLarge);
        }
        let event: RawEvent =
            serde_json::from_value(value.clone()).map_err(|_| DropReason::Malformed)?;
        if event.event != OVERLAY_EVENT_NAME || event.schema != OVERLAY_SCHEMA {
            return Err(DropReason::Schema);
        }
        if event.token != self.token {
            return Err(DropReason::Token);
        }
        check_phase_shape(&event)?;
        check_ranges(&event)?;
        if self.last_seq.is_some_and(|last| event.seq <= last) {
            return Err(DropReason::Sequence);
        }
        if event.phase == Phase::Finished && self.open_action != Some(event.action_id) {
            return Err(DropReason::Pairing);
        }

        let data = frame_data(&event);
        self.last_seq = Some(event.seq);
        match event.phase {
            Phase::WillAct => self.open_action = Some(event.action_id),
            Phase::Finished => self.open_action = None,
        }
        Ok(data)
    }
}

fn check_phase_shape(event: &RawEvent) -> Result<(), DropReason> {
    match event.phase {
        Phase::WillAct => {
            if event.action.is_none()
                || event.target.is_none()
                || event.space.is_none()
                || event.ttl_ms.is_none()
                || event.outcome.is_some()
            {
                return Err(DropReason::Schema);
            }
            if event.key.is_some() && event.action != Some(Action::Key) {
                return Err(DropReason::Schema);
            }
        }
        Phase::Finished => {
            if event.outcome.is_none()
                || event.action.is_some()
                || event.target.is_some()
                || event.space.is_some()
                || event.display.is_some()
                || event.key.is_some()
                || event.ttl_ms.is_some()
            {
                return Err(DropReason::Schema);
            }
        }
    }
    Ok(())
}

fn coordinate_ok(value: f64) -> bool {
    value.is_finite() && value.abs() <= MAX_COORDINATE
}

fn extent_ok(value: f64) -> bool {
    value.is_finite() && value > 0.0 && value <= MAX_EXTENT
}

fn check_ranges(event: &RawEvent) -> Result<(), DropReason> {
    let ok = match event.target {
        None | Some(Target::Nothing) => true,
        Some(Target::Point { x, y }) => coordinate_ok(x) && coordinate_ok(y),
        Some(Target::Rect {
            x,
            y,
            width,
            height,
        }) => coordinate_ok(x) && coordinate_ok(y) && extent_ok(width) && extent_ok(height),
        Some(Target::Path { from, to }) => {
            coordinate_ok(from.x)
                && coordinate_ok(from.y)
                && coordinate_ok(to.x)
                && coordinate_ok(to.y)
        }
    };
    let display_ok = event.display.is_none_or(|display| {
        let bounds = display.bounds;
        coordinate_ok(bounds.x)
            && coordinate_ok(bounds.y)
            && extent_ok(bounds.width)
            && extent_ok(bounds.height)
    });
    let ttl_ok = event.ttl_ms.is_none_or(|ttl| ttl <= MAX_TTL_MS);
    let key_ok = event
        .key
        .as_ref()
        .is_none_or(|key| key.modifiers.len() <= MAX_KEY_MODIFIERS);
    if ok && display_ok && ttl_ok && key_ok {
        Ok(())
    } else {
        Err(DropReason::Range)
    }
}

/// Helper -> App `data` object: the runner event minus `event`, `schema`, `token`.
fn frame_data(event: &RawEvent) -> Value {
    match event.phase {
        Phase::WillAct => {
            let mut data = json!({
                "v": FRAME_VERSION,
                "seq": event.seq,
                "phase": "will_act",
                "action_id": event.action_id,
                "action": event.action,
                "target": event.target,
                "space": event.space,
                "display": event.display,
                "ttl_ms": event.ttl_ms,
                "emitted_at_ms": event.emitted_at_ms,
            });
            if let Some(key) = &event.key {
                data["key"] = json!(key);
            }
            data
        }
        Phase::Finished => json!({
            "v": FRAME_VERSION,
            "seq": event.seq,
            "phase": "finished",
            "action_id": event.action_id,
            "outcome": event.outcome,
            "emitted_at_ms": event.emitted_at_ms,
        }),
    }
}

// ---------------------------------------------------------------------------
// Hub
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClearReason {
    Stopped,
    RunnerExited,
    Overflow,
    Disabled,
    SessionEnded,
}

impl ClearReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::RunnerExited => "runner_exited",
            Self::Overflow => "overflow",
            Self::Disabled => "disabled",
            Self::SessionEnded => "session_ended",
        }
    }
}

#[derive(Debug, Default)]
struct Counters {
    forwarded: AtomicU64,
    dropped_invalid: AtomicU64,
    dropped_disabled: AtomicU64,
    dropped_backpressure: AtomicU64,
    overflows: AtomicU64,
}

/// In-memory diagnostics. Never contains coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct OverlayCounters {
    pub(crate) forwarded: u64,
    pub(crate) dropped_invalid: u64,
    pub(crate) dropped_disabled: u64,
    pub(crate) dropped_backpressure: u64,
    pub(crate) overflows: u64,
}

/// Where overlay events come from. The supervisor's `MachineEventReceiver` in
/// production; a plain channel in tests.
pub(crate) trait OverlayEventSource: Send + 'static {
    fn recv(&mut self) -> impl Future<Output = Option<Value>> + Send;
}

impl OverlayEventSource for MachineEventReceiver {
    fn recv(&mut self) -> impl Future<Output = Option<Value>> + Send {
        MachineEventReceiver::recv(self)
    }
}

impl OverlayEventSource for mpsc::Receiver<Value> {
    fn recv(&mut self) -> impl Future<Output = Option<Value>> + Send {
        mpsc::Receiver::recv(self)
    }
}

pub(crate) struct ComputerOverlayHub {
    enabled: AtomicBool,
    out: mpsc::Sender<Value>,
    current: Mutex<Option<JoinHandle<()>>>,
    counters: Counters,
}

impl ComputerOverlayHub {
    /// The receiver feeds the bridge's stdout event writer.
    pub(crate) fn new() -> (Arc<Self>, mpsc::Receiver<Value>) {
        Self::with_capacity(OUT_CAPACITY)
    }

    pub(crate) fn with_capacity(capacity: usize) -> (Arc<Self>, mpsc::Receiver<Value>) {
        let (out, rx) = mpsc::channel(capacity);
        (
            Arc::new(Self {
                enabled: AtomicBool::new(false),
                out,
                current: Mutex::new(None),
                counters: Counters::default(),
            }),
            rx,
        )
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub(crate) fn counters(&self) -> OverlayCounters {
        OverlayCounters {
            forwarded: self.counters.forwarded.load(Ordering::Relaxed),
            dropped_invalid: self.counters.dropped_invalid.load(Ordering::Relaxed),
            dropped_disabled: self.counters.dropped_disabled.load(Ordering::Relaxed),
            dropped_backpressure: self.counters.dropped_backpressure.load(Ordering::Relaxed),
            overflows: self.counters.overflows.load(Ordering::Relaxed),
        }
    }

    /// Turn forwarding on or off. Turning it off sends one `clear(disabled)` and
    /// then discards every runner event (the pipe keeps being drained).
    pub(crate) fn set_enabled(&self, enabled: bool) {
        let was_enabled = self.enabled.swap(enabled, Ordering::SeqCst);
        if was_enabled && !enabled {
            self.send_clear(ClearReason::Disabled);
        }
    }

    /// Tell the App to hide the overlay. Silent while the App has not enabled events.
    pub(crate) fn clear(&self, reason: ClearReason) {
        if self.is_enabled() {
            self.send_clear(reason);
        }
    }

    fn send_clear(&self, reason: ClearReason) {
        let frame = json!({
            "v": FRAME_VERSION,
            "phase": "clear",
            "reason": reason.as_str(),
        });
        self.push_frame(frame);
    }

    fn forward(&self, data: Value) {
        if !self.is_enabled() {
            self.counters.dropped_disabled.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.push_frame(data);
    }

    fn push_frame(&self, frame: Value) {
        match self.out.try_send(frame) {
            Ok(()) => {
                self.counters.forwarded.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.counters
                    .dropped_backpressure
                    .fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Per-spawn private channel settings for the Runner command. Returns the
    /// token to hand to [`Self::attach_runner`]; `None` where the channel does
    /// not exist (non-macOS) or the system RNG fails (then no overlay).
    pub(crate) fn prepare_runner_command(&self, command: &mut Command) -> Option<String> {
        if !cfg!(target_os = "macos") {
            return None;
        }
        let token = new_channel_token()?;
        command.env(ENV_CHANNEL, ENV_CHANNEL_VALUE);
        command.env(ENV_TOKEN, &token);
        Some(token)
    }

    /// Start forwarding for a freshly spawned Runner. Replaces the previous
    /// forwarder and hides whatever the previous Runner left on screen.
    pub(crate) fn attach_runner<S: OverlayEventSource>(
        self: &Arc<Self>,
        source: S,
        token: String,
    ) {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(previous) = current.take() {
            previous.abort();
        }
        self.clear(ClearReason::RunnerExited);
        let hub = Arc::clone(self);
        *current = Some(tokio::spawn(async move {
            hub.forward_loop(source, token).await;
        }));
    }

    async fn forward_loop<S: OverlayEventSource>(self: Arc<Self>, mut source: S, token: String) {
        let mut stream = RunnerStream::new(token);
        while let Some(value) = source.recv().await {
            if value.get("event").and_then(Value::as_str) == Some("machine_event_overflow") {
                self.counters.overflows.fetch_add(1, Ordering::Relaxed);
                stream.forget_open_action();
                self.clear(ClearReason::Overflow);
                continue;
            }
            match stream.validate(&value) {
                Ok(data) => self.forward(data),
                Err(_) => {
                    self.counters.dropped_invalid.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.clear(ClearReason::RunnerExited);
    }

    /// `"attached"` while a forwarder for a live Runner exists.
    pub(crate) fn runner_channel(&self) -> &'static str {
        if !cfg!(target_os = "macos") {
            return "unsupported";
        }
        let attached = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .is_some_and(|handle| !handle.is_finished());
        if attached {
            "attached"
        } else {
            "detached"
        }
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_forwarder(&self) {
        let handle = self
            .current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(handle) = handle {
            let _ = handle.await;
        }
    }
}

impl Drop for ComputerOverlayHub {
    fn drop(&mut self) {
        if let Ok(current) = self.current.get_mut() {
            if let Some(handle) = current.take() {
                handle.abort();
            }
        }
    }
}

fn new_channel_token() -> Option<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

// ---------------------------------------------------------------------------
// Process-wide hub: the helper has exactly one stdout, so one hub. The bridge
// installs it at startup; the runtime coordinator looks it up when it spawns
// the Runner. Without an installed hub (unit tests) Runners are spawned with
// the old, overlay-free command line.
// ---------------------------------------------------------------------------

static SHARED_HUB: OnceLock<Arc<ComputerOverlayHub>> = OnceLock::new();

pub(crate) fn install_shared_hub(hub: Arc<ComputerOverlayHub>) {
    let _ = SHARED_HUB.set(hub);
}

pub(crate) fn shared_hub() -> Option<Arc<ComputerOverlayHub>> {
    SHARED_HUB.get().cloned()
}

/// The NDJSON line the bridge writes to stdout for one `data` object.
pub(crate) fn encode_event_frame(data: &Value) -> Vec<u8> {
    let mut line = serde_json::to_vec(&json!({
        "protocol_version": 1,
        "event": OVERLAY_EVENT_NAME,
        "data": data,
    }))
    .unwrap_or_default();
    line.push(b'\n');
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn will_act(seq: u64, action_id: u64) -> Value {
        json!({
            "event": "computer_overlay",
            "schema": "chadex.computer_overlay.v1",
            "token": TOKEN,
            "seq": seq,
            "phase": "will_act",
            "action_id": action_id,
            "action": "click",
            "target": {"kind": "point", "x": 812.5, "y": 433.0},
            "space": "macos_cg_global_pt",
            "display": {"id": 69733378, "bounds": {"x": 0.0, "y": 0.0, "width": 1512.0, "height": 982.0}},
            "ttl_ms": 2000,
            "emitted_at_ms": 1791500000123u64,
        })
    }

    fn finished(seq: u64, action_id: u64) -> Value {
        json!({
            "event": "computer_overlay",
            "schema": "chadex.computer_overlay.v1",
            "token": TOKEN,
            "seq": seq,
            "phase": "finished",
            "action_id": action_id,
            "outcome": "succeeded",
            "emitted_at_ms": 1791500000171u64,
        })
    }

    fn stream() -> RunnerStream {
        RunnerStream::new(TOKEN.to_string())
    }

    fn mutated(mut value: Value, f: impl FnOnce(&mut Value)) -> Value {
        f(&mut value);
        value
    }

    #[test]
    fn valid_events_become_documented_frames_without_secrets() {
        let mut stream = stream();
        let data = stream.validate(&will_act(1, 7)).unwrap();
        assert_eq!(
            data,
            json!({
                "v": 1,
                "seq": 1,
                "phase": "will_act",
                "action_id": 7,
                "action": "click",
                "target": {"kind": "point", "x": 812.5, "y": 433.0},
                "space": "macos_cg_global_pt",
                "display": {"id": 69733378, "bounds": {"x": 0.0, "y": 0.0, "width": 1512.0, "height": 982.0}},
                "ttl_ms": 2000,
                "emitted_at_ms": 1791500000123u64,
            })
        );
        assert!(data.get("token").is_none());
        assert!(data.get("event").is_none());
        assert!(data.get("schema").is_none());
        let data = stream.validate(&finished(2, 7)).unwrap();
        assert_eq!(
            data,
            json!({
                "v": 1,
                "seq": 2,
                "phase": "finished",
                "action_id": 7,
                "outcome": "succeeded",
                "emitted_at_ms": 1791500000171u64,
            })
        );
    }

    #[test]
    fn rect_none_null_display_and_key_pass_through() {
        let mut stream = stream();
        let event = mutated(will_act(1, 1), |event| {
            event["action"] = json!("key");
            event["target"] = json!({"kind": "rect", "x": 10.0, "y": 20.0, "width": 300.0, "height": 40.0});
            event["display"] = Value::Null;
            event["key"] = json!({"name": "enter", "modifiers": ["command", "shift"]});
        });
        let data = stream.validate(&event).unwrap();
        assert_eq!(data["display"], Value::Null);
        assert_eq!(data["key"], json!({"name": "enter", "modifiers": ["command", "shift"]}));
        assert_eq!(data["target"]["kind"], "rect");

        let none = mutated(will_act(2, 2), |event| {
            event["target"] = json!({"kind": "none"});
            event.as_object_mut().unwrap().remove("display");
        });
        assert_eq!(stream.validate(&none).unwrap()["target"], json!({"kind": "none"}));

        let path = mutated(will_act(3, 3), |event| {
            event["action"] = json!("drag");
            event["target"] = json!({"kind": "path", "from": {"x": 1.0, "y": 2.0}, "to": {"x": 3.0, "y": 4.0}});
        });
        assert_eq!(stream.validate(&path).unwrap()["action"], "drag");
    }

    #[test]
    fn invalid_events_are_dropped_with_the_right_reason() {
        let cases: Vec<(&str, Value, DropReason)> = vec![
            ("wrong token", mutated(will_act(1, 1), |e| e["token"] = json!("deadbeef")), DropReason::Token),
            ("wrong schema", mutated(will_act(1, 1), |e| e["schema"] = json!("chadex.computer_overlay.v2")), DropReason::Schema),
            ("wrong event", mutated(will_act(1, 1), |e| e["event"] = json!("progress")), DropReason::Schema),
            ("extra field", mutated(will_act(1, 1), |e| e["text"] = json!("hello")), DropReason::Malformed),
            ("unknown action", mutated(will_act(1, 1), |e| e["action"] = json!("teleport")), DropReason::Malformed),
            ("unknown phase", mutated(will_act(1, 1), |e| e["phase"] = json!("during")), DropReason::Malformed),
            ("unknown space", mutated(will_act(1, 1), |e| e["space"] = json!("linux_px")), DropReason::Malformed),
            ("unknown target kind", mutated(will_act(1, 1), |e| e["target"] = json!({"kind": "blob"})), DropReason::Malformed),
            ("target extra field", mutated(will_act(1, 1), |e| e["target"] = json!({"kind": "point", "x": 1.0, "y": 2.0, "label": "Save"})), DropReason::Malformed),
            ("display extra field", mutated(will_act(1, 1), |e| e["display"]["name"] = json!("Studio")), DropReason::Malformed),
            ("x out of range", mutated(will_act(1, 1), |e| e["target"]["x"] = json!(100_000.5)), DropReason::Range),
            ("y out of range", mutated(will_act(1, 1), |e| e["target"]["y"] = json!(-100_001.0)), DropReason::Range),
            ("zero width", mutated(will_act(1, 1), |e| e["target"] = json!({"kind": "rect", "x": 0.0, "y": 0.0, "width": 0.0, "height": 5.0})), DropReason::Range),
            ("huge height", mutated(will_act(1, 1), |e| e["target"] = json!({"kind": "rect", "x": 0.0, "y": 0.0, "width": 5.0, "height": 32_768.5})), DropReason::Range),
            ("display too small", mutated(will_act(1, 1), |e| e["display"]["bounds"]["width"] = json!(0.0)), DropReason::Range),
            ("ttl too long", mutated(will_act(1, 1), |e| e["ttl_ms"] = json!(10_001)), DropReason::Range),
            ("missing target", mutated(will_act(1, 1), |e| { e.as_object_mut().unwrap().remove("target"); }), DropReason::Schema),
            ("missing ttl", mutated(will_act(1, 1), |e| { e.as_object_mut().unwrap().remove("ttl_ms"); }), DropReason::Schema),
            ("will_act with outcome", mutated(will_act(1, 1), |e| e["outcome"] = json!("succeeded")), DropReason::Schema),
            ("key on a click", mutated(will_act(1, 1), |e| e["key"] = json!({"name": "enter", "modifiers": []})), DropReason::Schema),
            ("unknown key name", mutated(will_act(1, 1), |e| { e["action"] = json!("key"); e["key"] = json!({"name": "a", "modifiers": []}); }), DropReason::Malformed),
            ("unknown modifier", mutated(will_act(1, 1), |e| { e["action"] = json!("key"); e["key"] = json!({"name": "tab", "modifiers": ["hyper"]}); }), DropReason::Malformed),
            ("too many modifiers", mutated(will_act(1, 1), |e| { e["action"] = json!("key"); e["key"] = json!({"name": "tab", "modifiers": ["shift", "shift", "shift", "shift", "shift"]}); }), DropReason::Range),
            ("not an object", json!([1, 2, 3]), DropReason::Malformed),
            ("seq as string", mutated(will_act(1, 1), |e| e["seq"] = json!("1")), DropReason::Malformed),
            ("oversized", mutated(will_act(1, 1), |e| e["target"] = json!({"kind": "path", "from": {"x": 1.0, "y": 2.0}, "to": {"x": 3.0, "y": 4.0}}), ), DropReason::TooLarge),
        ];
        for (name, event, expected) in cases {
            let event = if name == "oversized" {
                // Padding in an allowed field would still be an unknown value; use a long token.
                mutated(event, |e| e["token"] = json!("a".repeat(1100)))
            } else {
                event
            };
            let mut stream = stream();
            assert_eq!(
                stream.validate(&event),
                Err(expected),
                "case {name}: {event}"
            );
            // A refused event must not advance any state.
            assert_eq!(stream.last_seq, None, "case {name}");
            assert_eq!(stream.open_action, None, "case {name}");
        }
    }

    #[test]
    fn finished_must_not_carry_will_act_fields() {
        for field in ["action", "target", "space", "display", "ttl_ms", "key"] {
            let mut stream = stream();
            stream.validate(&will_act(1, 9)).unwrap();
            let event = mutated(finished(2, 9), |e| {
                e[field] = match field {
                    "action" => json!("click"),
                    "target" => json!({"kind": "none"}),
                    "space" => json!("macos_cg_global_pt"),
                    "display" => json!({"id": 1, "bounds": {"x": 0.0, "y": 0.0, "width": 1.0, "height": 1.0}}),
                    "ttl_ms" => json!(10),
                    _ => json!({"name": "tab", "modifiers": []}),
                };
            });
            assert_eq!(stream.validate(&event), Err(DropReason::Schema), "{field}");
        }
        let mut stream = stream();
        stream.validate(&will_act(1, 9)).unwrap();
        let event = mutated(finished(2, 9), |e| {
            e.as_object_mut().unwrap().remove("outcome");
        });
        assert_eq!(stream.validate(&event), Err(DropReason::Schema));
    }

    #[test]
    fn sequence_must_increase_and_gaps_are_fine() {
        let mut stream = stream();
        stream.validate(&will_act(5, 1)).unwrap();
        assert_eq!(stream.validate(&finished(5, 1)), Err(DropReason::Sequence));
        assert_eq!(stream.validate(&finished(4, 1)), Err(DropReason::Sequence));
        // Seq 6..8 were dropped by the runner's bounded queue: still accepted.
        assert!(stream.validate(&finished(9, 1)).is_ok());
        assert_eq!(stream.validate(&will_act(9, 2)), Err(DropReason::Sequence));
        assert!(stream.validate(&will_act(10, 2)).is_ok());
    }

    #[test]
    fn finished_must_pair_with_the_latest_will_act() {
        let mut stream = stream();
        // finished without any will_act
        assert_eq!(stream.validate(&finished(1, 1)), Err(DropReason::Pairing));
        stream.validate(&will_act(2, 1)).unwrap();
        // wrong action id
        assert_eq!(stream.validate(&finished(3, 2)), Err(DropReason::Pairing));
        assert!(stream.validate(&finished(4, 1)).is_ok());
        // duplicate finished for an already closed action
        assert_eq!(stream.validate(&finished(5, 1)), Err(DropReason::Pairing));
        // a newer will_act supersedes an unfinished one
        stream.validate(&will_act(6, 2)).unwrap();
        stream.validate(&will_act(7, 3)).unwrap();
        assert_eq!(stream.validate(&finished(8, 2)), Err(DropReason::Pairing));
        assert!(stream.validate(&finished(9, 3)).is_ok());
    }

    #[test]
    fn forget_open_action_after_overflow_blocks_stale_finished() {
        let mut stream = stream();
        stream.validate(&will_act(1, 1)).unwrap();
        stream.forget_open_action();
        assert_eq!(stream.validate(&finished(2, 1)), Err(DropReason::Pairing));
    }

    /// Lines exactly as `ComputerOverlayEvent::encode_line` in chadex-runtime-computer
    /// writes them (keys sorted by serde_json). If the runtime's wire format changes,
    /// this and the runtime's golden tests must change together.
    #[test]
    fn lines_written_by_the_runtime_encoder_validate() {
        let lines = [
            r#"{"action":"click","action_id":1,"display":{"bounds":{"height":982.0,"width":1512.0,"x":0.0,"y":0.0},"id":69733378},"emitted_at_ms":1791500000000,"event":"computer_overlay","phase":"will_act","schema":"chadex.computer_overlay.v1","seq":1,"space":"macos_cg_global_pt","target":{"kind":"point","x":812.5,"y":433.0},"token":"0123456789abcdef0123456789abcdef","ttl_ms":2000}"#,
            r#"{"action_id":1,"emitted_at_ms":1791500000000,"event":"computer_overlay","outcome":"succeeded","phase":"finished","schema":"chadex.computer_overlay.v1","seq":2,"token":"0123456789abcdef0123456789abcdef"}"#,
            r#"{"action":"key","action_id":2,"display":null,"emitted_at_ms":1791500000000,"event":"computer_overlay","key":{"modifiers":["command","shift"],"name":"page_down"},"phase":"will_act","schema":"chadex.computer_overlay.v1","seq":3,"space":"macos_cg_global_pt","target":{"height":40.0,"kind":"rect","width":300.0,"x":10.0,"y":20.0},"token":"0123456789abcdef0123456789abcdef","ttl_ms":2000}"#,
            r#"{"action_id":2,"emitted_at_ms":1791500000000,"event":"computer_overlay","outcome":"unknown","phase":"finished","schema":"chadex.computer_overlay.v1","seq":4,"token":"0123456789abcdef0123456789abcdef"}"#,
            r#"{"action":"press","action_id":3,"display":null,"emitted_at_ms":1791500000000,"event":"computer_overlay","phase":"will_act","schema":"chadex.computer_overlay.v1","seq":5,"space":"macos_cg_global_pt","target":{"kind":"none"},"token":"0123456789abcdef0123456789abcdef","ttl_ms":2000}"#,
            r#"{"action_id":3,"emitted_at_ms":1791500000000,"event":"computer_overlay","outcome":"not_started","phase":"finished","schema":"chadex.computer_overlay.v1","seq":6,"token":"0123456789abcdef0123456789abcdef"}"#,
        ];
        let mut stream = stream();
        for line in lines {
            let value: Value = serde_json::from_str(line).unwrap();
            stream
                .validate(&value)
                .unwrap_or_else(|reason| panic!("{reason:?}: {line}"));
        }
    }

    #[test]
    fn event_frame_golden() {
        let data = json!({"v": 1, "phase": "clear", "reason": "stopped"});
        let line = encode_event_frame(&data);
        assert_eq!(
            String::from_utf8(line).unwrap(),
            "{\"data\":{\"phase\":\"clear\",\"reason\":\"stopped\",\"v\":1},\"event\":\"computer_overlay\",\"protocol_version\":1}\n"
        );
    }

    async fn next_frame(rx: &mut mpsc::Receiver<Value>) -> Value {
        tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("frame in time")
            .expect("channel open")
    }

    #[tokio::test]
    async fn nothing_is_forwarded_until_the_app_enables_events() {
        let (hub, mut rx) = ComputerOverlayHub::new();
        let (tx, source) = mpsc::channel(16);
        hub.attach_runner(source, TOKEN.to_string());
        tx.send(will_act(1, 1)).await.unwrap();
        tx.send(finished(2, 1)).await.unwrap();
        drop(tx);
        hub.wait_for_forwarder().await;
        assert!(rx.try_recv().is_err(), "disabled hub must stay silent");
        let counters = hub.counters();
        assert_eq!(counters.dropped_disabled, 2);
        assert_eq!(counters.forwarded, 0);
        // clear frames are silent too while disabled.
        hub.clear(ClearReason::Stopped);
        hub.set_enabled(false);
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn enabled_hub_forwards_validated_frames_and_counts_rejects() {
        let (hub, mut rx) = ComputerOverlayHub::new();
        hub.set_enabled(true);
        let (tx, source) = mpsc::channel(16);
        hub.attach_runner(source, TOKEN.to_string());
        // attach clears whatever the previous runner left behind.
        assert_eq!(next_frame(&mut rx).await["reason"], "runner_exited");

        tx.send(mutated(will_act(1, 1), |e| e["token"] = json!("bad"))).await.unwrap();
        tx.send(will_act(2, 1)).await.unwrap();
        tx.send(finished(3, 1)).await.unwrap();
        let first = next_frame(&mut rx).await;
        assert_eq!(first["phase"], "will_act");
        assert_eq!(first["seq"], 2);
        assert_eq!(next_frame(&mut rx).await["phase"], "finished");
        assert_eq!(hub.counters().dropped_invalid, 1);

        // Runner exit: the inbox closes.
        drop(tx);
        let cleared = next_frame(&mut rx).await;
        assert_eq!(cleared["phase"], "clear");
        assert_eq!(cleared["reason"], "runner_exited");
    }

    #[tokio::test]
    async fn disabling_sends_one_clear_and_then_discards_events() {
        let (hub, mut rx) = ComputerOverlayHub::new();
        hub.set_enabled(true);
        let (tx, source) = mpsc::channel(16);
        hub.attach_runner(source, TOKEN.to_string());
        let _ = next_frame(&mut rx).await;
        hub.set_enabled(false);
        let cleared = next_frame(&mut rx).await;
        assert_eq!(cleared["phase"], "clear");
        assert_eq!(cleared["reason"], "disabled");
        // Switching off twice stays quiet; events keep being drained but dropped.
        hub.set_enabled(false);
        tx.send(will_act(1, 1)).await.unwrap();
        drop(tx);
        hub.wait_for_forwarder().await;
        assert!(rx.try_recv().is_err());
        assert_eq!(hub.counters().dropped_disabled, 1);
    }

    #[tokio::test]
    async fn attaching_a_new_runner_cancels_the_old_forwarder() {
        let (hub, mut rx) = ComputerOverlayHub::new();
        hub.set_enabled(true);
        let (old_tx, old_source) = mpsc::channel(16);
        hub.attach_runner(old_source, TOKEN.to_string());
        let _ = next_frame(&mut rx).await;

        let new_token = "ffffffffffffffffffffffffffffffff".to_string();
        let (new_tx, new_source) = mpsc::channel(16);
        hub.attach_runner(new_source, new_token.clone());
        assert_eq!(next_frame(&mut rx).await["reason"], "runner_exited");

        // The old runner's late events go nowhere: its forwarder is gone.
        let _ = old_tx.send(will_act(1, 1)).await;
        // The new runner's events (new token) are forwarded.
        new_tx
            .send(mutated(will_act(1, 1), |e| e["token"] = json!(new_token)))
            .await
            .unwrap();
        let frame = next_frame(&mut rx).await;
        assert_eq!(frame["phase"], "will_act");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn machine_event_overflow_becomes_clear_overflow() {
        let (hub, mut rx) = ComputerOverlayHub::new();
        hub.set_enabled(true);
        let (tx, source) = mpsc::channel(16);
        hub.attach_runner(source, TOKEN.to_string());
        let _ = next_frame(&mut rx).await;
        tx.send(will_act(1, 1)).await.unwrap();
        assert_eq!(next_frame(&mut rx).await["phase"], "will_act");
        tx.send(json!({"event": "machine_event_overflow", "dropped_critical": 3}))
            .await
            .unwrap();
        let cleared = next_frame(&mut rx).await;
        assert_eq!(cleared["reason"], "overflow");
        assert_eq!(hub.counters().overflows, 1);
        // The lost will_act can no longer be paired.
        tx.send(finished(2, 1)).await.unwrap();
        drop(tx);
        assert_eq!(next_frame(&mut rx).await["reason"], "runner_exited");
        assert_eq!(hub.counters().dropped_invalid, 1);
    }

    #[tokio::test]
    async fn kill_switch_clear_reaches_the_app() {
        let (hub, mut rx) = ComputerOverlayHub::new();
        hub.set_enabled(true);
        hub.clear(ClearReason::Stopped);
        let frame = next_frame(&mut rx).await;
        assert_eq!(frame, json!({"v": 1, "phase": "clear", "reason": "stopped"}));
        hub.clear(ClearReason::SessionEnded);
        assert_eq!(next_frame(&mut rx).await["reason"], "session_ended");
    }

    #[tokio::test]
    async fn a_reader_that_never_reads_cannot_stall_the_forwarder() {
        let (hub, rx) = ComputerOverlayHub::with_capacity(4);
        hub.set_enabled(true);
        let (tx, source) = mpsc::channel(16);
        hub.attach_runner(source, TOKEN.to_string());
        // `rx` is held but never read, like an App that stopped consuming stdout.
        let started = std::time::Instant::now();
        for index in 0..500_u64 {
            tx.send(will_act(index * 2 + 1, index)).await.unwrap();
            tx.send(finished(index * 2 + 2, index)).await.unwrap();
        }
        drop(tx);
        tokio::time::timeout(Duration::from_secs(5), hub.wait_for_forwarder())
            .await
            .expect("forwarder must finish without a reader");
        assert!(started.elapsed() < Duration::from_secs(5));
        let counters = hub.counters();
        assert!(counters.dropped_backpressure >= 990, "{counters:?}");
        assert!(counters.forwarded <= 5);
        drop(rx);
    }

    #[test]
    fn runner_command_gets_a_fresh_private_token_on_macos_only() {
        let (hub, _rx) = ComputerOverlayHub::new();
        let mut first = Command::new("true");
        let mut second = Command::new("true");
        let token_a = hub.prepare_runner_command(&mut first);
        let token_b = hub.prepare_runner_command(&mut second);
        if cfg!(target_os = "macos") {
            let token_a = token_a.unwrap();
            let token_b = token_b.unwrap();
            assert_eq!(token_a.len(), 32);
            assert!(token_a.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
            assert_ne!(token_a, token_b, "every spawn needs its own token");
            let envs: std::collections::HashMap<_, _> = first
                .get_envs()
                .map(|(key, value)| {
                    (
                        key.to_string_lossy().into_owned(),
                        value.map(|v| v.to_string_lossy().into_owned()),
                    )
                })
                .collect();
            assert_eq!(envs[ENV_CHANNEL].as_deref(), Some("stdout-v1"));
            assert_eq!(envs[ENV_TOKEN].as_deref(), Some(token_a.as_str()));
        } else {
            assert!(token_a.is_none());
            assert_eq!(first.get_envs().count(), 0);
        }
    }

    /// Real process path: supervisor drain -> machine inbox -> hub forwarder.
    #[cfg(unix)]
    #[tokio::test]
    async fn supervised_runner_stdout_lines_reach_the_app_channel() {
        use crate::chadex_core::runtime_compat::activity::ActivityLog;
        use crate::chadex_core::runtime_compat::process::{ProcessKind, ProcessSupervisor};

        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("events.ndjson");
        let lines = [
            will_act(1, 1).to_string(),
            "not json at all".to_string(),
            mutated(will_act(2, 2), |e| e["token"] = json!("bad")).to_string(),
            finished(3, 1).to_string(),
        ];
        std::fs::write(&script, lines.join("\n") + "\n").unwrap();

        let (hub, mut rx) = ComputerOverlayHub::new();
        hub.set_enabled(true);
        let mut command = Command::new("/bin/sh");
        command.arg("-c").arg("cat \"$1\"").arg("chadex-fake-runner").arg(&script);
        let mut supervisor = ProcessSupervisor::new(ActivityLog::default());
        let inbox = supervisor
            .spawn_owned(ProcessKind::LocalRunner, command, true)
            .await
            .unwrap()
            .expect("machine stdout inbox");
        hub.attach_runner(inbox, TOKEN.to_string());

        let mut phases = Vec::new();
        while phases.last().map(String::as_str) != Some("runner_exited") || phases.len() < 3 {
            let frame = next_frame(&mut rx).await;
            let phase = frame["phase"].as_str().unwrap().to_string();
            phases.push(if phase == "clear" {
                frame["reason"].as_str().unwrap().to_string()
            } else {
                phase
            });
            if phases.len() > 8 {
                break;
            }
        }
        // attach clear, then the two valid events, then the pipe closes with the runner.
        assert_eq!(
            phases,
            ["runner_exited", "will_act", "finished", "runner_exited"]
        );
        assert_eq!(hub.counters().dropped_invalid, 1);
        supervisor.stop(ProcessKind::LocalRunner).await;
    }

    #[test]
    fn runner_channel_reports_detached_without_a_runner() {
        let (hub, _rx) = ComputerOverlayHub::new();
        let expected = if cfg!(target_os = "macos") {
            "detached"
        } else {
            "unsupported"
        };
        assert_eq!(hub.runner_channel(), expected);
    }
}
