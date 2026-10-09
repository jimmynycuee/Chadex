//! Chromium / Electron web accessibility enablement.
//!
//! Chromium-based apps build the accessibility tree of web content only after an
//! assistive technology asks for it. Setting `AXManualAccessibility` on the
//! application element is the Electron/Chromium-supported switch for that and, unlike
//! `AXEnhancedUserInterface`, has no window-animation side effects. The decisions live
//! here as pure, injectable logic; the macOS adapter only supplies the native setter
//! and probe.
#![cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]

use crate::WebAccessibilityPolicy;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

const MEMO_CAPACITY: usize = 64;
const ENGINE_CACHE_CAPACITY: usize = 32;
const MAX_BUNDLE_SCAN_ENTRIES: usize = 64;
const RENDERER_HELPER_SUFFIX: &str = " Helper (Renderer).app";
const ELECTRON_FRAMEWORK: &str = "Contents/Frameworks/Electron Framework.framework";

/// Bundle identifiers of Chromium based browsers known to honor
/// `AXManualAccessibility` (to be confirmed on a real machine, design item L1).
const CHROMIUM_BUNDLE_IDS: &[&str] = &[
    "com.google.Chrome",
    "com.google.Chrome.beta",
    "com.google.Chrome.dev",
    "com.google.Chrome.canary",
    "org.chromium.Chromium",
    "com.brave.Browser",
    "com.brave.Browser.beta",
    "com.brave.Browser.nightly",
    "com.microsoft.edgemac",
    "com.microsoft.edgemac.Beta",
    "com.microsoft.edgemac.Dev",
    "com.microsoft.edgemac.Canary",
    "company.thebrowser.Browser",
    "com.vivaldi.Vivaldi",
    "com.operasoftware.Opera",
];

/// Delays before each readiness probe. Cumulative sleep is at most 1.95 s.
pub(crate) const WAIT_SCHEDULE: [Duration; 5] = [
    Duration::from_millis(0),
    Duration::from_millis(150),
    Duration::from_millis(300),
    Duration::from_millis(600),
    Duration::from_millis(900),
];
/// Time that must stay available for the actual traversal; waiting never eats it.
pub(crate) const WAIT_RESERVE: Duration = Duration::from_secs(3);
/// Hard cap for the whole wait (sleeps and probes together), so a slow browser cannot
/// consume the deep-find soft budget before the first node is read.
pub(crate) const WAIT_TOTAL_BUDGET: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebEngine {
    None,
    Chromium,
    Electron,
}

fn has_renderer_helper(directory: &Path) -> bool {
    std::fs::read_dir(directory).is_ok_and(|entries| {
        entries
            .take(MAX_BUNDLE_SCAN_ENTRIES)
            .filter_map(Result::ok)
            .any(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|name| name.ends_with(RENDERER_HELPER_SUFFIX))
            })
    })
}

/// Decides whether the app behind `bundle_path` embeds Chromium. Only directory
/// names are inspected (at most 64 entries per directory); nothing is executed.
pub(crate) fn classify_web_engine(bundle_id: &str, bundle_path: &Path) -> WebEngine {
    if bundle_path.join(ELECTRON_FRAMEWORK).is_dir() {
        return WebEngine::Electron;
    }
    if CHROMIUM_BUNDLE_IDS.contains(&bundle_id) {
        return WebEngine::Chromium;
    }
    let frameworks = bundle_path.join("Contents/Frameworks");
    if has_renderer_helper(&frameworks) {
        return WebEngine::Chromium;
    }
    let Ok(entries) = std::fs::read_dir(&frameworks) else {
        return WebEngine::None;
    };
    for entry in entries.take(MAX_BUNDLE_SCAN_ENTRIES).filter_map(Result::ok) {
        let is_framework = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.ends_with(".framework"));
        if is_framework && has_renderer_helper(&entry.path().join("Versions/Current/Helpers")) {
            return WebEngine::Chromium;
        }
    }
    WebEngine::None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebAxState {
    NotApplicable,
    Enabled,
    AlreadyEnabled,
    Pending,
    Unsupported,
    Disabled,
    SkippedSensitive,
}

impl WebAxState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NotApplicable => "not_applicable",
            Self::Enabled => "enabled",
            Self::AlreadyEnabled => "already_enabled",
            Self::Pending => "pending",
            Self::Unsupported => "unsupported",
            Self::Disabled => "disabled",
            Self::SkippedSensitive => "skipped_sensitive",
        }
    }
}

/// `(pid, launch time in whole seconds)`: the launch time keeps a recycled pid from
/// inheriting another process's "already enabled" memo.
pub(crate) type ProcessKey = (u32, Option<i64>);

