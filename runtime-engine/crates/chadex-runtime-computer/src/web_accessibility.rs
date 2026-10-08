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

/// Process-wide bookkeeping. It only decides whether to *wait*; the attribute itself is
/// re-applied on every observation because Chromium auto-disables accessibility after a
/// period without assistive-technology requests.
#[derive(Default)]
pub(crate) struct WebAxRegistry {
    enabled: Mutex<VecDeque<ProcessKey>>,
    engines: Mutex<HashMap<PathBuf, WebEngine>>,
}

impl WebAxRegistry {
    pub(crate) fn is_enabled(&self, key: ProcessKey) -> bool {
        self.enabled
            .lock()
            .is_ok_and(|enabled| enabled.contains(&key))
    }

    pub(crate) fn mark_enabled(&self, key: ProcessKey) {
        if let Ok(mut enabled) = self.enabled.lock() {
            if enabled.contains(&key) {
                return;
            }
            if enabled.len() >= MEMO_CAPACITY {
                enabled.pop_front();
            }
            enabled.push_back(key);
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
    fn probe(&self) -> Result<WebProbe, String>;
    fn sleep(&self, duration: Duration);
    /// Time left on the observation deadline.
    fn remaining(&self) -> Duration;
}

/// Bounded wait for the renderer to publish its tree. Never sleeps into the time
/// reserved for the real traversal.
pub(crate) fn wait_for_web_content(env: &impl WebAxEnvironment) -> Result<WebAxState, String> {
    for delay in WAIT_SCHEDULE {
        let spare = env.remaining().saturating_sub(WAIT_RESERVE);
        let delay = delay.min(spare);
        if !delay.is_zero() {
            env.sleep(delay);
        }
        match env.probe()? {
            WebProbe::Content | WebProbe::NoWebArea => return Ok(WebAxState::Enabled),
            WebProbe::Empty => {}
        }
        if spare.is_zero() {
            break;
        }
    }
    Ok(WebAxState::Pending)
}

/// Enables web accessibility for the observed application when appropriate.
/// `identify` is only invoked once policy and surface sensitivity allow a write, so
/// disabled/sensitive observations never even read the bundle.
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
    match env.set_manual_accessibility()? {
        SetOutcome::Set => {}
        SetOutcome::Unsupported => return Ok(WebAxState::Unsupported),
        SetOutcome::PermissionDenied => {
            return Err(
                "permission_denied: macOS Accessibility permission is not granted".to_string(),
            )
        }
    }
    if context.registry.is_enabled(process) {
        return Ok(WebAxState::AlreadyEnabled);
    }
    let state = wait_for_web_content(env)?;
    if state == WebAxState::Enabled {
        context.registry.mark_enabled(process);
    }
    Ok(state)
}

#[cfg(test)]
mod tests;
