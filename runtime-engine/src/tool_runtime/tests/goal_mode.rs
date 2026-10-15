//! Goal Mode supervisor: deterministic fake executor with fault injection.
//!
//! These prove the supervisor's control logic (journal, lease fencing,
//! acceptance-only success, retries, stop semantics). They do not prove that
//! a real AI provider completes work; that is covered by the real-provider
//! end-to-end run documented in docs/PLAN_GOAL_MODE.md.

use crate::db::{
    AcceptanceCheck, Database, GoalBudget, GoalControlAction, GoalLease, GoalOpState,
    GoalRunState, GoalStartEnvironment, PlanApprovalBinding, PlanReader, PlanSpec,
    DEFAULT_PLAN_APPROVAL_TTL_MS,
};
use crate::tool_runtime::goal_supervisor::{
    BoxFut, ExecError, GoalExecutor, GoalProvider, GoalSupervisor, GoalSupervisorConfig,
    ObserveResult, ProcessOutcome, ProcessSpec, StartResult, Workspace,
};
use crate::db::GoalRunRecord;
use serde_json::{json,Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

const AGENT: &str = "fake-agent";

#[derive(Default)]
struct FakeState {
    agent_runs: u32,
    /// Acceptance passes once this many agent runs happened.
    fix_after_agent_runs: u32,
    /// Make failing output differ per call so the no-progress breaker stays off.
    vary_failure: bool,
    starts: Vec<ProcessSpec>,
    stops: Vec<String>,
    jobs: HashMap<String, (u32, ProcessOutcome)>,
    next_job: u32,
    agent_polls: u32,
    inject_not_started: u32,
    deny_agent: bool,
    lose_agent_job: bool,
    verify_calls: u32,
    preflight_denied: bool,
    active_work: bool,
    workspace_prepares: u32,
    verify_not_started: u32,
    commit_starts: u32,
    external_exit: Option<i64>,
    git_dir_override: Option<String>,
    /// When set, acceptance passes from this verify call on (ChatGPT mode,
    /// where no agent job runs).
    pass_from_verify: Option<u32>,
}

struct FakeExecutor {
    state: Mutex<FakeState>,
}

impl FakeExecutor {
    fn new(configure: impl FnOnce(&mut FakeState)) -> Arc<Self> {
        let mut state = FakeState {
            agent_polls: 2,
            ..Default::default()
        };
        configure(&mut state);
        Arc::new(Self {
            state: Mutex::new(state),
        })
    }

    fn agent_starts(&self) -> usize {
        self.state
            .lock()
            .unwrap()
            .starts
            .iter()
            .filter(|spec| spec.executable == AGENT)
            .count()
    }

    fn agent_prompts(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .starts
            .iter()
            .filter(|spec| spec.executable == AGENT)
            .filter_map(|spec| spec.stdin.clone())
            .collect()
    }
}

fn done(code: i64, stdout: &str, stderr: &str) -> ProcessOutcome {
    ProcessOutcome {
        exit_code: Some(code),
        timed_out: false,
        stdout_tail: stdout.to_string(),
        stderr_tail: stderr.to_string(),
    }
}

impl GoalExecutor for FakeExecutor {
    fn preflight<'a>(&'a self,_run:&'a GoalRunRecord,_workspace:&'a Workspace,provider:&'a GoalProvider)->BoxFut<'a,Result<GoalProvider,ExecError>> {
        Box::pin(async move {if self.state.lock().unwrap().preflight_denied {Err(ExecError::Denied("EXECUTABLE_MISSING: sandbox unavailable".into()))}else{Ok(provider.clone())}})
    }
    fn has_active_work<'a>(&'a self,_run:&'a GoalRunRecord,_workspace:&'a Workspace)->BoxFut<'a,bool>{Box::pin(async move {self.state.lock().unwrap().active_work})}

    fn prepare_workspace<'a>(&'a self, run: &'a GoalRunRecord) -> BoxFut<'a, Result<Workspace, ExecError>> {
        Box::pin(async move {
            self.state.lock().unwrap().workspace_prepares += 1;
            Ok(Workspace {
                project_id: format!("worktree:{}", run.goal_run_id),
                path: format!("/tmp/worktrees/{}", run.goal_run_id),
            })
        })
    }

    fn start<'a>(&'a self, _run: &'a GoalRunRecord, _workspace: &'a Workspace, spec: ProcessSpec) -> BoxFut<'a, Result<StartResult, ExecError>> {
        Box::pin(async move {
            let mut state = self.state.lock().unwrap();
            if state.inject_not_started > 0 {
                state.inject_not_started -= 1;
                state.starts.push(spec);
                return Err(ExecError::NotStarted("runner offline".to_string()));
            }
            state.starts.push(spec.clone());
            if spec.executable == AGENT {
                if state.deny_agent {
                    return Err(ExecError::Denied("insufficient_scope".to_string()));
                }
                state.agent_runs += 1;
                state.next_job += 1;
                let job = format!("job-{}", state.next_job);
                let polls = state.agent_polls;
                state
                    .jobs
                    .insert(job.clone(), (polls, done(0, "I fixed everything!\ntokens used\n1,250", "")));
                return Ok(StartResult::Started(job));
            }
            if spec.executable == "check" {
                if state.verify_not_started>0 {state.verify_not_started-=1;return Err(ExecError::NotStarted("SPAWN_ENOENT: check executable unavailable; command not started".into()));}
                state.verify_calls += 1;
                let passed = match state.pass_from_verify {
                    Some(from) => state.verify_calls >= from,
                    None => state.agent_runs >= state.fix_after_agent_runs,
                };
                let stderr = if state.vary_failure {
                    format!("assertion failed: attempt {}", state.verify_calls)
                } else {
                    "assertion failed: expected 4, got 5".to_string()
                };
                return Ok(StartResult::Finished(if passed {
                    done(0, "test result: ok", "")
                } else {
                    done(1, "test result: FAILED", &stderr)
                }));
            }
            if spec.executable == "external-check" {return Ok(StartResult::Finished(done(state.external_exit.unwrap_or(2),"","owner authorization is required")));}
            if let Some(payload)=spec.stdin.as_deref().and_then(|stdin|serde_json::from_str::<Value>(stdin).ok()).filter(|value|value["code"].as_str().is_some_and(|code|code.contains("git=[executable,'--literal-pathspecs'"))) {
                assert_eq!(&spec.args[..3], &["-I", "-S", "-c"]);
                let hardened:Vec<String>=serde_json::from_str(payload["args"][2].as_str().unwrap()).unwrap();
                assert!(hardened.contains(&"core.hooksPath=/dev/null".to_string()));
                assert!(hardened.iter().any(|arg|arg.starts_with("--git-dir=/repo/.git/worktrees/")));
                assert!(payload["code"].as_str().unwrap().contains("--exclude-standard"));
                return Ok(StartResult::Finished(done(0,"","")));
            }
            if spec.args.iter().any(|arg|arg=="commit") {state.commit_starts+=1;}
            if spec.args.iter().any(|arg| arg == "--verify") {
                return Ok(StartResult::Finished(done(0, "1111111111111111111111111111111111111111", "")));
            }
            if spec.args.iter().any(|arg| arg == "--format=%s") {
                return Ok(StartResult::Finished(done(0, &format!("chadex goal {}: iteration 1", _run.goal_run_id), "")));
            }
            if spec.args.iter().any(|arg| arg == "rev-parse") {
                let git_dir = state.git_dir_override.clone().unwrap_or_else(|| "/repo/.git/worktrees/repo-1".to_string());
                return Ok(StartResult::Finished(done(0, &git_dir, "")));
            }
            // checkpoint commit / branch: Chadex's git must be hardened.
            assert!(spec.args.contains(&"core.hooksPath=/dev/null".to_string()), "{:?}", spec.args);
            assert!(spec.args.iter().any(|arg| arg.starts_with("--git-dir=/repo/.git/worktrees/")), "{:?}", spec.args);
            Ok(StartResult::Finished(done(0, "", "")))
        })
    }

    fn observe<'a>(&'a self, _run: &'a GoalRunRecord, _workspace: &'a Workspace, job_id: &'a str) -> BoxFut<'a, Result<ObserveResult, ExecError>> {
        Box::pin(async move {
            let mut state = self.state.lock().unwrap();
            if state.lose_agent_job {
                return Ok(ObserveResult::Lost("runner restarted".to_string()));
            }
            let Some((polls, outcome)) = state.jobs.get_mut(job_id) else {
                return Ok(ObserveResult::Lost("job_not_found".to_string()));
            };
            if *polls > 0 {
                *polls -= 1;
                return Ok(ObserveResult::Running);
            }
            Ok(ObserveResult::Finished(outcome.clone()))
        })
    }

    fn stop<'a>(&'a self, _run: &'a GoalRunRecord, _workspace: &'a Workspace, job_id: &'a str) -> BoxFut<'a, Result<(), ExecError>> {
        Box::pin(async move {
            self.state.lock().unwrap().stops.push(job_id.to_string());
            Ok(())
        })
    }
}