/// What we know about a process we already applied the attribute to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebMemo {
    /// A readiness probe saw web content (or definitively no web area).
    Confirmed,
    /// We waited for this process before but never saw content (the schedule ran out or
    /// the time budget did). Later observations only probe once, without sleeping.
    Waited,
}

/// Process-wide bookkeeping. It only decides how long to *wait*; the attribute itself is
/// re-applied on every observation because Chromium auto-disables accessibility after a
/// period without assistive-technology requests, so even a `Confirmed` memo is
/// re-verified with one non-sleeping probe each time.
#[derive(Default)]
pub(crate) struct WebAxRegistry {
    memos: Mutex<VecDeque<(ProcessKey, WebMemo)>>,
    engines: Mutex<HashMap<PathBuf, WebEngine>>,
}

impl WebAxRegistry {
    pub(crate) fn memo(&self, key: ProcessKey) -> Option<WebMemo> {
        self.memos.lock().ok().and_then(|memos| {
            memos
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, memo)| *memo)
        })
    }

    pub(crate) fn set_memo(&self, key: ProcessKey, memo: WebMemo) {
        if let Ok(mut memos) = self.memos.lock() {
            if let Some(entry) = memos.iter_mut().find(|(candidate, _)| *candidate == key) {
                entry.1 = memo;
                return;
            }
            if memos.len() >= MEMO_CAPACITY {
                memos.pop_front();
            }
            memos.push_back((key, memo));
        }
    }

    pub(crate) fn cached_engine(&self, bundle_path: &Path) -> Option<WebEngine> {
        self.engines
            .lock()
            .ok()
            .and_then(|engines| engines.get(bundle_path).copied())
    }

    pub(crate) fn cache_engine(&self, bundle_path: &Path, engine: WebEngine) {
        if let Ok(mut engines) = self.engines.lock() {
            if engines.len() >= ENGINE_CACHE_CAPACITY {
                engines.clear();
            }
            engines.insert(bundle_path.to_path_buf(), engine);
        }
    }
}

/// Per-observation inputs the platform passes down.
#[derive(Clone, Copy)]
pub(crate) struct WebAxContext<'a> {
    pub(crate) policy: WebAccessibilityPolicy,
    /// Sensitive surfaces never get the attribute written.
    pub(crate) sensitive_surface: bool,
    pub(crate) registry: &'a WebAxRegistry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SetOutcome {
    Set,
    /// The app does not implement the attribute (or rejected the value).
    Unsupported,
    /// The Accessibility permission is missing (`kAXErrorAPIDisabled`).
    PermissionDenied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WebProbe {
    /// An `AXWebArea` with at least one child exists.
    Content,
    /// A web area exists but is still empty, or the search was inconclusive.
    Empty,
    /// The whole reachable window tree has no `AXWebArea` (for example a native
    /// Electron settings window).
    NoWebArea,
}

pub(crate) trait WebAxEnvironment {
    /// Writes `AXManualAccessibility = true` on the application element.
    fn set_manual_accessibility(&self) -> Result<SetOutcome, String>;
    /// One readiness probe that must stop on its own after roughly `budget`.
    fn probe(&self, budget: Duration) -> Result<WebProbe, String>;
    fn sleep(&self, duration: Duration);
    /// Time left on the observation deadline.
    fn remaining(&self) -> Duration;
}

