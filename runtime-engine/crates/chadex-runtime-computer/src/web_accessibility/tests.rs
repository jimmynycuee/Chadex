use super::*;
use std::cell::{Cell, RefCell};
use std::fs;

fn make_dirs(root: &Path, relative: &[&str]) {
    for path in relative {
        fs::create_dir_all(root.join(path)).unwrap();
    }
}

#[test]
fn classify_detects_electron_by_framework_regardless_of_bundle_id() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("Notion.app");
    make_dirs(
        &app,
        &["Contents/Frameworks/Electron Framework.framework/Versions/A"],
    );
    assert_eq!(
        classify_web_engine("notion.id", &app),
        WebEngine::Electron
    );
    // Electron wins over the Chromium allowlist.
    assert_eq!(
        classify_web_engine("com.google.Chrome", &app),
        WebEngine::Electron
    );
}

#[test]
fn classify_allowlisted_chromium_browsers_without_touching_the_bundle() {
    let missing = Path::new("/definitely/not/a/real/bundle.app");
    for bundle_id in CHROMIUM_BUNDLE_IDS {
        assert_eq!(
            classify_web_engine(bundle_id, missing),
            WebEngine::Chromium,
            "{bundle_id}"
        );
    }
    for bundle_id in ["com.apple.Safari", "org.mozilla.firefox", "com.google.Chromex", ""] {
        assert_eq!(classify_web_engine(bundle_id, missing), WebEngine::None, "{bundle_id}");
    }
}

#[test]
fn classify_generic_chromium_by_renderer_helper_bundle() {
    let temp = tempfile::tempdir().unwrap();

    let direct = temp.path().join("Direct.app");
    make_dirs(
        &direct,
        &["Contents/Frameworks/Some Browser Helper (Renderer).app"],
    );
    assert_eq!(
        classify_web_engine("com.example.direct", &direct),
        WebEngine::Chromium
    );

    // Chrome-style layout: helpers live inside the versioned framework.
    let nested = temp.path().join("Nested.app");
    make_dirs(
        &nested,
        &["Contents/Frameworks/Some Framework.framework/Versions/1.2.3/Helpers/Some Helper (Renderer).app"],
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        "1.2.3",
        nested.join("Contents/Frameworks/Some Framework.framework/Versions/Current"),
    )
    .unwrap();
    #[cfg(not(unix))]
    make_dirs(
        &nested,
        &["Contents/Frameworks/Some Framework.framework/Versions/Current/Helpers/Some Helper (Renderer).app"],
    );
    assert_eq!(
        classify_web_engine("com.example.nested", &nested),
        WebEngine::Chromium
    );
}

#[test]
fn classify_ordinary_and_unreadable_bundles_as_none() {
    let temp = tempfile::tempdir().unwrap();
    let plain = temp.path().join("Plain.app");
    make_dirs(
        &plain,
        &[
            "Contents/Frameworks/Sparkle.framework/Versions/A",
            "Contents/Frameworks/Helper.app",
            "Contents/MacOS",
        ],
    );
    assert_eq!(classify_web_engine("com.example.plain", &plain), WebEngine::None);
    let bare = temp.path().join("Bare.app");
    make_dirs(&bare, &["Contents/MacOS"]);
    assert_eq!(classify_web_engine("com.example.bare", &bare), WebEngine::None);
    // Only a renderer *app bundle* name counts, not an arbitrary similar file.
    let decoy = temp.path().join("Decoy.app");
    make_dirs(&decoy, &["Contents/Frameworks"]);
    fs::write(decoy.join("Contents/Frameworks/X Helper (Renderer).txt"), b"").unwrap();
    assert_eq!(classify_web_engine("com.example.decoy", &decoy), WebEngine::None);
}

#[test]
fn classify_scan_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let big = temp.path().join("Big.app");
    for index in 0..(MAX_BUNDLE_SCAN_ENTRIES * 3) {
        fs::create_dir_all(big.join(format!("Contents/Frameworks/Plain{index}.framework")))
            .unwrap();
    }
    assert_eq!(classify_web_engine("com.example.big", &big), WebEngine::None);
}

#[test]
fn policy_parses_only_off_case_insensitively() {
    use WebAccessibilityPolicy::{Auto, Off};
    assert_eq!(WebAccessibilityPolicy::from_env_value(None), Auto);
    assert_eq!(WebAccessibilityPolicy::from_env_value(Some("off")), Off);
    assert_eq!(WebAccessibilityPolicy::from_env_value(Some(" OFF ")), Off);
    for other in ["", "on", "auto", "0", "false", "disabled", "offf"] {
        assert_eq!(WebAccessibilityPolicy::from_env_value(Some(other)), Auto, "{other}");
    }
    assert_eq!(WebAccessibilityPolicy::default(), Auto);
}