fn config() -> GoalSupervisorConfig {
    GoalSupervisorConfig {
        tick: Duration::from_millis(10),
        lease_ms: 600,
        max_concurrent_runs: 4,
        transient_retry_delay: Duration::from_millis(5),
        transient_retry_limit: 3,
        observe_pause: Duration::from_millis(5),
    }
}

fn supervisor(db: &Arc<Database>, executor: Arc<FakeExecutor>) -> Arc<GoalSupervisor> {
    supervisor_with(db, executor, GoalProvider::custom_for_tests(&[AGENT]))
}

fn supervisor_with(db: &Arc<Database>, executor: Arc<FakeExecutor>, provider: GoalProvider) -> Arc<GoalSupervisor> {
    GoalSupervisor::new(
        db.clone(),
        executor,
        provider,
        config(),
        Arc::new(Notify::new()),
    )
}

/// Create a plan, approve it via the store, and start a run.
fn approved_run(db: &Database, budget: GoalBudget) -> String {
    approved_run_with(db, budget, "custom")
}

fn approved_run_with(db: &Database, budget: GoalBudget, provider: &str) -> String {
    approved_run_checks(db,budget,provider,vec![serde_json::from_value(json!({"name":"unit tests","argv":["check"],"timeout_secs":60})).unwrap()])
}