/// Errors that must abort the observation: a missing Accessibility permission and the
/// observation deadline. Everything else (Chromium rebuilding its tree answers with
/// `InvalidUIElement`, `CannotComplete`, ...) only affects this best-effort feature.
fn is_fatal_web_error(error: &str) -> bool {
    error.starts_with("permission_denied:") || error.contains("deadline exceeded")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WaitOutcome {
    Ready,
    /// The whole schedule ran without seeing content.
    Exhausted,
    /// The time budget ran out before the schedule could finish.
    OutOfBudget,
}

/// Bounded wait for the renderer to publish its tree. Both sleeping and probing count
/// against `WAIT_TOTAL_BUDGET` and never reach into `WAIT_RESERVE`.
fn wait_for_web_content_outcome(env: &impl WebAxEnvironment) -> Result<WaitOutcome, String> {
    let started_remaining = env.remaining();
    let spare = || {
        let used = started_remaining.saturating_sub(env.remaining());
        env.remaining()
            .saturating_sub(WAIT_RESERVE)
            .min(WAIT_TOTAL_BUDGET.saturating_sub(used))
    };
    for delay in WAIT_SCHEDULE {
        let available = spare();
        if available.is_zero() {
            return Ok(WaitOutcome::OutOfBudget);
        }
        let delay = delay.min(available);
        if !delay.is_zero() {
            env.sleep(delay);
        }
        let probe_budget = spare();
        if probe_budget.is_zero() {
            return Ok(WaitOutcome::OutOfBudget);
        }
        match env.probe(probe_budget) {
            Ok(WebProbe::Content | WebProbe::NoWebArea) => return Ok(WaitOutcome::Ready),
            Ok(WebProbe::Empty) => {}
            Err(error) if is_fatal_web_error(&error) => return Err(error),
            // Transient AX errors while the tree is rebuilt: keep waiting.
            Err(_) => {}
        }
    }
    Ok(WaitOutcome::Exhausted)
}

/// Whether the (legacy, rootless) tree of a sensitive surface must stop at the
/// `AXWebArea` boundary. Accessibility is enabled per application but sensitivity is per
/// window, so once another window of a Chromium/Electron app has had web accessibility
/// switched on, a sensitive window of the same app would otherwise start exposing its
/// page content, which it did not before Chadex enabled the feature.
pub(crate) fn should_omit_web_content(
    policy: WebAccessibilityPolicy,
    sensitive_surface: bool,
    engine: WebEngine,
) -> bool {
    policy == WebAccessibilityPolicy::Auto && sensitive_surface && engine != WebEngine::None
}

/// Budget for the single non-sleeping probe used when a process is already memoized.
fn single_probe_budget(env: &impl WebAxEnvironment) -> Duration {
    env.remaining()
        .saturating_sub(WAIT_RESERVE)
        .min(WAIT_TOTAL_BUDGET)
}

/// Enables web accessibility for the observed application when appropriate.
/// `identify` is only invoked once policy and surface sensitivity allow a write (the
/// `Off` policy and a sensitive surface return before reading the bundle). Failures of
/// this best-effort step never fail the observation, except a missing Accessibility
/// permission and the observation deadline.
pub(crate) fn enable_web_accessibility(
    env: &impl WebAxEnvironment,
    context: &WebAxContext<'_>,
    identify: impl FnOnce() -> Option<(WebEngine, ProcessKey)>,
) -> Result<WebAxState, String> {
    if context.policy == WebAccessibilityPolicy::Off {
        return Ok(WebAxState::Disabled);
    }
    if context.sensitive_surface {
        return Ok(WebAxState::SkippedSensitive);
    }
    let Some((engine, process)) = identify() else {
        return Ok(WebAxState::NotApplicable);
    };
    if engine == WebEngine::None {
        return Ok(WebAxState::NotApplicable);
    }
    match env.set_manual_accessibility() {
        Ok(SetOutcome::Set) => {}
        Ok(SetOutcome::Unsupported) => return Ok(WebAxState::Unsupported),
        Ok(SetOutcome::PermissionDenied) => {
            return Err(
                "permission_denied: macOS Accessibility permission is not granted".to_string(),
            )
        }
        Err(error) if is_fatal_web_error(&error) => return Err(error),
        Err(_) => return Ok(WebAxState::Unsupported),
    }
    let registry = context.registry;
    if registry.memo(process).is_some() {
        // Waited (or confirmed) before: never sleep again, verify with one probe.
        let budget = single_probe_budget(env);
        if budget.is_zero() {
            // No evidence this time: never claim `already_enabled` unverified,
            // and leave the memo as it is.
            return Ok(WebAxState::Pending);
        }
        return match env.probe(budget) {
            Ok(WebProbe::Content | WebProbe::NoWebArea) => {
                registry.set_memo(process, WebMemo::Confirmed);
                Ok(WebAxState::AlreadyEnabled)
            }
            Ok(WebProbe::Empty) => {
                registry.set_memo(process, WebMemo::Waited);
                Ok(WebAxState::Pending)
            }
            Err(error) if is_fatal_web_error(&error) => Err(error),
            // Transient AX error: cannot confirm, so do not claim availability.
            Err(_) => {
                registry.set_memo(process, WebMemo::Waited);
                Ok(WebAxState::Pending)
            }
        };
    }
    match wait_for_web_content_outcome(env)? {
        WaitOutcome::Ready => {
            registry.set_memo(process, WebMemo::Confirmed);
            Ok(WebAxState::Enabled)
        }
        // Either the schedule ran out or the budget did: remember that we waited so
        // later observations only pay for one probe, not the whole wait again.
        WaitOutcome::Exhausted | WaitOutcome::OutOfBudget => {
            registry.set_memo(process, WebMemo::Waited);
            Ok(WebAxState::Pending)
        }
    }
}

#[cfg(test)]
mod tests;
