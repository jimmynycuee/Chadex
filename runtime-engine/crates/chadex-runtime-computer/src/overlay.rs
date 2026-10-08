//! Cursor overlay event model for Computer Use (see
//! `docs/computer-use/cursor-overlay-design.md`).
//!
//! The overlay is purely visual and best effort. Nothing in this module may
//! block, perform IO, or change the result of a Computer action:
//!
//! * `ComputerOverlaySink::emit` implementations must return immediately.
//! * `OverlayActionGuard` swallows panics from the sink.
//! * Events carry geometry and a closed action vocabulary only. They never
//!   carry text, application names, window titles, element names or ids, or
//!   error messages.

use serde_json::{json, Value};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

/// Wire schema identifier for runner -> helper events.
pub const OVERLAY_SCHEMA: &str = "chadex.computer_overlay.v1";
/// Wire event name used by the helper to route a line.
pub const OVERLAY_EVENT_NAME: &str = "computer_overlay";
/// Coordinate space used by the macOS implementation: Quartz global display
/// coordinates in points, origin at the top-left of the main display, y down.
pub const OVERLAY_SPACE_MACOS_CG_GLOBAL_PT: &str = "macos_cg_global_pt";
/// How long the App may keep the marker without a `finished` event.
pub const DEFAULT_OVERLAY_TTL_MS: u32 = 2000;
/// Title of the App's overlay panel. The App sets the same string; the runtime
/// uses it to keep that window out of `list_windows`.
pub const OVERLAY_WINDOW_TITLE: &str = "Chadex Agent Cursor";
/// Upper bound for one encoded overlay line, newline included.
pub const MAX_OVERLAY_LINE_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayAction {
    Move,
    Click,
    Press,
    Focus,
    Scroll,
    Input,
    Key,
    Activate,
}

impl OverlayAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Move => "move",
            Self::Click => "click",
            Self::Press => "press",
            Self::Focus => "focus",
            Self::Scroll => "scroll",
            Self::Input => "input",
            Self::Key => "key",
            Self::Activate => "activate",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum OverlayTarget {
    Point {
        x: f64,
        y: f64,
    },
    /// Top-left origin plus size.
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    },
    None,
}

impl OverlayTarget {
    /// Non-finite coordinates collapse to `None` so a bad native read can never
    /// produce an invalid event.
    pub fn point(x: f64, y: f64) -> Self {
        if x.is_finite() && y.is_finite() {
            Self::Point { x, y }
        } else {
            Self::None
        }
    }

    /// Non-finite values and empty or negative sizes collapse to `None`.
    pub fn rect(x: f64, y: f64, width: f64, height: f64) -> Self {
        if x.is_finite()
            && y.is_finite()
            && width.is_finite()
            && height.is_finite()
            && width > 0.0
            && height > 0.0
        {
            Self::Rect {
                x,
                y,
                width,
                height,
            }
        } else {
            Self::None
        }
    }

    /// Center of a point or rect; used to pick the containing display.
    pub fn center(self) -> Option<(f64, f64)> {
        match self {
            Self::Point { x, y } => Some((x, y)),
            Self::Rect {
                x,
                y,
                width,
                height,
            } => Some((x + width / 2.0, y + height / 2.0)),
            Self::None => None,
        }
    }

    fn to_value(self) -> Value {
        match self {
            Self::Point { x, y } => json!({"kind": "point", "x": x, "y": y}),
            Self::Rect {
                x,
                y,
                width,
                height,
            } => json!({"kind": "rect", "x": x, "y": y, "width": width, "height": height}),
            Self::None => json!({"kind": "none"}),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayOutcome {
    Succeeded,
    Failed,
    NotStarted,
    Unknown,
}

impl OverlayOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::NotStarted => "not_started",
            Self::Unknown => "unknown",
        }
    }
}

/// Native display the target lives on: `CGDirectDisplayID` plus the
/// `CGDisplayBounds` (x, y, width, height) the runtime observed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayDisplay {
    pub id: u32,
    pub bounds: (f64, f64, f64, f64),
}

impl OverlayDisplay {
    fn to_value(self) -> Value {
        json!({
            "id": self.id,
            "bounds": {
                "x": self.bounds.0,
                "y": self.bounds.1,
                "width": self.bounds.2,
                "height": self.bounds.3,
            },
        })
    }
}