fn approved_run_checks(db:&Database,budget:GoalBudget,provider:&str,checks:Vec<AcceptanceCheck>)->String {
    let now = crate::tool_runtime::plan_mode::now_unix_ms();
    let owner = "owner-test";
    let plan = db.plan_start(owner, "agent:c:repo", "Fix the bug", now).unwrap();
    let saved = db
        .plan_save_revision(
            owner,
            &plan.plan.plan_id,
            0,
            "Fix the off-by-one in add()",
            PlanSpec {
                acceptance: checks,
                budget,
                ..Default::default()
            },
            now,
        )
        .unwrap();
    db.plan_approve(
        &plan.plan.plan_id,
        1,
        &saved.revision.content_sha256,
        PlanApprovalBinding {
            project_id: "agent:c:repo".to_string(),
            runner_client_id: "c".to_string(),
            project_root: "/repo".to_string(),
            base_commit: "0123".to_string(),
        },
        "operator",
        DEFAULT_PLAN_APPROVAL_TTL_MS,
        now,
    )
    .unwrap();
    db.goal_run_start(
        owner,
        &plan.plan.plan_id,
        &GoalStartEnvironment {
            runner_client_id: "c".to_string(),
            project_root: "/repo".to_string(),
        },
        &json!({"local_development": true}),
        provider,
        now,
    )
    .unwrap()
    .run
    .goal_run_id
}

async fn wait_for(db: &Database, run_id: &str, wanted: &[GoalRunState]) -> GoalRunRecord {
    for _ in 0..800 {
        let run = db.goal_run_read(PlanReader::Operator, run_id).unwrap().run;
        if wanted.contains(&run.state) {
            return run;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let detail = db.goal_run_read(PlanReader::Operator, run_id).unwrap();
    panic!("run did not reach {wanted:?}: {:?} / {:?}", detail.run.state, detail.run.state_reason);
}

const PARKED: &[GoalRunState] = &[
    GoalRunState::Succeeded,
    GoalRunState::Failed,
    GoalRunState::NeedsAttention,
    GoalRunState::Paused,
    GoalRunState::Cancelled,
    GoalRunState::Stopped,
];

#[tokio::test]
async fn failing_acceptance_drives_repair_and_success_requires_independent_checks() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    let fake = FakeExecutor::new(|state| state.fix_after_agent_runs = 2);
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::Succeeded, "{:?}", run.state_reason);
    // Iteration 1: agent claimed success, checks failed -> not accepted.
    assert_eq!(run.iteration, 2);
    assert_eq!(fake.agent_starts(), 2);
    assert_eq!(run.tokens_used, Some(2500));
    assert_eq!(run.workspace_branch.as_deref(), Some(format!("chadex/goal/{run_id}").as_str()));
    let prompts = fake.agent_prompts();
    assert!(!prompts[0].contains("did not pass acceptance"));
    assert!(prompts[1].contains("assertion failed: expected 4, got 5"));
    let detail = db.goal_run_read(PlanReader::Operator, &run_id).unwrap();
    let iterations: Vec<_> = detail.events.iter().filter(|e| e.kind == "iteration").collect();
    assert_eq!(iterations.len(), 2);
    assert_eq!(iterations[0].detail["all_passed"], json!(false));
    assert_eq!(iterations[1].detail["all_passed"], json!(true));
}

#[tokio::test]
async fn agent_self_report_never_counts_and_budgets_and_no_progress_stop_the_loop() {
    // Identical failures: the no-progress breaker parks after 3.
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget { max_iterations: 10, ..GoalBudget::default() });
    let fake = FakeExecutor::new(|state| state.fix_after_agent_runs = 99);
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().starts_with("no_progress"));
    assert_eq!(fake.agent_starts(), 3);
    assert!(fake.agent_prompts()[2].contains("try a different approach"));

    // Changing failures: the iteration budget ends it as failed.
    let (_tmp2, db2) = crate::test_support::test_db();
    let run_id = approved_run(&db2, GoalBudget { max_iterations: 3, ..GoalBudget::default() });
    let fake = FakeExecutor::new(|state| {
        state.fix_after_agent_runs = 99;
        state.vary_failure = true;
    });
    supervisor(&db2, fake.clone()).tick_once().await;
    let run = wait_for(&db2, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::Failed);
    assert!(run.state_reason.unwrap().starts_with("budget_iterations_exhausted"));
    assert_eq!(fake.agent_starts(), 3);
}

#[tokio::test]
async fn deadline_ends_the_run() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    db.goal_run_override_deadline(&run_id, 1).unwrap();
    let fake = FakeExecutor::new(|_| {});
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::Failed);
    assert!(run.state_reason.unwrap().starts_with("budget_time_exhausted"));
    assert_eq!(fake.agent_starts(), 0);
}

/// Simulate a supervisor that crashed after journaling an intent.
fn crashed_holder(db: &Database, run_id: &str, record_job: Option<&str>) {
    let now = crate::tool_runtime::plan_mode::now_unix_ms();
    let (_, lease) = db.goal_run_claim("crashed", now, 1, 1).unwrap().remove(0);
    let lease = GoalLease { owner: lease.owner, generation: lease.generation };
    db.goal_run_set_workspace(run_id, &lease, "worktree:x", "/tmp/x", "", now).unwrap();
    db.goal_op_intent(run_id, &lease, "agent:1", "agent", 1, now).unwrap();
    if let Some(job) = record_job {
        db.goal_op_dispatched(run_id, &lease, "agent:1", job).unwrap();
    }
}