#[test]
fn state_strings_match_the_wire_vocabulary() {
    let all = [
        (WebAxState::NotApplicable, "not_applicable"),
        (WebAxState::Enabled, "enabled"),
        (WebAxState::AlreadyEnabled, "already_enabled"),
        (WebAxState::Pending, "pending"),
        (WebAxState::Unsupported, "unsupported"),
        (WebAxState::Disabled, "disabled"),
        (WebAxState::SkippedSensitive, "skipped_sensitive"),
    ];
    for (state, text) in all {
        assert_eq!(state.as_str(), text);
    }
}

// ---------------------------------------------------------------------------
// wait / enable logic with an injected environment
// ---------------------------------------------------------------------------

struct FakeEnvironment {
    set_calls: Cell<usize>,
    set_outcome: RefCell<Result<SetOutcome, String>>,
    probes: RefCell<Vec<Result<WebProbe, String>>>,
    probe_calls: Cell<usize>,
    probe_budgets: RefCell<Vec<Duration>>,
    /// Time one probe takes (capped by the budget it is given, like the real probe).
    probe_cost: Cell<Duration>,
    sleeps: RefCell<Vec<Duration>>,
    remaining: Cell<Duration>,
}

impl FakeEnvironment {
    fn new(probes: Vec<WebProbe>) -> Self {
        Self::with_results(probes.into_iter().map(Ok).collect())
    }

    fn with_results(probes: Vec<Result<WebProbe, String>>) -> Self {
        Self {
            set_calls: Cell::new(0),
            set_outcome: RefCell::new(Ok(SetOutcome::Set)),
            probes: RefCell::new(probes),
            probe_calls: Cell::new(0),
            probe_budgets: RefCell::new(Vec::new()),
            probe_cost: Cell::new(Duration::ZERO),
            sleeps: RefCell::new(Vec::new()),
            remaining: Cell::new(Duration::from_secs(10)),
        }
    }

    fn total_sleep(&self) -> Duration {
        self.sleeps.borrow().iter().sum()
    }
}

impl WebAxEnvironment for FakeEnvironment {
    fn set_manual_accessibility(&self) -> Result<SetOutcome, String> {
        self.set_calls.set(self.set_calls.get() + 1);
        self.set_outcome.borrow().clone()
    }

    fn probe(&self, budget: Duration) -> Result<WebProbe, String> {
        self.probe_calls.set(self.probe_calls.get() + 1);
        self.probe_budgets.borrow_mut().push(budget);
        let spent = self.probe_cost.get().min(budget);
        self.remaining.set(self.remaining.get().saturating_sub(spent));
        let mut probes = self.probes.borrow_mut();
        if probes.len() > 1 {
            probes.remove(0)
        } else {
            probes.first().cloned().unwrap_or(Ok(WebProbe::Empty))
        }
    }

    fn sleep(&self, duration: Duration) {
        self.sleeps.borrow_mut().push(duration);
        self.remaining
            .set(self.remaining.get().saturating_sub(duration));
    }

    fn remaining(&self) -> Duration {
        self.remaining.get()
    }
}

fn context<'a>(
    registry: &'a WebAxRegistry,
    policy: WebAccessibilityPolicy,
    sensitive_surface: bool,
) -> WebAxContext<'a> {
    WebAxContext {
        policy,
        sensitive_surface,
        registry,
    }
}

const PROCESS: ProcessKey = (4242, Some(1_700_000_000));

fn chromium() -> Option<(WebEngine, ProcessKey)> {
    Some((WebEngine::Chromium, PROCESS))
}

#[test]
fn first_observation_waits_for_content_then_remembers_the_process() {
    let registry = WebAxRegistry::default();
    let env = FakeEnvironment::new(vec![WebProbe::Empty, WebProbe::Empty, WebProbe::Content]);
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    let state = enable_web_accessibility(&env, &ctx, chromium).unwrap();
    assert_eq!(state, WebAxState::Enabled);
    assert_eq!(env.set_calls.get(), 1);
    assert_eq!(env.probe_calls.get(), 3);
    assert_eq!(
        *env.sleeps.borrow(),
        vec![Duration::from_millis(150), Duration::from_millis(300)]
    );
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Confirmed));
}