/// Key description for `key` actions. Both fields come from the closed
/// vocabulary already enforced by `validate_key_input`; no user text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverlayKey {
    pub name: String,
    pub modifiers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ComputerOverlayEvent {
    WillAct {
        action_id: u64,
        action: OverlayAction,
        target: OverlayTarget,
        display: Option<OverlayDisplay>,
        key: Option<OverlayKey>,
        ttl_ms: u32,
    },
    Finished {
        action_id: u64,
        outcome: OverlayOutcome,
    },
}

impl ComputerOverlayEvent {
    /// Encode as the runner -> helper wire object (schema v1).
    pub fn to_wire_value(&self, token: &str, seq: u64, emitted_at_ms: u64) -> Value {
        match self {
            Self::WillAct {
                action_id,
                action,
                target,
                display,
                key,
                ttl_ms,
            } => {
                let mut value = json!({
                    "event": OVERLAY_EVENT_NAME,
                    "schema": OVERLAY_SCHEMA,
                    "token": token,
                    "seq": seq,
                    "phase": "will_act",
                    "action_id": action_id,
                    "action": action.as_str(),
                    "target": target.to_value(),
                    "space": OVERLAY_SPACE_MACOS_CG_GLOBAL_PT,
                    "display": display.map(OverlayDisplay::to_value),
                    "ttl_ms": ttl_ms,
                    "emitted_at_ms": emitted_at_ms,
                });
                if let Some(key) = key {
                    value["key"] = json!({"name": key.name, "modifiers": key.modifiers});
                }
                value
            }
            Self::Finished { action_id, outcome } => json!({
                "event": OVERLAY_EVENT_NAME,
                "schema": OVERLAY_SCHEMA,
                "token": token,
                "seq": seq,
                "phase": "finished",
                "action_id": action_id,
                "outcome": outcome.as_str(),
                "emitted_at_ms": emitted_at_ms,
            }),
        }
    }

    /// Encode as one NDJSON line (newline included). Returns `None` when the
    /// line would exceed [`MAX_OVERLAY_LINE_BYTES`]; callers drop the event.
    pub fn encode_line(&self, token: &str, seq: u64, emitted_at_ms: u64) -> Option<Vec<u8>> {
        let mut line = serde_json::to_vec(&self.to_wire_value(token, seq, emitted_at_ms)).ok()?;
        line.push(b'\n');
        (line.len() <= MAX_OVERLAY_LINE_BYTES).then_some(line)
    }
}

/// Receiver of overlay events.
pub trait ComputerOverlaySink: Send + Sync {
    /// Must return immediately: no blocking, no IO that can wait on a peer.
    fn emit(&self, event: ComputerOverlayEvent);
}

/// Map an operation result to the wire outcome (design section 3.2).
pub fn overlay_outcome(result: &Result<Value, String>) -> OverlayOutcome {
    match result {
        Ok(value) => {
            if value.get("success").and_then(Value::as_bool) == Some(true) {
                OverlayOutcome::Succeeded
            } else {
                OverlayOutcome::Failed
            }
        }
        Err(message) if message.starts_with("not_started:") => OverlayOutcome::NotStarted,
        Err(message) if message.contains("outcome_unknown") => OverlayOutcome::Unknown,
        Err(_) => OverlayOutcome::Failed,
    }
}

/// True for the App's overlay panel: exact sentinel title and a `Chadex...` owner.
pub fn is_chadex_overlay_window(application: &str, title: &str) -> bool {
    title == OVERLAY_WINDOW_TITLE && application.starts_with("Chadex")
}