#[tokio::test]
async fn restart_with_unknown_dispatch_never_redispatches_the_agent() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    crashed_holder(&db, &run_id, None);
    tokio::time::sleep(Duration::from_millis(10)).await; // lease (1 ms) expired
    let fake = FakeExecutor::new(|state| state.fix_after_agent_runs = 0);
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().starts_with("outcome_unknown"));
    assert_eq!(fake.agent_starts(), 0, "unknown outcome must not be dispatched again");
    let op = db.goal_op_get(&run_id, "agent:1").unwrap().unwrap();
    assert_eq!(op.state, GoalOpState::Unknown);

    // After review the operator resumes: the workspace is re-observed by the
    // commit and acceptance checks, still without re-running agent:1.
    db.goal_run_control(PlanReader::Operator, &run_id, GoalControlAction::Resume, crate::tool_runtime::plan_mode::now_unix_ms())
        .unwrap();
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, &[GoalRunState::Succeeded, GoalRunState::Failed, GoalRunState::NeedsAttention]).await;
    assert_eq!(run.state, GoalRunState::Succeeded, "{:?}", run.state_reason);
    assert_eq!(fake.agent_starts(), 0);
}

#[tokio::test]
async fn restart_with_recorded_job_reconciles_by_observing_it() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    crashed_holder(&db, &run_id, Some("job-77"));
    tokio::time::sleep(Duration::from_millis(10)).await;
    let fake = FakeExecutor::new(|state| state.fix_after_agent_runs = 0);
    fake.state
        .lock()
        .unwrap()
        .jobs
        .insert("job-77".to_string(), (1, done(0, "done", "")));
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::Succeeded, "{:?}", run.state_reason);
    assert_eq!(fake.agent_starts(), 0, "the recorded job is observed, not re-dispatched");
    assert_eq!(db.goal_op_get(&run_id, "agent:1").unwrap().unwrap().state, GoalOpState::Succeeded);
}

#[tokio::test]
async fn lost_agent_job_is_an_unknown_outcome() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    let fake = FakeExecutor::new(|state| state.lose_agent_job = true);
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().starts_with("outcome_unknown"));
    assert_eq!(fake.agent_starts(), 1);
}

#[tokio::test]
async fn concurrent_supervisors_dispatch_each_operation_once() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    let fake = FakeExecutor::new(|state| state.fix_after_agent_runs = 3);
    let a = supervisor(&db, fake.clone());
    let b = supervisor(&db, fake.clone());
    let c = supervisor(&db, fake.clone());
    let handles = [a.spawn(), b.spawn(), c.spawn()];
    let run = wait_for(&db, &run_id, PARKED).await;
    for handle in handles {
        handle.abort();
    }
    assert_eq!(run.state, GoalRunState::Succeeded, "{:?}", run.state_reason);
    assert_eq!(run.iteration, 3);
    assert_eq!(fake.agent_starts(), 3, "one agent dispatch per iteration across all supervisors");
}

#[tokio::test]
async fn permission_denial_is_not_retried() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    let fake = FakeExecutor::new(|state| state.deny_agent = true);
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().starts_with("permission_denied"));
    assert_eq!(fake.agent_starts(), 1);
}

#[tokio::test]
async fn transient_dispatch_failures_are_retried_with_a_bound() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    let fake = FakeExecutor::new(|state| state.inject_not_started = 2);
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::Succeeded, "{:?}", run.state_reason);

    let (_tmp2, db2) = crate::test_support::test_db();
    let run_id = approved_run(&db2, GoalBudget::default());
    let fake = FakeExecutor::new(|state| state.inject_not_started = 50);
    supervisor(&db2, fake.clone()).tick_once().await;
    let run = wait_for(&db2, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().starts_with("runner_unavailable"));
    assert_eq!(fake.state.lock().unwrap().starts.len(), 3, "bounded by transient_retry_limit");
}