#[test]
fn confirmed_process_is_re_verified_with_one_non_sleeping_probe() {
    let registry = WebAxRegistry::default();
    registry.set_memo(PROCESS, WebMemo::Confirmed);
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);

    // Content still there: already_enabled, no waiting.
    let env = FakeEnvironment::new(vec![WebProbe::Content]);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::AlreadyEnabled
    );
    // Chromium can auto-disable accessibility, so the attribute is re-applied each time.
    assert_eq!(env.set_calls.get(), 1);
    assert_eq!(env.probe_calls.get(), 1);
    assert!(env.sleeps.borrow().is_empty());
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Confirmed));

    // The tree went away (auto-disable): pending, no waiting, memo downgraded.
    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Pending
    );
    assert_eq!(env.probe_calls.get(), 1);
    assert!(env.sleeps.borrow().is_empty());
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Waited));

    // And it comes back: upgraded again.
    let env = FakeEnvironment::new(vec![WebProbe::Content]);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::AlreadyEnabled
    );
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Confirmed));
}

#[test]
fn waited_process_probes_once_without_sleeping_and_never_claims_unconfirmed_content() {
    let registry = WebAxRegistry::default();
    registry.set_memo(PROCESS, WebMemo::Waited);
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);

    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Pending
    );
    assert_eq!((env.set_calls.get(), env.probe_calls.get()), (1, 1));
    assert!(env.sleeps.borrow().is_empty());
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Waited));

    let env = FakeEnvironment::new(vec![WebProbe::Content]);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::AlreadyEnabled
    );
    assert!(env.sleeps.borrow().is_empty());
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Confirmed));

    // A definite "no web area" also confirms (native Electron windows).
    registry.set_memo(PROCESS, WebMemo::Waited);
    let env = FakeEnvironment::new(vec![WebProbe::NoWebArea]);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::AlreadyEnabled
    );

    // Transient probe errors cannot confirm anything.
    for memo in [WebMemo::Waited, WebMemo::Confirmed] {
        registry.set_memo(PROCESS, memo);
        let env = FakeEnvironment::with_results(vec![Err(
            "accessibility_failed: x failed with AXError(-25202)".to_string(),
        )]);
        assert_eq!(
            enable_web_accessibility(&env, &ctx, chromium).unwrap(),
            WebAxState::Pending
        );
        assert_eq!(registry.memo(PROCESS), Some(WebMemo::Waited));
    }

    // Fatal errors still abort.
    let env = FakeEnvironment::with_results(vec![Err(
        "accessibility_failed: macOS Accessibility observation deadline exceeded".to_string(),
    )]);
    assert!(enable_web_accessibility(&env, &ctx, chromium)
        .unwrap_err()
        .contains("deadline exceeded"));
}

#[test]
fn memoized_process_without_probe_budget_reports_only_what_it_knows() {
    let registry = WebAxRegistry::default();
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    for memo in [WebMemo::Confirmed, WebMemo::Waited] {
        registry.set_memo(PROCESS, memo);
        let env = FakeEnvironment::new(vec![WebProbe::Content]);
        env.remaining.set(WAIT_RESERVE);
        assert_eq!(
            enable_web_accessibility(&env, &ctx, chromium).unwrap(),
            WebAxState::Pending
        );
        assert_eq!(env.probe_calls.get(), 0);
        assert_eq!(registry.memo(PROCESS), Some(memo));
    }
}

#[test]
fn recycled_pid_with_a_different_launch_time_is_not_remembered() {
    let registry = WebAxRegistry::default();
    registry.set_memo((4242, Some(1)), WebMemo::Confirmed);
    assert_eq!(registry.memo((4242, Some(2))), None);
    assert_eq!(registry.memo((4242, None)), None);
    assert_eq!(registry.memo((4242, Some(1))), Some(WebMemo::Confirmed));
}

#[test]
fn never_ready_content_stays_pending_and_later_observations_do_not_wait_again() {
    let registry = WebAxRegistry::default();
    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    let state = enable_web_accessibility(&env, &ctx, chromium).unwrap();
    assert_eq!(state, WebAxState::Pending);
    assert_eq!(env.probe_calls.get(), WAIT_SCHEDULE.len());
    assert_eq!(env.total_sleep(), Duration::from_millis(1950));
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Waited));
    // Every later observation costs one probe, no sleeping, and still says pending.
    for _ in 0..3 {
        let again = FakeEnvironment::new(vec![WebProbe::Empty]);
        assert_eq!(
            enable_web_accessibility(&again, &ctx, chromium).unwrap(),
            WebAxState::Pending
        );
        assert_eq!(again.set_calls.get(), 1);
        assert_eq!(again.probe_calls.get(), 1);
        assert!(again.sleeps.borrow().is_empty());
    }
}

