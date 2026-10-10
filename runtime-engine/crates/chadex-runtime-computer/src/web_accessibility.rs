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
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

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

/// How long an app keeps the web accessibility Chadex switched on after the last
/// observation that needed it. A full web accessibility tree makes Chromium noticeably
/// slower (every open tab, including a long ChatGPT conversation, keeps it up to date),
/// so the switch is a lease rather than a permanent change.
pub(crate) const RELEASE_IDLE: Duration = Duration::from_secs(120);
/// How often the background sweeper looks for idle leases.
const SWEEP_INTERVAL: Duration = Duration::from_secs(20);

/// Writes `AXManualAccessibility = false` for a process (the native half of a release).
/// Implementations must re-check the launch time so a recycled pid is never touched.
pub(crate) type WebAxReleaser = fn(ProcessKey);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Lease {
    last_used: Instant,
    /// False when the attribute was already on before Chadex first wrote it (another
    /// assistive tool turned it on); such apps are never switched off by Chadex.
    restore: bool,
}

#[derive(Default)]
struct Leases {
    entries: HashMap<ProcessKey, Lease>,
    sweeper_running: bool,
}

#[derive(Default)]
struct RegistryState {
    memos: Mutex<VecDeque<(ProcessKey, WebMemo)>>,
    engines: Mutex<HashMap<PathBuf, WebEngine>>,
    leases: Mutex<Leases>,
}

/// Process-wide bookkeeping. It decides how long to *wait*, and which apps Chadex
/// switched on so they can be switched off again once idle. The attribute itself is
/// re-applied on every observation because Chromium auto-disables accessibility after a
/// period without assistive-technology requests, so even a `Confirmed` memo is
/// re-verified with one non-sleeping probe each time.
#[derive(Default)]
pub(crate) struct WebAxRegistry {
    state: Arc<RegistryState>,
    /// `None` (tests, platforms without the switch) never spawns the sweeper.
    releaser: Option<WebAxReleaser>,
}

impl WebAxRegistry {
    pub(crate) fn with_releaser(releaser: WebAxReleaser) -> Self {
        Self {
            state: Arc::default(),
            releaser: Some(releaser),
        }
    }

    pub(crate) fn memo(&self, key: ProcessKey) -> Option<WebMemo> {
        self.state.memos.lock().ok().and_then(|memos| {
            memos
                .iter()
                .find(|(candidate, _)| *candidate == key)
                .map(|(_, memo)| *memo)
        })
    }

    pub(crate) fn set_memo(&self, key: ProcessKey, memo: WebMemo) {
        if let Ok(mut memos) = self.state.memos.lock() {
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
        self.state
            .engines
            .lock()
            .ok()
            .and_then(|engines| engines.get(bundle_path).copied())
    }

    pub(crate) fn cache_engine(&self, bundle_path: &Path, engine: WebEngine) {
        if let Ok(mut engines) = self.state.engines.lock() {
            if engines.len() >= ENGINE_CACHE_CAPACITY {
                engines.clear();
            }
            engines.insert(bundle_path.to_path_buf(), engine);
        }
    }

    pub(crate) fn has_lease(&self, key: ProcessKey) -> bool {
        self.state
            .leases
            .lock()
            .is_ok_and(|leases| leases.entries.contains_key(&key))
    }

    /// Records that Chadex wrote the attribute for `key` at `now`. The first write
    /// decides whether a release may switch it off again (`restore`); later writes only
    /// extend the lease.
    pub(crate) fn touch_lease(&self, key: ProcessKey, now: Instant, restore: bool) {
        let Ok(mut leases) = self.state.leases.lock() else {
            return;
        };
        leases
            .entries
            .entry(key)
            .and_modify(|lease| lease.last_used = now)
            .or_insert(Lease {
                last_used: now,
                restore,
            });
        if let Some(releaser) = self.releaser {
            if !leases.sweeper_running {
                leases.sweeper_running = true;
                spawn_sweeper(Arc::downgrade(&self.state), releaser);
            }
        }
    }

    /// Ends every lease idle for at least `idle` and returns the processes whose
    /// attribute must be switched off. Their memos are dropped too, so the next
    /// observation waits for the tree again instead of trusting a stale `Confirmed`.
    /// The sweeper calls the same logic on the shared state.
    #[cfg(test)]
    pub(crate) fn release_idle(&self, now: Instant, idle: Duration) -> Vec<ProcessKey> {
        self.state.release_idle(now, idle)
    }

    /// Ends every lease (runtime shutdown).
    pub(crate) fn release_all(&self) -> Vec<ProcessKey> {
        self.state.release_matching(|_| true)
    }
}

impl RegistryState {
    fn release_idle(&self, now: Instant, idle: Duration) -> Vec<ProcessKey> {
        self.release_matching(|lease| now.saturating_duration_since(lease.last_used) >= idle)
    }