#[tokio::test]
async fn cancel_pause_and_emergency_stop_stop_the_running_job() {
    for (action, expected) in [
        ("cancel", GoalRunState::Cancelled),
        ("pause", GoalRunState::Paused),
        ("emergency", GoalRunState::Stopped),
    ] {
        let (_tmp, db) = crate::test_support::test_db();
        let run_id = approved_run(&db, GoalBudget::default());
        let fake = FakeExecutor::new(|state| state.agent_polls = 100_000);
        supervisor(&db, fake.clone()).tick_once().await;
        // Wait until the agent job is running.
        for _ in 0..400 {
            if fake.agent_starts() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(fake.agent_starts(), 1);
        let now = crate::tool_runtime::plan_mode::now_unix_ms();
        match action {
            "cancel" => {
                db.goal_run_control(PlanReader::Owner("owner-test"), &run_id, GoalControlAction::Cancel, now)
                    .unwrap();
            }
            "pause" => {
                db.goal_run_control(PlanReader::Owner("owner-test"), &run_id, GoalControlAction::Pause, now)
                    .unwrap();
            }
            _ => {
                db.goal_kill_switch_set(true, "test", now).unwrap();
            }
        }
        let run = wait_for(&db, &run_id, PARKED).await;
        assert_eq!(run.state, expected, "{action}: {:?}", run.state_reason);
        assert_eq!(fake.state.lock().unwrap().stops, vec!["job-1".to_string()], "{action}");
        let detail = db.goal_run_read(PlanReader::Operator, &run_id).unwrap();
        assert!(detail.events.iter().any(|event| event.kind == "job_stopped"), "{action}");
    }
}

#[test]
fn provider_default_is_hardened_codex() {
    // Only meaningful when the operator has not overridden the argv.
    if std::env::var("CHADEX_GOAL_AGENT_ARGV").is_ok() {
        return;
    }
    let provider = GoalProvider::from_env();
    assert_eq!(provider.name(), "codex");
    let debug = format!("{provider:?}");
    for required in [
        "--ignore-user-config",
        "--ignore-rules",
        "workspace-write",
        "sandbox_workspace_write.network_access=false",
        "approval_policy=\\\"never\\\"",
    ] {
        assert!(debug.contains(required), "missing {required}: {debug}");
    }
    assert!(!debug.contains("danger"), "{debug}");
    if std::env::var("CHADEX_GOAL_CHECK_SANDBOX_ARGV").is_err() {
        let (executable, args) = provider.sandboxed_check(&["cargo".to_string(), "test".to_string()]).unwrap();
        assert_eq!(executable, "codex");
        assert_eq!(&args[..1], &["sandbox".to_string()]);
        assert!(args.contains(&"sandbox_workspace_write.network_access=false".to_string()));
        assert_eq!(&args[args.len() - 3..], &["--".to_string(), "cargo".to_string(), "test".to_string()]);
    }
}

#[test]
fn output_classifiers_are_conservative() {
    use crate::tool_runtime::goal_supervisor::{classify_observation, classify_start, parse_tokens_used};
    assert_eq!(parse_tokens_used("x\ntokens used\n12,345\n"), Some(12345));
    // Real Runner shape captured in the isolated end-to-end run.
    let workspace = crate::tool_runtime::goal_supervisor::parse_workspace(&json!({
        "id": "agent:device-1:calc-f4af179c-9cd965bf",
        "agent_project_id": "calc-f4af179c-9cd965bf",
        "path": "/data/managed-worktrees/calc-f4af179c",
        "kind": "auto_registered"
    }))
    .unwrap();
    assert_eq!(workspace.project_id, "agent:device-1:calc-f4af179c-9cd965bf");
    assert_eq!(workspace.path, "/data/managed-worktrees/calc-f4af179c");
    let id = crate::tool_runtime::goal_supervisor::workspace_operation_id("wc_grun_x");
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert_eq!(id, crate::tool_runtime::goal_supervisor::workspace_operation_id("wc_grun_x"));
    assert_ne!(id, crate::tool_runtime::goal_supervisor::workspace_operation_id("wc_grun_y"));
    assert_eq!(parse_tokens_used("tokens used: 77"), Some(77));
    assert_eq!(parse_tokens_used("no usage"), None);
    // Real compact success projection (captured end to end): exit 0.
    assert!(matches!(
        classify_start(true, &json!({"stdout_lines": 2, "stdout_tail": "ok"}), None),
        Ok(StartResult::Finished(outcome)) if outcome.exit_code == Some(0)
    ));
    assert!(matches!(
        classify_start(true, &json!({}), None),
        Ok(StartResult::Finished(outcome)) if outcome.exit_code == Some(0)
    ));
    // A job id without a terminal result is a started job.
    assert!(matches!(
        classify_start(true, &json!({"job_id": "j1", "promoted_to_job": true}), None),
        Ok(StartResult::Started(id)) if id == "j1"
    ));
    // A completed process with a non-zero exit is a finished failure.
    assert!(matches!(
        classify_start(false, &json!({"exit_code": 2, "stderr_tail": "boom"}), None),
        Ok(StartResult::Finished(outcome)) if outcome.exit_code == Some(2)
    ));
    // Denials are never retryable.
    assert!(matches!(
        classify_start(false, &json!({"error_kind": "plan_mode_read_only"}), None),
        Err(ExecError::Denied(_))
    ));
    // Known not-started is retryable; anything else is unknown.
    assert!(matches!(
        classify_start(false, &json!({"error_kind": "runner_offline", "command_started": false}), None),
        Err(ExecError::NotStarted(_))
    ));
    assert!(matches!(
        classify_start(false, &json!({"error_kind": "transport_lost"}), None),
        Err(ExecError::Unknown(_))
    ));
    assert!(matches!(
        classify_observation(&json!({"error_kind": "job_not_found"})),
        Ok(ObserveResult::Lost(_))
    ));
    assert!(matches!(
        classify_observation(&json!({"status": "running"})),
        Ok(ObserveResult::Running)
    ));
    assert!(matches!(
        classify_observation(&json!({"status": "completed", "terminal": true, "exit_code": 0})),
        Ok(ObserveResult::Finished(outcome)) if outcome.exit_code == Some(0)
    ));
}

#[tokio::test]
async fn review_n2_custom_provider_without_sandbox_never_runs_checks() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    let fake = FakeExecutor::new(|state| state.fix_after_agent_runs = 0);
    supervisor_with(&db, fake.clone(), GoalProvider::unsandboxed_custom_for_tests(&[AGENT]))
        .tick_once()
        .await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().starts_with("acceptance_sandbox_unconfigured"));
    assert_eq!(fake.state.lock().unwrap().verify_calls, 0);
}

#[tokio::test]
async fn review_n1_unexpected_git_dir_parks_before_agent_work() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    let fake = FakeExecutor::new(|state| state.git_dir_override = Some("/tmp/evil".to_string()));
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().starts_with("workspace_git_dir_unexpected"));
    assert_eq!(fake.agent_starts(), 0);
}

#[test]
fn review_n4_negative_fields_are_not_presumed_success() {
    use crate::tool_runtime::goal_supervisor::classify_start;
    for output in [
        json!({"command_ok": false}),
        json!({"passed": false}),
        json!({"terminal": false}),
        json!({"execution_state": "running"}),
    ] {
        assert!(classify_start(true, &output, None).is_err(), "{output}");
    }
}