#[test]
fn definite_absence_of_a_web_area_counts_as_enabled_without_waiting() {
    let registry = WebAxRegistry::default();
    let env = FakeEnvironment::new(vec![WebProbe::NoWebArea]);
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, || Some((WebEngine::Electron, PROCESS))).unwrap(),
        WebAxState::Enabled
    );
    assert_eq!(env.probe_calls.get(), 1);
    assert!(env.sleeps.borrow().is_empty());
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Confirmed));
}

#[test]
fn waiting_shrinks_to_leave_the_traversal_reserve() {
    let registry = WebAxRegistry::default();
    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    env.remaining.set(WAIT_RESERVE + Duration::from_millis(200));
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Pending
    );
    assert!(env.total_sleep() <= Duration::from_millis(200), "{:?}", env.total_sleep());
    assert!(env.remaining() >= WAIT_RESERVE);
    assert!(env.probe_calls.get() < WAIT_SCHEDULE.len());
    // Ran out of budget before finishing the schedule: remembered as "waited", so the
    // next observation costs one probe instead of the same budget again.
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Waited));

    // No spare time at all: no sleeping and no probing.
    let registry = WebAxRegistry::default();
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    env.remaining.set(WAIT_RESERVE);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Pending
    );
    assert!(env.sleeps.borrow().is_empty());
    assert_eq!(env.probe_calls.get(), 0);
    assert_eq!(env.set_calls.get(), 1);
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Waited));
}

#[test]
fn slow_probes_count_against_the_wait_budget_and_the_traversal_reserve() {
    let registry = WebAxRegistry::default();
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    // A slow browser: every probe burns whatever budget it is given (up to 2 s).
    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    env.probe_cost.set(Duration::from_secs(2));
    let before = env.remaining();
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Pending
    );
    let used = before - env.remaining();
    assert!(used <= WAIT_TOTAL_BUDGET, "{used:?}");
    assert!(env.remaining() >= WAIT_RESERVE);
    // No probe was ever given more time than was left in the budget.
    for budget in env.probe_budgets.borrow().iter() {
        assert!(*budget <= WAIT_TOTAL_BUDGET && !budget.is_zero(), "{budget:?}");
    }
    assert!(env.probe_calls.get() < WAIT_SCHEDULE.len());
    assert_eq!(registry.memo(PROCESS), Some(WebMemo::Waited));

    // Even with a tight deadline the probe budget never reaches into the reserve.
    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    env.probe_cost.set(Duration::from_secs(5));
    env.remaining.set(WAIT_RESERVE + Duration::from_secs(1));
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Pending
    );
    assert!(env.remaining() >= WAIT_RESERVE, "{:?}", env.remaining());
}

#[test]
fn off_policy_never_identifies_the_process_or_writes_the_attribute() {
    let registry = WebAxRegistry::default();
    let env = FakeEnvironment::new(vec![WebProbe::Content]);
    let ctx = context(&registry, WebAccessibilityPolicy::Off, false);
    let identified = Cell::new(false);
    let state = enable_web_accessibility(&env, &ctx, || {
        identified.set(true);
        chromium()
    })
    .unwrap();
    assert_eq!(state, WebAxState::Disabled);
    assert_eq!(env.set_calls.get(), 0);
    assert_eq!(env.probe_calls.get(), 0);
    assert!(!identified.get());
}

#[test]
fn sensitive_surface_never_identifies_the_process_or_writes_the_attribute() {
    let registry = WebAxRegistry::default();
    let env = FakeEnvironment::new(vec![WebProbe::Content]);
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, true);
    let identified = Cell::new(false);
    let state = enable_web_accessibility(&env, &ctx, || {
        identified.set(true);
        chromium()
    })
    .unwrap();
    assert_eq!(state, WebAxState::SkippedSensitive);
    assert_eq!(env.set_calls.get(), 0);
    assert!(!identified.get());
    // Off wins over sensitivity in the report (nothing is written either way).
    let ctx = context(&registry, WebAccessibilityPolicy::Off, true);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Disabled
    );
    assert_eq!(env.set_calls.get(), 0);
}

#[test]
fn non_web_apps_are_left_alone() {
    let registry = WebAxRegistry::default();
    let env = FakeEnvironment::new(vec![WebProbe::Content]);
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    for identify in [None, Some((WebEngine::None, PROCESS))] {
        assert_eq!(
            enable_web_accessibility(&env, &ctx, || identify).unwrap(),
            WebAxState::NotApplicable
        );
    }
    assert_eq!(env.set_calls.get(), 0);
}