/// Pick the display whose bounds contain the point. Bounds are half-open
/// (`x <= px < x + width`) like Quartz display bounds.
pub fn display_containing_point(
    displays: &[OverlayDisplay],
    x: f64,
    y: f64,
) -> Option<OverlayDisplay> {
    displays.iter().copied().find(|display| {
        let (bx, by, bw, bh) = display.bounds;
        x >= bx && x < bx + bw && y >= by && y < by + bh
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuardState {
    Idle,
    Begun,
    Finished,
}

/// Brackets one effectful action with `WillAct` / `Finished`.
///
/// * With no sink the guard is inert: it never reads a frame and never emits.
/// * `begin` is called by platform code after the last validation and right
///   before the first native effect; it runs the frame reader lazily.
/// * `finish` reports the real result. If the guard is dropped after `begin`
///   without `finish` (early `?` return or panic) it reports `Unknown`.
/// * A guard that never began (validation failed, sensitive surface refused)
///   emits nothing at all.
pub(crate) struct OverlayActionGuard {
    sink: Option<Arc<dyn ComputerOverlaySink>>,
    action_id: u64,
    action: OverlayAction,
    key: Option<OverlayKey>,
    state: GuardState,
}

impl OverlayActionGuard {
    pub(crate) fn new(
        sink: Option<Arc<dyn ComputerOverlaySink>>,
        action_id: u64,
        action: OverlayAction,
    ) -> Self {
        Self {
            sink,
            action_id,
            action,
            key: None,
            state: GuardState::Idle,
        }
    }

    pub(crate) fn with_key(mut self, key: OverlayKey) -> Self {
        self.key = Some(key);
        self
    }

    /// Whether a sink is installed. Platform code may use this to skip work
    /// that only the overlay needs.
    #[allow(dead_code)]
    pub(crate) fn is_enabled(&self) -> bool {
        self.sink.is_some()
    }

    /// Emit `WillAct`. `read_frame` runs only when a sink exists and only on
    /// the first call. It must be infallible: failures become
    /// `(OverlayTarget::None, None)`.
    pub(crate) fn begin(
        &mut self,
        read_frame: impl FnOnce() -> (OverlayTarget, Option<OverlayDisplay>),
    ) {
        if self.state != GuardState::Idle {
            return;
        }
        let Some(sink) = self.sink.clone() else {
            return;
        };
        self.state = GuardState::Begun;
        let (target, display) =
            catch_unwind(AssertUnwindSafe(read_frame)).unwrap_or((OverlayTarget::None, None));
        safe_emit(
            &sink,
            ComputerOverlayEvent::WillAct {
                action_id: self.action_id,
                action: self.action,
                target,
                display,
                key: self.key.clone(),
                ttl_ms: DEFAULT_OVERLAY_TTL_MS,
            },
        );
    }

    pub(crate) fn finish(&mut self, result: &Result<Value, String>) {
        self.finish_with(overlay_outcome(result));
    }

    fn finish_with(&mut self, outcome: OverlayOutcome) {
        if self.state != GuardState::Begun {
            return;
        }
        self.state = GuardState::Finished;
        if let Some(sink) = self.sink.clone() {
            safe_emit(
                &sink,
                ComputerOverlayEvent::Finished {
                    action_id: self.action_id,
                    outcome,
                },
            );
        }
    }
}

impl Drop for OverlayActionGuard {
    fn drop(&mut self) {
        self.finish_with(OverlayOutcome::Unknown);
    }
}

fn safe_emit(sink: &Arc<dyn ComputerOverlaySink>, event: ComputerOverlayEvent) {
    // A misbehaving sink must never affect the action or abort on unwind in Drop.
    let _ = catch_unwind(AssertUnwindSafe(|| sink.emit(event)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<ComputerOverlayEvent>>,
    }

    impl ComputerOverlaySink for RecordingSink {
        fn emit(&self, event: ComputerOverlayEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    impl RecordingSink {
        fn take(&self) -> Vec<ComputerOverlayEvent> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }
    }

    fn guard_with(sink: &Arc<RecordingSink>, action: OverlayAction) -> OverlayActionGuard {
        let sink: Arc<dyn ComputerOverlaySink> = sink.clone();
        OverlayActionGuard::new(Some(sink), 17, action)
    }

    fn ok_success(success: bool) -> Result<Value, String> {
        Ok(json!({"success": success}))
    }

    #[test]
    fn will_act_point_encodes_the_documented_wire_shape() {
        let event = ComputerOverlayEvent::WillAct {
            action_id: 17,
            action: OverlayAction::Click,
            target: OverlayTarget::point(812.5, 433.0),
            display: Some(OverlayDisplay {
                id: 69733378,
                bounds: (0.0, 0.0, 1512.0, 982.0),
            }),
            key: None,
            ttl_ms: 2000,
        };
        let value = event.to_wire_value("tok", 42, 1791500000123);
        assert_eq!(
            value,
            json!({
                "event": "computer_overlay",
                "schema": "chadex.computer_overlay.v1",
                "token": "tok",
                "seq": 42,
                "phase": "will_act",
                "action_id": 17,
                "action": "click",
                "target": {"kind": "point", "x": 812.5, "y": 433.0},
                "space": "macos_cg_global_pt",
                "display": {"id": 69733378, "bounds": {"x": 0.0, "y": 0.0, "width": 1512.0, "height": 982.0}},
                "ttl_ms": 2000,
                "emitted_at_ms": 1791500000123u64,
            })
        );
        assert!(value.get("key").is_none());
        let line = event.encode_line("tok", 42, 1791500000123).unwrap();
        assert!(line.ends_with(b"\n"));
        assert!(line.len() <= MAX_OVERLAY_LINE_BYTES);
        assert_eq!(line.iter().filter(|b| **b == b'\n').count(), 1);
    }

    #[test]
    fn will_act_rect_none_display_and_key_encode() {
        let event = ComputerOverlayEvent::WillAct {
            action_id: 3,
            action: OverlayAction::Key,
            target: OverlayTarget::rect(10.0, 20.0, 300.0, 40.0),
            display: None,
            key: Some(OverlayKey {
                name: "enter".to_string(),
                modifiers: vec!["command".to_string()],
            }),
            ttl_ms: 2000,
        };
        let value = event.to_wire_value("t", 1, 2);
        assert_eq!(
            value["target"],
            json!({"kind": "rect", "x": 10.0, "y": 20.0, "width": 300.0, "height": 40.0})
        );
        assert_eq!(value["display"], Value::Null);
        assert_eq!(
            value["key"],
            json!({"name": "enter", "modifiers": ["command"]})
        );
        assert_eq!(value["action"], "key");

        let none = ComputerOverlayEvent::WillAct {
            action_id: 4,
            action: OverlayAction::Press,
            target: OverlayTarget::None,
            display: None,
            key: None,
            ttl_ms: 2000,
        };
        assert_eq!(none.to_wire_value("t", 1, 2)["target"], json!({"kind": "none"}));
    }

    #[test]
    fn finished_encodes_the_documented_wire_shape_for_every_outcome() {
        for (outcome, text) in [
            (OverlayOutcome::Succeeded, "succeeded"),
            (OverlayOutcome::Failed, "failed"),
            (OverlayOutcome::NotStarted, "not_started"),
            (OverlayOutcome::Unknown, "unknown"),
        ] {
            let value = ComputerOverlayEvent::Finished {
                action_id: 17,
                outcome,
            }
            .to_wire_value("tok", 43, 1791500000171);
            assert_eq!(
                value,
                json!({
                    "event": "computer_overlay",
                    "schema": "chadex.computer_overlay.v1",
                    "token": "tok",
                    "seq": 43,
                    "phase": "finished",
                    "action_id": 17,
                    "outcome": text,
                    "emitted_at_ms": 1791500000171u64,
                })
            );
        }
    }

    #[test]
    fn every_action_has_a_closed_wire_name() {
        let names: Vec<_> = [
            OverlayAction::Move,
            OverlayAction::Click,
            OverlayAction::Press,
            OverlayAction::Focus,
            OverlayAction::Scroll,
            OverlayAction::Input,
            OverlayAction::Key,
            OverlayAction::Activate,
        ]
        .iter()
        .map(|action| action.as_str())
        .collect();
        assert_eq!(
            names,
            ["move", "click", "press", "focus", "scroll", "input", "key", "activate"]
        );
    }

    #[test]
    fn targets_reject_non_finite_and_empty_geometry() {
        assert_eq!(OverlayTarget::point(f64::NAN, 1.0), OverlayTarget::None);
        assert_eq!(OverlayTarget::point(1.0, f64::INFINITY), OverlayTarget::None);
        assert_eq!(OverlayTarget::rect(0.0, 0.0, 0.0, 10.0), OverlayTarget::None);
        assert_eq!(OverlayTarget::rect(0.0, 0.0, 10.0, -1.0), OverlayTarget::None);
        assert_eq!(
            OverlayTarget::rect(f64::NAN, 0.0, 10.0, 10.0),
            OverlayTarget::None
        );
        assert_eq!(
            OverlayTarget::rect(-5.0, -5.0, 10.0, 10.0).center(),
            Some((0.0, 0.0))
        );
        assert_eq!(OverlayTarget::None.center(), None);
    }

    #[test]
    fn events_never_carry_free_text() {
        // Serialize the widest event and assert the exact key set; anything new
        // must be added deliberately here and in the design document.
        let value = ComputerOverlayEvent::WillAct {
            action_id: 1,
            action: OverlayAction::Input,
            target: OverlayTarget::rect(1.0, 2.0, 3.0, 4.0),
            display: Some(OverlayDisplay {
                id: 1,
                bounds: (0.0, 0.0, 1.0, 1.0),
            }),
            key: Some(OverlayKey {
                name: "tab".to_string(),
                modifiers: vec![],
            }),
            ttl_ms: 2000,
        }
        .to_wire_value("t", 1, 1);
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "action",
                "action_id",
                "display",
                "emitted_at_ms",
                "event",
                "key",
                "phase",
                "schema",
                "seq",
                "space",
                "target",
                "token",
                "ttl_ms"
            ]
        );
    }

    #[test]
    fn outcome_mapping_follows_the_design_table() {
        assert_eq!(overlay_outcome(&ok_success(true)), OverlayOutcome::Succeeded);
        assert_eq!(overlay_outcome(&ok_success(false)), OverlayOutcome::Failed);
        assert_eq!(overlay_outcome(&Ok(json!({}))), OverlayOutcome::Failed);
        assert_eq!(
            overlay_outcome(&Err("not_started: click plan incomplete".to_string())),
            OverlayOutcome::NotStarted
        );
        assert_eq!(
            overlay_outcome(&Err("outcome_unknown: AX returned after write".to_string())),
            OverlayOutcome::Unknown
        );
        assert_eq!(
            overlay_outcome(&Err("control_failed: unsupported".to_string())),
            OverlayOutcome::Failed
        );
        assert_eq!(
            overlay_outcome(&Err("permission_denied: nope".to_string())),
            OverlayOutcome::Failed
        );
    }

    #[test]
    fn guard_emits_will_act_then_finished_once() {
        let sink = Arc::new(RecordingSink::default());
        {
            let mut guard = guard_with(&sink, OverlayAction::Click);
            guard.begin(|| (OverlayTarget::point(1.0, 2.0), None));
            guard.finish(&ok_success(true));
        }
        let events = sink.take();
        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0],
            ComputerOverlayEvent::WillAct {
                action_id: 17,
                action: OverlayAction::Click,
                target: OverlayTarget::Point { .. },
                ..
            }
        ));
        assert_eq!(
            events[1],
            ComputerOverlayEvent::Finished {
                action_id: 17,
                outcome: OverlayOutcome::Succeeded
            }
        );
    }

    #[test]
    fn guard_that_never_began_emits_nothing() {
        let sink = Arc::new(RecordingSink::default());
        {
            let mut guard = guard_with(&sink, OverlayAction::Press);
            // Validation failed before the effect boundary.
            guard.finish(&Err("stale_element: gone".to_string()));
        }
        {
            let _guard = guard_with(&sink, OverlayAction::Press);
        }
        assert!(sink.take().is_empty());
    }

    #[test]
    fn guard_dropped_after_begin_reports_unknown_on_early_return() {
        let sink = Arc::new(RecordingSink::default());
        fn early_return(sink: &Arc<RecordingSink>) -> Result<(), String> {
            let mut guard = guard_with(sink, OverlayAction::Focus);
            guard.begin(|| (OverlayTarget::None, None));
            Err("returned with ? before finish".to_string())
        }
        assert!(early_return(&sink).is_err());
        let events = sink.take();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1],
            ComputerOverlayEvent::Finished {
                action_id: 17,
                outcome: OverlayOutcome::Unknown
            }
        );
    }

    #[test]
    fn guard_dropped_during_panic_reports_unknown() {
        let sink = Arc::new(RecordingSink::default());
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut guard = guard_with(&sink, OverlayAction::Input);
            guard.begin(|| (OverlayTarget::None, None));
            panic!("action panicked");
        }));
        assert!(result.is_err());
        let events = sink.take();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1],
            ComputerOverlayEvent::Finished {
                action_id: 17,
                outcome: OverlayOutcome::Unknown
            }
        );
    }

    #[test]
    fn guard_does_not_resend_after_finish_or_repeat_begin() {
        let sink = Arc::new(RecordingSink::default());
        {
            let mut guard = guard_with(&sink, OverlayAction::Scroll);
            guard.begin(|| (OverlayTarget::None, None));
            guard.begin(|| panic!("second begin must not read a frame"));
            guard.finish(&ok_success(false));
            guard.finish(&ok_success(true));
        }
        let events = sink.take();
        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1],
            ComputerOverlayEvent::Finished {
                action_id: 17,
                outcome: OverlayOutcome::Failed
            }
        );
    }

    #[test]
    fn guard_without_sink_never_reads_a_frame_or_emits() {
        let reads = AtomicUsize::new(0);
        let mut guard = OverlayActionGuard::new(None, 1, OverlayAction::Press);
        assert!(!guard.is_enabled());
        guard.begin(|| {
            reads.fetch_add(1, Ordering::SeqCst);
            (OverlayTarget::None, None)
        });
        guard.finish(&ok_success(true));
        drop(guard);
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn guard_survives_a_panicking_sink_and_frame_reader() {
        struct PanicSink;
        impl ComputerOverlaySink for PanicSink {
            fn emit(&self, _event: ComputerOverlayEvent) {
                panic!("sink bug");
            }
        }
        let mut guard =
            OverlayActionGuard::new(Some(Arc::new(PanicSink)), 1, OverlayAction::Press);
        guard.begin(|| (OverlayTarget::None, None));
        guard.finish(&ok_success(true));
        drop(guard);

        let sink = Arc::new(RecordingSink::default());
        let mut guard = guard_with(&sink, OverlayAction::Press);
        guard.begin(|| panic!("frame reader bug"));
        guard.finish(&ok_success(true));
        let events = sink.take();
        assert!(matches!(
            events[0],
            ComputerOverlayEvent::WillAct {
                target: OverlayTarget::None,
                display: None,
                ..
            }
        ));
    }

    #[test]
    fn key_guard_carries_the_closed_vocabulary_key() {
        let sink = Arc::new(RecordingSink::default());
        let dyn_sink: Arc<dyn ComputerOverlaySink> = sink.clone();
        let mut guard = OverlayActionGuard::new(Some(dyn_sink), 9, OverlayAction::Key)
            .with_key(OverlayKey {
                name: "enter".to_string(),
                modifiers: vec!["command".to_string()],
            });
        guard.begin(|| (OverlayTarget::None, None));
        let events = sink.take();
        assert!(matches!(
            &events[0],
            ComputerOverlayEvent::WillAct { key: Some(key), .. } if key.name == "enter"
        ));
    }

    #[test]
    fn overlay_window_filter_needs_exact_title_and_chadex_owner() {
        for (application, title, expected) in [
            ("Chadex", "Chadex Agent Cursor", true),
            ("Chadex Dev", "Chadex Agent Cursor", true),
            ("Chadex", "Chadex Agent Cursor ", false),
            ("Chadex", "chadex agent cursor", false),
            ("Chadex", "Chadex", false),
            ("Safari", "Chadex Agent Cursor", false),
            ("", "Chadex Agent Cursor", false),
            ("MyChadex", "Chadex Agent Cursor", false),
        ] {
            assert_eq!(
                is_chadex_overlay_window(application, title),
                expected,
                "{application:?} / {title:?}"
            );
        }
    }

    #[test]
    fn display_lookup_uses_half_open_bounds_and_negative_origins() {
        let main = OverlayDisplay {
            id: 1,
            bounds: (0.0, 0.0, 1512.0, 982.0),
        };
        let left = OverlayDisplay {
            id: 2,
            bounds: (-1920.0, -100.0, 1920.0, 1080.0),
        };
        let displays = [main, left];
        assert_eq!(display_containing_point(&displays, 10.0, 10.0), Some(main));
        assert_eq!(
            display_containing_point(&displays, -1.0, 10.0),
            Some(left)
        );
        assert_eq!(display_containing_point(&displays, 1512.0, 10.0), None);
        assert_eq!(display_containing_point(&displays, 10.0, 982.0), None);
        assert_eq!(display_containing_point(&[], 0.0, 0.0), None);
    }
}