    fn release_matching(&self, expired: impl Fn(&Lease) -> bool) -> Vec<ProcessKey> {
        let released: Vec<(ProcessKey, bool)> = match self.leases.lock() {
            Ok(mut leases) => {
                let keys: Vec<ProcessKey> = leases
                    .entries
                    .iter()
                    .filter(|(_, lease)| expired(lease))
                    .map(|(key, _)| *key)
                    .collect();
                keys.into_iter()
                    .filter_map(|key| leases.entries.remove(&key).map(|lease| (key, lease.restore)))
                    .collect()
            }
            Err(_) => return Vec::new(),
        };
        if let Ok(mut memos) = self.memos.lock() {
            memos.retain(|(key, _)| !released.iter().any(|(released, _)| released == key));
        }
        released
            .into_iter()
            .filter_map(|(key, restore)| restore.then_some(key))
            .collect()
    }

    /// Clears the running flag when nothing is left to watch; the check and the flag
    /// share the lease lock so a concurrent `touch_lease` either sees the sweeper still
    /// running or starts a new one.
    fn sweeper_may_stop(&self) -> bool {
        self.leases.lock().map_or(true, |mut leases| {
            if leases.entries.is_empty() {
                leases.sweeper_running = false;
                true
            } else {
                false
            }
        })
    }
}

fn spawn_sweeper(state: Weak<RegistryState>, releaser: WebAxReleaser) {
    let watched = state.clone();
    let spawned = std::thread::Builder::new()
        .name("chadex-web-ax-release".to_string())
        .spawn(move || loop {
            std::thread::sleep(SWEEP_INTERVAL);
            let Some(state) = watched.upgrade() else {
                return;
            };
            for key in state.release_idle(Instant::now(), RELEASE_IDLE) {
                releaser(key);
            }
            if state.sweeper_may_stop() {
                return;
            }
        });
    if spawned.is_err() {
        // Without a sweeper the leases still end on shutdown (`Drop`).
        if let Some(state) = state.upgrade() {
            if let Ok(mut leases) = state.leases.lock() {
                leases.sweeper_running = false;
            }
        }
    }
}

impl Drop for WebAxRegistry {
    fn drop(&mut self) {
        if let Some(releaser) = self.releaser {
            for key in self.release_all() {
                releaser(key);
            }
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
    /// Current value of `AXManualAccessibility`, if the app reports one. Read once,
    /// before Chadex first writes it, to avoid switching off what someone else enabled.
    fn manual_accessibility(&self) -> Option<bool>;
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
    let registry = context.registry;
    let already_on = !registry.has_lease(process) && env.manual_accessibility() == Some(true);
    match env.set_manual_accessibility() {
        Ok(SetOutcome::Set) => registry.touch_lease(process, Instant::now(), !already_on),
        Ok(SetOutcome::Unsupported) => return Ok(WebAxState::Unsupported),
        Ok(SetOutcome::PermissionDenied) => {
            return Err(
                "permission_denied: macOS Accessibility permission is not granted".to_string(),
            )
        }
        Err(error) if is_fatal_web_error(&error) => return Err(error),
        Err(_) => return Ok(WebAxState::Unsupported),
    }
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