#[test]
fn unsupported_attribute_degrades_to_a_successful_observation() {
    let registry = WebAxRegistry::default();
    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    *env.set_outcome.borrow_mut() = Ok(SetOutcome::Unsupported);
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Unsupported
    );
    assert_eq!(env.probe_calls.get(), 0);
    assert!(env.sleeps.borrow().is_empty());
}

#[test]
fn only_permission_and_deadline_errors_abort_the_observation() {
    let registry = WebAxRegistry::default();
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    let deadline = "accessibility_failed: macOS Accessibility observation deadline exceeded";
    let transient = "accessibility_failed: AXUIElementCopyAttributeValue failed with AXError(-25202)";

    // Setting the attribute.
    let env = FakeEnvironment::new(vec![WebProbe::Empty]);
    *env.set_outcome.borrow_mut() = Ok(SetOutcome::PermissionDenied);
    let error = enable_web_accessibility(&env, &ctx, chromium).unwrap_err();
    assert!(error.starts_with("permission_denied:"), "{error}");
    *env.set_outcome.borrow_mut() = Err(deadline.to_string());
    let error = enable_web_accessibility(&env, &ctx, chromium).unwrap_err();
    assert!(error.contains("deadline exceeded"), "{error}");
    *env.set_outcome.borrow_mut() = Err(transient.to_string());
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Unsupported
    );

    // Probing.
    for fatal in [deadline, "permission_denied: macOS Accessibility permission is not granted"] {
        let env = FakeEnvironment::with_results(vec![Err(fatal.to_string())]);
        let error = enable_web_accessibility(&env, &ctx, chromium).unwrap_err();
        assert_eq!(error, fatal);
    }
    // Chromium rebuilding its tree answers with transient errors: keep waiting.
    let env = FakeEnvironment::with_results(vec![
        Err(transient.to_string()),
        Err(transient.to_string()),
        Ok(WebProbe::Content),
    ]);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Enabled
    );
    assert_eq!(env.probe_calls.get(), 3);
    let registry = WebAxRegistry::default();
    let ctx = context(&registry, WebAccessibilityPolicy::Auto, false);
    let env = FakeEnvironment::with_results(vec![Err(transient.to_string())]);
    assert_eq!(
        enable_web_accessibility(&env, &ctx, chromium).unwrap(),
        WebAxState::Pending
    );
    assert_eq!(env.probe_calls.get(), WAIT_SCHEDULE.len());
}

#[test]
fn sensitive_web_surfaces_stop_at_the_web_area_boundary_only_when_we_enable_things() {
    use WebAccessibilityPolicy::{Auto, Off};
    for engine in [WebEngine::Chromium, WebEngine::Electron] {
        assert!(should_omit_web_content(Auto, true, engine));
        assert!(!should_omit_web_content(Auto, false, engine));
        assert!(!should_omit_web_content(Off, true, engine));
    }
    // Native apps and WebKit-only apps keep their historical exposure.
    assert!(!should_omit_web_content(Auto, true, WebEngine::None));
}

#[test]
fn registry_memo_and_engine_cache_are_bounded() {
    let registry = WebAxRegistry::default();
    for pid in 0..(MEMO_CAPACITY as u32 + 10) {
        registry.set_memo((pid, None), WebMemo::Confirmed);
    }
    assert_eq!(registry.memo((0, None)), None, "oldest entry is evicted");
    let newest = (MEMO_CAPACITY as u32 + 9, None);
    assert_eq!(registry.memo(newest), Some(WebMemo::Confirmed));
    assert_eq!(registry.memos.lock().unwrap().len(), MEMO_CAPACITY);
    // Setting twice updates in place instead of duplicating.
    registry.set_memo(newest, WebMemo::Waited);
    assert_eq!(registry.memo(newest), Some(WebMemo::Waited));
    assert_eq!(registry.memos.lock().unwrap().len(), MEMO_CAPACITY);

    for index in 0..(ENGINE_CACHE_CAPACITY + 5) {
        registry.cache_engine(Path::new(&format!("/apps/{index}.app")), WebEngine::Chromium);
    }
    assert!(registry.engines.lock().unwrap().len() <= ENGINE_CACHE_CAPACITY);
    registry.cache_engine(Path::new("/apps/x.app"), WebEngine::Electron);
    assert_eq!(
        registry.cached_engine(Path::new("/apps/x.app")),
        Some(WebEngine::Electron)
    );
    assert_eq!(registry.cached_engine(Path::new("/apps/unknown.app")), None);
}