#[tokio::test]
async fn chatgpt_mode_runs_checks_only_after_each_reported_turn_and_never_starts_an_agent() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run_with(&db, GoalBudget::default(), crate::db::CHATGPT_GOAL_PROVIDER);
    let fake = FakeExecutor::new(|state| state.pass_from_verify = Some(2));
    let sup = supervisor(&db, fake.clone());
    let owner = "owner-test";
    let read = |db: &Database| db.goal_run_read(PlanReader::Operator, &run_id).unwrap().run;

    // Before the worktree exists, a goal_check does not count as a turn.
    let early = db.goal_run_chatgpt_ready(owner, &run_id, crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();
    assert_eq!(early.chatgpt_ready_iteration, 0);

    // First claim prepares the worktree, then hands the lease back and waits.
    sup.tick_once().await;
    let mut run = read(&db);
    for _ in 0..400 {
        run = read(&db);
        if run.workspace_path.is_some() && run.lease_owner.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(run.workspace_path.is_some());
    assert_eq!(run.iteration, 0);
    assert_eq!(run.state, GoalRunState::Running);
    assert!(run.lease_owner.is_none(), "a waiting ChatGPT run holds no lease");
    assert_eq!(crate::tool_runtime::plan_mode::goal_run_projection(&run)["waiting_for_chatgpt"],true);
    let mut preparing=run.clone();preparing.state=GoalRunState::Queued;
    assert_eq!(crate::tool_runtime::plan_mode::goal_run_projection(&preparing)["waiting_for_chatgpt"],false);
    preparing.state=GoalRunState::Running;preparing.lease_owner=Some("preparation-lease".into());
    assert_eq!(crate::tool_runtime::plan_mode::goal_run_projection(&preparing)["waiting_for_chatgpt"],false);
    // Nothing to do: it is not claimed again.
    assert_eq!(sup.tick_once().await, 0);

    // Only the owner can report a turn; a Codex-mode run refuses it.
    assert_eq!(
        db.goal_run_chatgpt_ready("someone-else", &run_id, crate::tool_runtime::plan_mode::now_unix_ms())
            .unwrap_err()
            .code(),
        "goal_run_not_found"
    );
    let codex_run = approved_run(&db, GoalBudget::default());
    assert_eq!(
        db.goal_run_chatgpt_ready(owner, &codex_run, crate::tool_runtime::plan_mode::now_unix_ms())
            .unwrap_err()
            .code(),
        "goal_run_not_chatgpt_mode"
    );
    db.goal_run_control(PlanReader::Operator, &codex_run, GoalControlAction::Cancel, crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();

    // Turn 1: checks fail, iteration 1 recorded, back to waiting.
    let ready = db.goal_run_chatgpt_ready(owner, &run_id, crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();
    assert_eq!(ready.chatgpt_ready_iteration, 1);
    // Repeating it while the check is pending does not add a turn.
    let again = db.goal_run_chatgpt_ready(owner, &run_id, crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();
    assert_eq!(again.chatgpt_ready_iteration, 1);
    sup.tick_once().await;
    for _ in 0..400 {
        run = read(&db);
        if run.iteration == 1 && run.lease_owner.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(run.iteration, 1);
    assert_eq!(run.state, GoalRunState::Running);
    let feedback = crate::tool_runtime::goal_supervisor::acceptance_feedback(&db, &run_id, 1).unwrap();
    assert!(feedback.contains("assertion failed"), "{feedback}");

    // Asking again before ChatGPT has seen round 1's result starts nothing.
    let unseen = db.goal_run_chatgpt_ready(owner, &run_id, crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();
    assert_eq!(unseen.chatgpt_ready_iteration, 1);
    // goal_check shows round 1 to ChatGPT; its next call starts round 2.
    db.goal_run_chatgpt_reported(owner, &run_id, 1).unwrap();
    // Turn 2: checks pass, run succeeds with a result branch.
    let next = db.goal_run_chatgpt_ready(owner, &run_id, crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();
    assert_eq!(next.chatgpt_ready_iteration, 2);
    sup.tick_once().await;
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::Succeeded, "{:?}", run.state_reason);
    assert_eq!(run.iteration, 2);
    assert_eq!(fake.agent_starts(), 0, "ChatGPT mode never starts a Codex agent");
    assert!(run.workspace_branch.is_some());
}

#[tokio::test]
async fn chatgpt_mode_pauses_when_idle_and_a_new_turn_resumes_it() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run_with(&db, GoalBudget::default(), crate::db::CHATGPT_GOAL_PROVIDER);
    let fake = FakeExecutor::new(|state| state.pass_from_verify = Some(1));
    let sup = supervisor(&db, fake.clone());
    sup.tick_once().await;
    for _ in 0..400 {
        let run = db.goal_run_read(PlanReader::Operator, &run_id).unwrap().run;
        if run.workspace_path.is_some() && run.lease_owner.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // Pretend ChatGPT was last seen long ago.
    db.goal_run_override_chatgpt_seen(&run_id, 1).unwrap();
    assert_eq!(sup.tick_once().await, 1, "an idle ChatGPT run is claimed to be paused");
    let run = wait_for(&db, &run_id, PARKED).await;
    assert_eq!(run.state, GoalRunState::Paused);
    assert!(run.state_reason.as_deref().unwrap().starts_with("chatgpt_idle"));
    // The next goal_check resumes it and counts as the next turn.
    let resumed = db
        .goal_run_chatgpt_ready("owner-test", &run_id, crate::tool_runtime::plan_mode::now_unix_ms())
        .unwrap();
    assert_eq!(resumed.state, GoalRunState::Queued);
    assert_eq!(resumed.chatgpt_ready_iteration, 1);
    sup.tick_once().await;
    let run = wait_for(&db, &run_id, &[GoalRunState::Succeeded]).await;
    assert_eq!(run.iteration, 1);
}

#[tokio::test]
async fn controls_and_emergency_stop_apply_at_once_to_a_chatgpt_run_waiting_for_its_turn() {
    let now = crate::tool_runtime::plan_mode::now_unix_ms;
    let (_tmp, db) = crate::test_support::test_db();
    let fake = FakeExecutor::new(|state| state.pass_from_verify = Some(1));
    let sup = supervisor(&db, fake.clone());
    let waiting = |db: &Arc<Database>| {
        let run_id = approved_run_with(db, GoalBudget::default(), crate::db::CHATGPT_GOAL_PROVIDER);
        run_id
    };
    // Pause applies directly (no supervisor holds it); resume refreshes the
    // idle clock so it is not paused again for idleness right away.
    let run_id = waiting(&db);
    sup.tick_once().await;
    for _ in 0..400 {
        let run = db.goal_run_read(PlanReader::Operator, &run_id).unwrap().run;
        if run.workspace_path.is_some() && run.lease_owner.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let paused = db.goal_run_control(PlanReader::Operator, &run_id, GoalControlAction::Pause, now()).unwrap();
    assert_eq!(paused.state, GoalRunState::Paused);
    // A paused (not idle) run refuses goal_check.
    assert_eq!(
        db.goal_run_chatgpt_ready("owner-test", &run_id, now()).unwrap_err().code(),
        "goal_run_not_waiting"
    );
    db.goal_run_override_chatgpt_seen(&run_id, 1).unwrap();
    let resumed = db.goal_run_control(PlanReader::Operator, &run_id, GoalControlAction::Resume, now()).unwrap();
    assert!(resumed.chatgpt_seen_at_unix_ms.unwrap() > 1);

    // Emergency stop stops a waiting ChatGPT run immediately.
    sup.tick_once().await;
    for _ in 0..400 {
        let run = db.goal_run_read(PlanReader::Operator, &run_id).unwrap().run;
        if run.state == GoalRunState::Running && run.lease_owner.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    db.goal_kill_switch_set(true, "test", now()).unwrap();
    let run = db.goal_run_read(PlanReader::Operator, &run_id).unwrap().run;
    assert_eq!(run.state, GoalRunState::Stopped);
    db.goal_kill_switch_set(false, "test", now()).unwrap();

    // Cancel also applies at once.
    let other = waiting(&db);
    sup.tick_once().await;
    for _ in 0..400 {
        let run = db.goal_run_read(PlanReader::Operator, &other).unwrap().run;
        if run.workspace_path.is_some() && run.lease_owner.is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let cancelled = db.goal_run_control(PlanReader::Operator, &other, GoalControlAction::Cancel, now()).unwrap();
    assert_eq!(cancelled.state, GoalRunState::Cancelled);
}


#[test]
fn windows_goal_acceptance_capability_fails_closed() {
    use crate::tool_runtime::goal_supervisor::windows_goal_acceptance_unavailable;
    assert!(windows_goal_acceptance_unavailable("windows", true).unwrap().starts_with("WINDOWS_GOAL_SANDBOX_UNAVAILABLE:"));
    assert_eq!(windows_goal_acceptance_unavailable("windows", false), None);
    assert_eq!(windows_goal_acceptance_unavailable("macos", true), None);
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_goal_is_parked_before_workspace_or_agent_work() {
    let (_tmp, db) = crate::test_support::test_db();
    let run_id = approved_run(&db, GoalBudget::default());
    let fake = FakeExecutor::new(|_| {});
    supervisor(&db, fake.clone()).tick_once().await;
    let run = wait_for(&db, &run_id, &[GoalRunState::NeedsAttention]).await;
    assert!(run.state_reason.as_deref().unwrap_or_default().starts_with("WINDOWS_GOAL_SANDBOX_UNAVAILABLE:"));
    assert_eq!(run.committed_rounds, 0);
    assert_eq!(run.checkpoint_sha, None);
    assert_eq!(fake.agent_starts(), 0);
    assert_eq!(fake.state.lock().unwrap().workspace_prepares, 0);
}

#[tokio::test]
async fn repair_preflight_missing_tool_blocks_before_any_agent_or_checkpoint() {
    let (_tmp,db)=crate::test_support::test_db();
    let run_id=approved_run(&db,GoalBudget::default());
    let fake=FakeExecutor::new(|state|state.preflight_denied=true);
    supervisor(&db,fake.clone()).tick_once().await;
    let run=wait_for(&db,&run_id,PARKED).await;
    assert_eq!(run.state,GoalRunState::NeedsAttention);
    assert_eq!(run.committed_rounds,0);
    assert_eq!(fake.agent_starts(),0);
    assert_eq!(fake.state.lock().unwrap().commit_starts,0);
    assert!(run.state_reason.unwrap().contains("preflight"));
}

#[tokio::test]
async fn repair_spawn_failure_retries_acceptance_at_same_checkpoint_without_agent_or_commit_replay() {
    let (_tmp,db)=crate::test_support::test_db();
    let run_id=approved_run(&db,GoalBudget::default());
    let fake=FakeExecutor::new(|state|{state.verify_not_started=100;state.fix_after_agent_runs=1;});
    let sup=supervisor(&db,fake.clone());
    sup.tick_once().await;
    let parked=wait_for(&db,&run_id,PARKED).await;
    assert_eq!(parked.state,GoalRunState::NeedsAttention);
    assert_eq!(parked.iteration,0);
    assert_eq!(parked.committed_rounds,1);
    assert_eq!(parked.completed_acceptance_rounds,0);
    assert!(parked.checkpoint_sha.is_some());
    assert_eq!(fake.state.lock().unwrap().commit_starts,1);
    assert!(db.goal_run_control(PlanReader::Owner("owner-test"),&run_id,GoalControlAction::RetryAcceptance,crate::tool_runtime::plan_mode::now_unix_ms()).is_err());
    fake.state.lock().unwrap().verify_not_started=0;
    db.goal_run_control(PlanReader::Operator,&run_id,GoalControlAction::RetryAcceptance,crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();
    sup.tick_once().await;
    let done=wait_for(&db,&run_id,&[GoalRunState::Succeeded]).await;
    assert_eq!(done.checkpoint_sha,parked.checkpoint_sha);
    assert_eq!(done.goal_run_id,run_id);
    assert_eq!(done.iteration,1);
    assert_eq!(fake.agent_starts(),1);
    assert_eq!(fake.state.lock().unwrap().commit_starts,1);
    assert_eq!(db.goal_acceptance_receipts(PlanReader::Operator,&run_id).unwrap().len(),1);
}

#[tokio::test]
async fn repair_external_exit_two_never_succeeds_but_independent_local_check_still_runs() {
    let (_tmp,db)=crate::test_support::test_db();
    let checks=vec![serde_json::from_value(json!({"name":"required owner gate","argv":["external-check"],"kind":"external_gate","lane":"external_attestation","external_gate":{"owner":"human","action":"real signature","evidence":"signed evidence","next_step":"request real owner"}})).unwrap(),
        serde_json::from_value(json!({"name":"independent local test","argv":["check"]})).unwrap()];
    let run_id=approved_run_checks(&db,GoalBudget::default(),"custom",checks);
    let fake=FakeExecutor::new(|state|state.fix_after_agent_runs=0);
    supervisor(&db,fake.clone()).tick_once().await;
    let run=wait_for(&db,&run_id,PARKED).await;
    assert_eq!(run.state,GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().contains("blocked_external"));
    assert_eq!(fake.state.lock().unwrap().verify_calls,1);
    let receipts=db.goal_acceptance_receipts(PlanReader::Operator,&run_id).unwrap();
    assert_eq!(receipts.len(),2);
    assert!(receipts.iter().any(|receipt|receipt.exit_code==Some(2)));
    assert_eq!(run.same_failure_streak,0);
}

#[tokio::test]
async fn repair_registered_activity_prevents_false_idle_pause_but_deadline_remains_enforced() {
    let (_tmp,db)=crate::test_support::test_db();
    let run_id=approved_run_with(&db,GoalBudget::default(),crate::db::CHATGPT_GOAL_PROVIDER);
    let fake=FakeExecutor::new(|state|state.active_work=true);
    let sup=supervisor(&db,fake.clone());
    sup.tick_once().await;
    for _ in 0..400 {if db.goal_run_read(PlanReader::Operator,&run_id).unwrap().run.workspace_path.is_some(){break}tokio::time::sleep(Duration::from_millis(5)).await;}
    let old=crate::tool_runtime::plan_mode::now_unix_ms()-crate::db::CHATGPT_GOAL_IDLE_MS-10;
    db.goal_run_override_chatgpt_seen(&run_id,old).unwrap();
    sup.tick_once().await;
    for _ in 0..400 {if db.goal_run_read(PlanReader::Operator,&run_id).unwrap().run.chatgpt_seen_at_unix_ms.is_some_and(|seen|seen>old){break}tokio::time::sleep(Duration::from_millis(5)).await;}
    let run=db.goal_run_read(PlanReader::Operator,&run_id).unwrap().run;
    assert_eq!(run.state,GoalRunState::Running);
    assert!(run.chatgpt_seen_at_unix_ms.unwrap()>old);
    db.goal_run_override_deadline(&run_id,1).unwrap();
    sup.tick_once().await;
    assert_eq!(wait_for(&db,&run_id,PARKED).await.state,GoalRunState::Failed);
}

#[tokio::test]
async fn repair_external_exit_zero_cannot_self_attest_a_formal_gate() {
    let (_tmp,db)=crate::test_support::test_db();
    let checks=vec![serde_json::from_value(json!({"name":"external authority","argv":["external-check"],"kind":"external_gate","lane":"external_attestation","external_gate":{"owner":"real owner","action":"witness","evidence":"missing.json","next_step":"real authorization"}})).unwrap(),
        serde_json::from_value(json!({"name":"independent test","argv":["check"]})).unwrap()];
    let run_id=approved_run_checks(&db,GoalBudget::default(),"custom",checks);
    let fake=FakeExecutor::new(|state|{state.external_exit=Some(0);state.fix_after_agent_runs=0});
    supervisor(&db,fake.clone()).tick_once().await;
    let run=wait_for(&db,&run_id,PARKED).await;
    assert_eq!(run.state,GoalRunState::NeedsAttention);
    assert!(run.state_reason.unwrap().contains("blocked_external"));
    assert_eq!(fake.state.lock().unwrap().verify_calls,1);
    let receipts=db.goal_acceptance_receipts(PlanReader::Operator,&run_id).unwrap();
    assert!(receipts.iter().any(|receipt|receipt.exit_code==Some(0) && receipt.outcome==crate::db::GoalAcceptanceOutcome::BlockedExternal));
}
