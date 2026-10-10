//! Goal Mode supervisor: drives approved Goal runs inside the Chadex runtime,
//! without any ChatGPT turn.
//!
//! Loop per iteration: agent work -> checkpoint commit -> independent
//! acceptance checks -> evaluate. Success is decided only by the approved
//! acceptance commands' exit codes, never by the agent's own report.
//!
//! Reliability contract:
//! - One active driver per run: a SQLite lease with a fence generation that
//!   every write re-checks.
//! - Every dispatch is journaled as `intent` before it leaves the process and
//!   `dispatched(job_id)` after. After a restart an `intent` without a job id
//!   for agent work is an unknown outcome: the run parks in `needs_attention`
//!   instead of dispatching the agent again. Checkpoint commits and acceptance
//!   checks are re-observable and simply run again under a new fence key.
//! - Bounded iterations, wall-clock deadline, per-process timeouts, transient
//!   retry limits, and a no-progress breaker. Permission/policy denials are
//!   never retried.
//! - Pause/cancel/emergency stop are honoured before every dispatch and while
//!   waiting; a running job is stopped and its result recorded.

use super::kernel::{ToolCallContext, ToolCallRequest, ToolTransport};
use super::plan_mode::{goal_dispatch_auth, now_unix_ms, with_goal_run_dispatch};
use super::ToolRuntime;
use crate::db::{
    Database, GoalLease, GoalOpState, GoalRunRecord, GoalRunState, PlanReader, PlanStoreError,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

pub(crate) type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const GOAL_AGENT_ARGV_ENV: &str = "CHADEX_GOAL_AGENT_ARGV";
const MAX_FEEDBACK_BYTES: usize = 6 * 1024;
const MAX_PROMPT_BYTES: usize = 60 * 1024;

/// The non-interactive agent the supervisor runs for each iteration. The
/// prompt is always passed on stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GoalProvider {
    name: &'static str,
    argv: Vec<String>,
}

impl GoalProvider {
    /// `codex exec` hardened for unattended work: user config (plugins, MCP
    /// servers, connectors) and user rules are ignored, the sandbox is
    /// workspace-write with network disabled, approvals never prompt, and no
    /// session is persisted. An operator may override the argv for testing or
    /// another CLI with `CHADEX_GOAL_AGENT_ARGV` (JSON array).
    pub(crate) fn from_env() -> Self {
        if let Some(argv) = std::env::var(GOAL_AGENT_ARGV_ENV)
            .ok()
            .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
            .filter(|argv| !argv.is_empty() && !argv[0].trim().is_empty())
        {
            return Self {
                name: "custom",
                argv,
            };
        }
        Self {
            name: "codex",
            argv: [
                "codex",
                "exec",
                "--ignore-user-config",
                "--ignore-rules",
                "--ephemeral",
                "--color",
                "never",
                "-s",
                "workspace-write",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "sandbox_workspace_write.network_access=false",
                "-C",
                ".",
                "-",
            ]
            .iter()
            .map(|value| value.to_string())
            .collect(),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        self.name
    }

    #[cfg(test)]
    pub(crate) fn custom_for_tests(argv: &[&str]) -> Self {
        Self {
            name: "custom",
            argv: argv.iter().map(|value| value.to_string()).collect(),
        }
    }

    fn spec(&self, prompt: String, timeout_secs: u32) -> ProcessSpec {
        ProcessSpec {
            executable: self.argv[0].clone(),
            args: self.argv[1..].to_vec(),
            stdin: Some(prompt),
            cwd: None,
            timeout_secs,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessSpec {
    pub(crate) executable: String,
    pub(crate) args: Vec<String>,
    pub(crate) stdin: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) timeout_secs: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessOutcome {
    pub(crate) exit_code: Option<i64>,
    pub(crate) timed_out: bool,
    pub(crate) stdout_tail: String,
    pub(crate) stderr_tail: String,
}

impl ProcessOutcome {
    fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    fn to_json(&self) -> Value {
        json!({
            "exit_code": self.exit_code,
            "timed_out": self.timed_out,
            "stdout_tail": tail_bytes(&self.stdout_tail, 4096),
            "stderr_tail": tail_bytes(&self.stderr_tail, 4096),
        })
    }

    fn from_json(value: &Value) -> Self {
        Self {
            exit_code: value.get("exit_code").and_then(Value::as_i64),
            timed_out: value.get("timed_out").and_then(Value::as_bool).unwrap_or(false),
            stdout_tail: value
                .get("stdout_tail")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            stderr_tail: value
                .get("stderr_tail")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartResult {
    Started(String),
    Finished(ProcessOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObserveResult {
    Running,
    Finished(ProcessOutcome),
    /// The job is gone without a result (e.g. Runner restarted).
    Lost(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ExecError {
    /// Definitely not dispatched; safe to retry (bounded).
    NotStarted(String),
    /// Authorization/policy/safety denial; never retried.
    Denied(String),
    /// May or may not have happened.
    Unknown(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Workspace {
    pub(crate) project_id: String,
    pub(crate) path: String,
}

/// Execution backend. Production dispatches through the normal tool kernel and
/// Runner; tests use a deterministic fake with fault injection.
pub(crate) trait GoalExecutor: Send + Sync {
    fn prepare_workspace<'a>(
        &'a self,
        run: &'a GoalRunRecord,
    ) -> BoxFut<'a, Result<Workspace, ExecError>>;
    fn start<'a>(
        &'a self,
        run: &'a GoalRunRecord,
        workspace: &'a Workspace,
        spec: ProcessSpec,
    ) -> BoxFut<'a, Result<StartResult, ExecError>>;
    fn observe<'a>(
        &'a self,
        run: &'a GoalRunRecord,
        workspace: &'a Workspace,
        job_id: &'a str,
    ) -> BoxFut<'a, Result<ObserveResult, ExecError>>;
    fn stop<'a>(
        &'a self,
        run: &'a GoalRunRecord,
        workspace: &'a Workspace,
        job_id: &'a str,
    ) -> BoxFut<'a, Result<(), ExecError>>;
}

#[derive(Debug, Clone)]
pub(crate) struct GoalSupervisorConfig {
    pub(crate) tick: Duration,
    pub(crate) lease_ms: i64,
    pub(crate) max_concurrent_runs: usize,
    pub(crate) transient_retry_delay: Duration,
    pub(crate) transient_retry_limit: u32,
    pub(crate) observe_pause: Duration,
}

impl Default for GoalSupervisorConfig {
    fn default() -> Self {
        Self {
            tick: Duration::from_secs(2),
            lease_ms: 60_000,
            max_concurrent_runs: 2,
            transient_retry_delay: Duration::from_secs(5),
            transient_retry_limit: 12,
            observe_pause: Duration::from_millis(500),
        }
    }
}

pub(crate) struct GoalSupervisor {
    db: Arc<Database>,
    executor: Arc<dyn GoalExecutor>,
    provider: GoalProvider,
    owner: String,
    config: GoalSupervisorConfig,
    wake: Arc<Notify>,
}

/// Why the driver stopped touching a run without a recorded transition.
#[derive(Debug)]
enum Exit {
    LeaseLost,
    Store(PlanStoreError),
}

impl From<PlanStoreError> for Exit {
    fn from(error: PlanStoreError) -> Self {
        if error.code() == "goal_lease_lost" {
            Exit::LeaseLost
        } else {
            Exit::Store(error)
        }
    }
}

/// A decision to park the run in a non-running state.
#[derive(Debug, Clone)]
struct Park {
    state: GoalRunState,
    reason: String,
}

impl Park {
    fn new(state: GoalRunState, reason: impl Into<String>) -> Self {
        Self {
            state,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpPolicy {
    /// Side effects that must never be blindly repeated (agent work).
    ExactlyOnce,
    /// Safe to repeat after an unknown outcome (checks, checkpoint commits).
    Reobservable,
}

enum OpOutcome {
    Done(ProcessOutcome),
    Park(Park),
}

impl GoalSupervisor {
    pub(crate) fn new(
        db: Arc<Database>,
        executor: Arc<dyn GoalExecutor>,
        provider: GoalProvider,
        config: GoalSupervisorConfig,
        wake: Arc<Notify>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            executor,
            provider,
            owner: format!("sup_{}", uuid::Uuid::new_v4().simple()),
            config,
            wake,
        })
    }

    /// Background loop: claim runnable runs and drive each in its own task.
    pub(crate) fn spawn(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                this.tick_once().await;
                tokio::select! {
                    _ = tokio::time::sleep(this.config.tick) => {}
                    _ = this.wake.notified() => {}
                }
            }
        })
    }

    /// One claim pass. Returns the number of runs claimed.
    pub(crate) async fn tick_once(self: &Arc<Self>) -> usize {
        let claimed = match self.db.goal_run_claim(
            &self.owner,
            now_unix_ms(),
            self.config.lease_ms,
            self.config.max_concurrent_runs,
        ) {
            Ok(claimed) => claimed,
            Err(error) => {
                tracing::warn!(error = %error, "goal supervisor claim failed");
                return 0;
            }
        };
        let count = claimed.len();
        for (run, lease) in claimed {
            let this = self.clone();
            tokio::spawn(async move { this.drive(run.goal_run_id, lease).await });
        }
        count
    }

    pub(crate) async fn drive(self: Arc<Self>, run_id: String, lease: GoalLease) {
        let lost = Arc::new(AtomicBool::new(false));
        let heartbeat = {
            let db = self.db.clone();
            let lost = lost.clone();
            let run_id = run_id.clone();
            let lease = lease.clone();
            let lease_ms = self.config.lease_ms;
            tokio::spawn(async move {
                let every = Duration::from_millis((lease_ms / 3).max(50) as u64);
                loop {
                    tokio::time::sleep(every).await;
                    if db
                        .goal_run_renew_lease(&run_id, &lease, now_unix_ms(), lease_ms)
                        .is_err()
                    {
                        lost.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            })
        };
        let result = self.drive_inner(&run_id, &lease, &lost).await;
        heartbeat.abort();
        match result {
            Ok(()) => {}
            Err(Exit::LeaseLost) => {
                tracing::info!(goal_run_id = %run_id, "goal run lease lost; another supervisor owns it");
            }
            Err(Exit::Store(error)) => {
                // Leave the run as is; the lease expires and a later claim retries.
                tracing::warn!(goal_run_id = %run_id, error = %error, "goal run store error");
            }
        }
    }

    fn event(&self, run_id: &str, kind: &str, detail: Value) {
        if let Err(error) = self.db.goal_run_event(run_id, kind, &detail, now_unix_ms()) {
            tracing::warn!(goal_run_id = %run_id, error = %error, "goal event write failed");
        }
    }

    fn park(&self, run_id: &str, lease: &GoalLease, park: Park) -> Result<(), Exit> {
        self.db
            .goal_run_transition(run_id, lease, park.state, &park.reason, now_unix_ms())?;
        Ok(())
    }

    /// Control/budget check at a safe point (before dispatch or while waiting).
    fn control_check(&self, run_id: &str, lease: &GoalLease, lost: &AtomicBool) -> Result<Option<Park>, Exit> {
        if lost.load(Ordering::SeqCst) {
            return Err(Exit::LeaseLost);
        }
        let run = self.db.goal_run_fenced(run_id, lease)?;
        if self.db.goal_kill_switch_engaged()? {
            return Ok(Some(Park::new(GoalRunState::Stopped, "emergency_stop")));
        }
        match run.control_request.as_deref() {
            Some("cancel") => return Ok(Some(Park::new(GoalRunState::Cancelled, "cancel requested"))),
            Some("pause") => return Ok(Some(Park::new(GoalRunState::Paused, "pause requested"))),
            _ => {}
        }
        if now_unix_ms() >= run.deadline_at_unix_ms {
            return Ok(Some(Park::new(
                GoalRunState::Failed,
                format!(
                    "budget_time_exhausted: {} minute limit reached",
                    run.spec.budget.max_minutes
                ),
            )));
        }
        Ok(None)
    }

    async fn drive_inner(&self, run_id: &str, lease: &GoalLease, lost: &AtomicBool) -> Result<(), Exit> {
        if let Some(park) = self.control_check(run_id, lease, lost)? {
            return self.park(run_id, lease, park);
        }
        let run = self.db.goal_run_fenced(run_id, lease)?;
        let workspace = match self.ensure_workspace(&run, lease, lost).await? {
            Ok(workspace) => workspace,
            Err(park) => return self.park(run_id, lease, park),
        };
        let mut tokens = run.tokens_used;
        loop {
            if let Some(park) = self.control_check(run_id, lease, lost)? {
                return self.park(run_id, lease, park);
            }
            let run = self.db.goal_run_fenced(run_id, lease)?;
            let iteration = run.iteration + 1;
            let max = i64::from(run.spec.budget.max_iterations);
            if iteration > max {
                self.make_result_branch(&run, &workspace, lease, lost).await?;
                return self.park(
                    run_id,
                    lease,
                    Park::new(
                        GoalRunState::Failed,
                        format!("budget_iterations_exhausted: acceptance still failing after {max} iterations"),
                    ),
                );
            }

            // 1. Agent work (exactly once per iteration).
            let feedback = self.previous_feedback(run_id, iteration - 1);
            let prompt = build_prompt(&self.db, &run, iteration, feedback.as_deref());
            let spec = self.provider.spec(prompt, run.spec.budget.agent_timeout_secs);
            let agent = match self
                .run_op(&run, &workspace, lease, lost, &format!("agent:{iteration}"), "agent", iteration, spec, OpPolicy::ExactlyOnce)
                .await?
            {
                OpOutcome::Done(outcome) => outcome,
                OpOutcome::Park(park) => return self.park(run_id, lease, park),
            };
            if let Some(used) = parse_tokens_used(&agent.stdout_tail).or_else(|| parse_tokens_used(&agent.stderr_tail)) {
                tokens = Some(tokens.unwrap_or(0) + used);
            }

            // 2. Checkpoint commit of whatever the agent changed.
            let commit_spec = ProcessSpec {
                executable: "sh".to_string(),
                args: vec![
                    "-c".to_string(),
                    format!(
                        "git add -A && git -c user.name='Chadex Goal' -c user.email=goal@chadex.local -c commit.gpgsign=false commit --no-verify --allow-empty -q -m 'chadex goal {}: iteration {}'",
                        run.goal_run_id, iteration
                    ),
                ],
                stdin: None,
                cwd: None,
                timeout_secs: 120,
            };
            let commit_key = format!("commit:{iteration}@g{}", lease.generation);
            match self
                .run_op(&run, &workspace, lease, lost, &commit_key, "commit", iteration, commit_spec, OpPolicy::Reobservable)
                .await?
            {
                OpOutcome::Done(outcome) if outcome.passed() => {}
                OpOutcome::Done(outcome) => {
                    return self.park(
                        run_id,
                        lease,
                        Park::new(
                            GoalRunState::NeedsAttention,
                            format!(
                                "checkpoint_commit_failed: exit {:?} {}",
                                outcome.exit_code,
                                tail_bytes(&outcome.stderr_tail, 300)
                            ),
                        ),
                    )
                }
                OpOutcome::Park(park) => return self.park(run_id, lease, park),
            }

            // 3. Independent acceptance checks.
            let mut results = Vec::new();
            for (index, check) in run.spec.acceptance.iter().enumerate() {
                let spec = ProcessSpec {
                    executable: check.argv[0].clone(),
                    args: check.argv[1..].to_vec(),
                    stdin: None,
                    cwd: check.cwd.clone(),
                    timeout_secs: check.timeout_secs,
                };
                let key = format!("verify:{iteration}:{index}@g{}", lease.generation);
                match self
                    .run_op(&run, &workspace, lease, lost, &key, "verify", iteration, spec, OpPolicy::Reobservable)
                    .await?
                {
                    OpOutcome::Done(outcome) => results.push((check.name.clone(), outcome)),
                    OpOutcome::Park(park) => return self.park(run_id, lease, park),
                }
            }
            let all_passed = !results.is_empty() && results.iter().all(|(_, outcome)| outcome.passed());
            let digest = failure_digest(&results);
            let streak = if !all_passed && run.last_failure_digest.as_deref() == Some(digest.as_str()) {
                run.same_failure_streak + 1
            } else {
                0
            };
            self.db.goal_run_progress(
                run_id,
                lease,
                iteration,
                streak,
                (!all_passed).then_some(digest.as_str()),
                tokens,
                now_unix_ms(),
            )?;
            self.event(
                run_id,
                "iteration",
                json!({
                    "iteration": iteration,
                    "agent_exit_code": agent.exit_code,
                    "agent_timed_out": agent.timed_out,
                    "acceptance": results.iter().map(|(name, outcome)| json!({
                        "name": name,
                        "passed": outcome.passed(),
                        "exit_code": outcome.exit_code,
                        "timed_out": outcome.timed_out,
                    })).collect::<Vec<_>>(),
                    "all_passed": all_passed,
                }),
            );
            if all_passed {
                let run = self.db.goal_run_fenced(run_id, lease)?;
                self.make_result_branch(&run, &workspace, lease, lost).await?;
                return self.park(
                    run_id,
                    lease,
                    Park::new(
                        GoalRunState::Succeeded,
                        format!("acceptance_passed: all {} checks passed in iteration {iteration}", results.len()),
                    ),
                );
            }
            if streak >= 2 {
                let run = self.db.goal_run_fenced(run_id, lease)?;
                self.make_result_branch(&run, &workspace, lease, lost).await?;
                return self.park(
                    run_id,
                    lease,
                    Park::new(
                        GoalRunState::NeedsAttention,
                        "no_progress: the same acceptance failure repeated in 3 consecutive iterations",
                    ),
                );
            }
        }
    }

    async fn ensure_workspace(
        &self,
        run: &GoalRunRecord,
        lease: &GoalLease,
        lost: &AtomicBool,
    ) -> Result<Result<Workspace, Park>, Exit> {
        if let (Some(project_id), Some(path)) = (&run.workspace_project_id, &run.workspace_path) {
            return Ok(Ok(Workspace {
                project_id: project_id.clone(),
                path: path.clone(),
            }));
        }
        let mut attempts = 0;
        loop {
            if let Some(park) = self.control_check(&run.goal_run_id, lease, lost)? {
                return Ok(Err(park));
            }
            // Idempotent on the Runner (operation id = run id), so a retry
            // after an unknown outcome re-observes the same worktree.
            match self.executor.prepare_workspace(run).await {
                Ok(workspace) => {
                    self.db.goal_run_set_workspace(
                        &run.goal_run_id,
                        lease,
                        &workspace.project_id,
                        &workspace.path,
                        "",
                        now_unix_ms(),
                    )?;
                    self.event(
                        &run.goal_run_id,
                        "workspace",
                        json!({"project": workspace.project_id, "path": workspace.path, "base_commit": run.binding.base_commit}),
                    );
                    return Ok(Ok(workspace));
                }
                Err(ExecError::Denied(message)) => {
                    return Ok(Err(Park::new(
                        GoalRunState::NeedsAttention,
                        format!("permission_denied: {message}"),
                    )))
                }
                Err(ExecError::NotStarted(message)) | Err(ExecError::Unknown(message)) => {
                    attempts += 1;
                    if attempts >= self.config.transient_retry_limit {
                        return Ok(Err(Park::new(
                            GoalRunState::NeedsAttention,
                            format!("workspace_unavailable: {message}"),
                        )));
                    }
                    tokio::time::sleep(self.config.transient_retry_delay).await;
                }
            }
        }
    }

    async fn make_result_branch(
        &self,
        run: &GoalRunRecord,
        workspace: &Workspace,
        lease: &GoalLease,
        lost: &AtomicBool,
    ) -> Result<(), Exit> {
        if run.iteration == 0 {
            return Ok(());
        }
        let branch = format!("chadex/goal/{}", run.goal_run_id);
        let spec = ProcessSpec {
            executable: "git".to_string(),
            args: vec!["branch".to_string(), "-f".to_string(), branch.clone(), "HEAD".to_string()],
            stdin: None,
            cwd: None,
            timeout_secs: 60,
        };
        let key = format!("branch@g{}", lease.generation);
        if let OpOutcome::Done(outcome) = self
            .run_op(run, workspace, lease, lost, &key, "branch", run.iteration, spec, OpPolicy::Reobservable)
            .await?
        {
            if outcome.passed() {
                self.db.goal_run_set_workspace(
                    &run.goal_run_id,
                    lease,
                    &workspace.project_id,
                    &workspace.path,
                    &branch,
                    now_unix_ms(),
                )?;
            }
        }
        Ok(())
    }

    fn previous_feedback(&self, run_id: &str, iteration: i64) -> Option<String> {
        if iteration < 1 {
            return None;
        }
        let detail = self.db.goal_run_read(PlanReader::Operator, run_id).ok()?;
        let mut feedback = String::new();
        for op in detail
            .ops
            .iter()
            .filter(|op| op.kind == "verify" && op.iteration == iteration)
        {
            let Some(result) = op.result.as_ref() else {
                continue;
            };
            let outcome = ProcessOutcome::from_json(result);
            if outcome.passed() {
                continue;
            }
            let name = result.get("name").and_then(Value::as_str).unwrap_or(&op.op_key);
            feedback.push_str(&format!(
                "- {name}: exit {:?}{}\n  stdout (tail):\n{}\n  stderr (tail):\n{}\n",
                outcome.exit_code,
                if outcome.timed_out { " (timed out)" } else { "" },
                indent(&tail_bytes(&outcome.stdout_tail, 1500)),
                indent(&tail_bytes(&outcome.stderr_tail, 1500)),
            ));
        }
        (!feedback.is_empty()).then(|| tail_bytes(&feedback, MAX_FEEDBACK_BYTES))
    }

    /// Execute one journaled operation and wait for its terminal outcome.
    #[allow(clippy::too_many_arguments)]
    async fn run_op(
        &self,
        run: &GoalRunRecord,
        workspace: &Workspace,
        lease: &GoalLease,
        lost: &AtomicBool,
        op_key: &str,
        kind: &str,
        iteration: i64,
        spec: ProcessSpec,
        policy: OpPolicy,
    ) -> Result<OpOutcome, Exit> {
        let run_id = run.goal_run_id.as_str();
        let label = spec_label(kind, &spec);
        let job_id = match self.db.goal_op_get(run_id, op_key)? {
            Some(op) => match op.state {
                GoalOpState::Succeeded | GoalOpState::Failed => {
                    return Ok(OpOutcome::Done(ProcessOutcome::from_json(
                        op.result.as_ref().unwrap_or(&Value::Null),
                    )))
                }
                // Only reachable after the operator reviewed and resumed: the
                // workspace is re-observed by the following commit and checks.
                GoalOpState::Unknown => {
                    return Ok(OpOutcome::Done(ProcessOutcome::from_json(&Value::Null)))
                }
                GoalOpState::Dispatched => match op.job_id {
                    Some(job_id) => job_id,
                    None => return Ok(OpOutcome::Park(self.unknown(run_id, lease, op_key, "dispatched without job id")?)),
                },
                GoalOpState::Intent => {
                    // Crashed between journaling and recording the dispatch.
                    let park = self.unknown(run_id, lease, op_key, "process restarted before the dispatch result was recorded")?;
                    return Ok(if policy == OpPolicy::ExactlyOnce {
                        OpOutcome::Park(park)
                    } else {
                        OpOutcome::Done(ProcessOutcome::from_json(&Value::Null))
                    });
                }
            },
            None => {
                if let Some(park) = self.control_check(run_id, lease, lost)? {
                    return Ok(OpOutcome::Park(park));
                }
                self.db
                    .goal_op_intent(run_id, lease, op_key, kind, iteration, now_unix_ms())?;
                let mut attempts = 0;
                loop {
                    match self.executor.start(run, workspace, spec.clone()).await {
                        Ok(StartResult::Started(job_id)) => {
                            self.db.goal_op_dispatched(run_id, lease, op_key, &job_id)?;
                            break job_id;
                        }
                        Ok(StartResult::Finished(outcome)) => {
                            return self.finish(run_id, lease, op_key, &label, outcome);
                        }
                        Err(ExecError::NotStarted(message)) => {
                            attempts += 1;
                            if attempts >= self.config.transient_retry_limit {
                                self.db.goal_op_finish(run_id, lease, op_key, GoalOpState::Failed, &json!({"error": message, "dispatched": false}), now_unix_ms())?;
                                return Ok(OpOutcome::Park(Park::new(
                                    GoalRunState::NeedsAttention,
                                    format!("runner_unavailable: {message}"),
                                )));
                            }
                            if let Some(park) = self.control_check(run_id, lease, lost)? {
                                self.db.goal_op_finish(run_id, lease, op_key, GoalOpState::Failed, &json!({"error": "stopped before dispatch", "dispatched": false}), now_unix_ms())?;
                                return Ok(OpOutcome::Park(park));
                            }
                            tokio::time::sleep(self.config.transient_retry_delay).await;
                        }
                        Err(ExecError::Denied(message)) => {
                            self.db.goal_op_finish(run_id, lease, op_key, GoalOpState::Failed, &json!({"error": message, "denied": true}), now_unix_ms())?;
                            return Ok(OpOutcome::Park(Park::new(
                                GoalRunState::NeedsAttention,
                                format!("permission_denied: {message}"),
                            )));
                        }
                        Err(ExecError::Unknown(message)) => {
                            let park = self.unknown(run_id, lease, op_key, &message)?;
                            return Ok(if policy == OpPolicy::ExactlyOnce {
                                OpOutcome::Park(park)
                            } else {
                                OpOutcome::Done(ProcessOutcome::from_json(&Value::Null))
                            });
                        }
                    }
                }
            }
        };

        // Wait for the dispatched job.
        let mut transient = 0;
        loop {
            if let Some(park) = self.control_check(run_id, lease, lost)? {
                let stopped = self.executor.stop(run, workspace, &job_id).await;
                self.db.goal_op_finish(
                    run_id,
                    lease,
                    op_key,
                    GoalOpState::Failed,
                    &json!({"stopped": park.reason, "stop_confirmed": stopped.is_ok()}),
                    now_unix_ms(),
                )?;
                self.event(run_id, "job_stopped", json!({"op": op_key, "job_id": job_id, "reason": park.reason, "stop_confirmed": stopped.is_ok()}));
                return Ok(OpOutcome::Park(park));
            }
            match self.executor.observe(run, workspace, &job_id).await {
                Ok(ObserveResult::Running) => {
                    transient = 0;
                    tokio::time::sleep(self.config.observe_pause).await;
                }
                Ok(ObserveResult::Finished(outcome)) => {
                    return self.finish(run_id, lease, op_key, &label, outcome);
                }
                Ok(ObserveResult::Lost(message)) => {
                    let park = self.unknown(run_id, lease, op_key, &format!("job lost: {message}"))?;
                    return Ok(if policy == OpPolicy::ExactlyOnce {
                        OpOutcome::Park(park)
                    } else {
                        OpOutcome::Done(ProcessOutcome::from_json(&Value::Null))
                    });
                }
                Err(ExecError::Denied(message)) => {
                    return Ok(OpOutcome::Park(Park::new(
                        GoalRunState::NeedsAttention,
                        format!("permission_denied while observing: {message}"),
                    )));
                }
                Err(ExecError::NotStarted(message)) | Err(ExecError::Unknown(message)) => {
                    transient += 1;
                    if transient >= self.config.transient_retry_limit {
                        // Leave the op as dispatched: after review the job is
                        // observed again instead of dispatched again.
                        return Ok(OpOutcome::Park(Park::new(
                            GoalRunState::NeedsAttention,
                            format!("runner_unavailable while observing job {job_id}: {message}"),
                        )));
                    }
                    tokio::time::sleep(self.config.transient_retry_delay).await;
                }
            }
        }
    }

    fn finish(
        &self,
        run_id: &str,
        lease: &GoalLease,
        op_key: &str,
        label: &str,
        outcome: ProcessOutcome,
    ) -> Result<OpOutcome, Exit> {
        let mut result = outcome.to_json();
        result["name"] = json!(label);
        let state = if outcome.passed() {
            GoalOpState::Succeeded
        } else {
            GoalOpState::Failed
        };
        self.db
            .goal_op_finish(run_id, lease, op_key, state, &result, now_unix_ms())?;
        Ok(OpOutcome::Done(outcome))
    }

    fn unknown(&self, run_id: &str, lease: &GoalLease, op_key: &str, why: &str) -> Result<Park, Exit> {
        self.db.goal_op_finish(
            run_id,
            lease,
            op_key,
            GoalOpState::Unknown,
            &json!({"outcome_unknown": why}),
            now_unix_ms(),
        )?;
        self.event(run_id, "outcome_unknown", json!({"op": op_key, "why": why}));
        Ok(Park::new(
            GoalRunState::NeedsAttention,
            format!("outcome_unknown: {op_key} ({why}); review the worktree, then resume or cancel in the Chadex app"),
        ))
    }
}

fn spec_label(kind: &str, spec: &ProcessSpec) -> String {
    if kind == "verify" {
        std::iter::once(spec.executable.as_str())
            .chain(spec.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(200)
            .collect()
    } else {
        kind.to_string()
    }
}

fn indent(text: &str) -> String {
    text.lines().map(|line| format!("    {line}")).collect::<Vec<_>>().join("\n")
}

fn tail_bytes(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

fn failure_digest(results: &[(String, ProcessOutcome)]) -> String {
    let mut hasher = Sha256::new();
    for (name, outcome) in results.iter().filter(|(_, outcome)| !outcome.passed()) {
        hasher.update(name.as_bytes());
        hasher.update(format!("\0{:?}\0{}\0", outcome.exit_code, outcome.timed_out).as_bytes());
        // Last lines carry the assertion; ignore volatile timing lines.
        let tail: Vec<&str> = outcome
            .stdout_tail
            .lines()
            .chain(outcome.stderr_tail.lines())
            .filter(|line| !line.contains("finished in") && !line.contains(" ms") && !line.contains("elapsed"))
            .collect();
        for line in tail.iter().rev().take(20) {
            hasher.update(line.trim().as_bytes());
            hasher.update(b"\n");
        }
    }
    format!("{:x}", hasher.finalize())
}

/// `codex exec` reports e.g. "tokens used\n12,345" or "tokens used: 12345".
pub(crate) fn parse_tokens_used(text: &str) -> Option<i64> {
    let lower = text.to_ascii_lowercase();
    let index = lower.rfind("tokens used")?;
    let digits: String = text[index + "tokens used".len()..]
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

fn build_prompt(db: &Database, run: &GoalRunRecord, iteration: i64, feedback: Option<&str>) -> String {
    let plan = db
        .plan_revision(&run.plan_id, run.revision)
        .ok()
        .flatten()
        .map(|revision| revision.content)
        .unwrap_or_default();
    let checks = run
        .spec
        .acceptance
        .iter()
        .map(|check| {
            format!(
                "  - {}: `{}`{}",
                check.name,
                check.argv.join(" "),
                check
                    .cwd
                    .as_deref()
                    .map(|cwd| format!(" (in {cwd})"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut prompt = format!(
        "You are the unattended coding agent for Chadex Goal Mode, working in an isolated Git worktree (your current directory is its root).\n\
         Implement the approved plan below. This is iteration {iteration} of at most {max}.\n\n\
         Rules:\n\
         - Change files only inside this worktree. Do not run `git commit`, `git push`, `git reset`, or change Git configuration; Chadex checkpoints your work after you finish.\n\
         - Network access is disabled. Never try to deploy, publish, send messages, make payments, or contact external services.\n\
         - Do not delete, skip, or weaken tests or acceptance checks to make them pass.\n\
         - Chadex will independently run these acceptance checks after you finish; your own claim of success is not accepted:\n{checks}\n\
         - Finish with a short summary of what you changed and anything you could not do.\n\n\
         Approved plan (revision {revision}, sha256 {sha}):\n<<<PLAN\n{plan}\nPLAN>>>\n",
        max = run.spec.budget.max_iterations,
        revision = run.revision,
        sha = run.content_sha256,
    );
    if let Some(feedback) = feedback {
        prompt.push_str(&format!(
            "\nThe previous iteration did not pass acceptance. Failing checks:\n{feedback}\n"
        ));
        if run.same_failure_streak >= 1 {
            prompt.push_str("\nThe same failure has now repeated. Re-examine your assumptions and try a different approach.\n");
        }
    }
    tail_bytes(&prompt, MAX_PROMPT_BYTES)
}

/// Text of a process stream from a run_process/observe_jobs result.
pub(crate) fn process_stream(output: &Value, stream: &str) -> String {
    for key in [format!("{stream}_tail"), stream.to_string(), format!("{stream}_lines")] {
        match output.get(&key) {
            Some(Value::String(text)) => return text.clone(),
            Some(Value::Array(lines)) => {
                return lines
                    .iter()
                    .filter_map(|line| line.as_str().or_else(|| line.get("text").and_then(Value::as_str)))
                    .collect::<Vec<_>>()
                    .join("\n")
            }
            _ => {}
        }
    }
    String::new()
}

pub(crate) fn process_stdout(output: &Value) -> String {
    process_stream(output, "stdout")
}

fn outcome_from_output(output: &Value) -> ProcessOutcome {
    let failure = output
        .get("failure_kind")
        .or_else(|| output.get("error_kind"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    ProcessOutcome {
        exit_code: output.get("exit_code").and_then(Value::as_i64),
        timed_out: failure.contains("timeout") || failure.contains("timed_out")
            || output.get("timed_out").and_then(Value::as_bool) == Some(true),
        stdout_tail: process_stream(output, "stdout"),
        stderr_tail: process_stream(output, "stderr"),
    }
}

fn is_denial(kind: &str) -> bool {
    [
        "permission", "scope", "forbidden", "denied", "plan_mode", "goal_dispatch", "policy",
        "not_allowed", "unauthorized", "invalid_arguments", "invalid_",
    ]
    .iter()
    .any(|marker| kind.contains(marker))
}

/// Classify a kernel run_process outcome.
pub(crate) fn classify_start(success: bool, output: &Value, error: Option<String>) -> Result<StartResult, ExecError> {
    let terminal = output.get("terminal").and_then(Value::as_bool);
    if let Some(job_id) = output.get("job_id").and_then(Value::as_str) {
        if terminal != Some(true) && output.get("exit_code").and_then(Value::as_i64).is_none() {
            return Ok(StartResult::Started(job_id.to_string()));
        }
    }
    if output.get("exit_code").and_then(Value::as_i64).is_some() {
        return Ok(StartResult::Finished(outcome_from_output(output)));
    }
    let kind = output
        .get("error_kind")
        .or_else(|| output.get("failure_kind"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let message = error.unwrap_or_else(|| kind.clone());
    if success {
        return Err(ExecError::Unknown(format!("unrecognized process result: {message}")));
    }
    if kind.contains("timeout") && output.get("command_started").and_then(Value::as_bool) == Some(true) {
        return Ok(StartResult::Finished(outcome_from_output(output)));
    }
    if is_denial(&kind) {
        return Err(ExecError::Denied(message));
    }
    if output.get("command_started").and_then(Value::as_bool) == Some(false) {
        return Err(ExecError::NotStarted(message));
    }
    Err(ExecError::Unknown(message))
}

/// Classify one observe_jobs item.
pub(crate) fn classify_observation(item: &Value) -> Result<ObserveResult, ExecError> {
    let kind = item
        .get("error_kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind.contains("not_found") || kind.contains("unknown_job") || kind.contains("expired") {
        return Ok(ObserveResult::Lost(kind.to_string()));
    }
    if is_denial(kind) {
        return Err(ExecError::Denied(kind.to_string()));
    }
    let status = item
        .get("status")
        .or_else(|| item.get("execution_state"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let terminal = item.get("terminal").and_then(Value::as_bool) == Some(true)
        || matches!(
            status,
            "completed" | "failed" | "succeeded" | "cancelled" | "timed_out" | "killed" | "exited" | "stopped"
        );
    if matches!(status, "lost" | "unknown" | "interrupted") {
        return Ok(ObserveResult::Lost(status.to_string()));
    }
    if terminal {
        return Ok(ObserveResult::Finished(outcome_from_output(item)));
    }
    if !kind.is_empty() {
        return Err(ExecError::Unknown(kind.to_string()));
    }
    Ok(ObserveResult::Running)
}

/// Production executor: everything goes through the ordinary tool kernel and
/// Runner with the run's own (restricted) principal, so Project/Runner/tool
/// authorization applies exactly as for any other caller.
pub(crate) struct RuntimeGoalExecutor {
    runtime: ToolRuntime,
}

impl RuntimeGoalExecutor {
    pub(crate) fn new(runtime: ToolRuntime) -> Self {
        Self { runtime }
    }

    async fn call(&self, run: &GoalRunRecord, tool: &str, arguments: Value) -> Result<(bool, Value, Option<String>), ExecError> {
        let auth = goal_dispatch_auth(&run.dispatch_auth).map_err(ExecError::Denied)?;
        let outcome = with_goal_run_dispatch(run.goal_run_id.clone(), async {
            self.runtime
                .call_tool_with_context(
                    ToolCallRequest {
                        tool_name: tool.to_string(),
                        arguments,
                    },
                    ToolCallContext {
                        transport: ToolTransport::Api,
                        session_id: None,
                        auth: auth.as_ref(),
                        window: None,
                        record_oauth_scope_denials: false,
                        host_file_import_trust: Default::default(),
                    },
                )
                .await
        })
        .await;
        match (outcome.result, outcome.error_status) {
            (Some(result), _) => Ok((result.success, result.output, result.error)),
            (None, Some(status)) => Err(ExecError::Denied(format!("{status:?}"))),
            (None, None) => Err(ExecError::Unknown("empty tool outcome".to_string())),
        }
    }
}

impl GoalExecutor for RuntimeGoalExecutor {
    fn prepare_workspace<'a>(&'a self, run: &'a GoalRunRecord) -> BoxFut<'a, Result<Workspace, ExecError>> {
        Box::pin(async move {
            let auth = goal_dispatch_auth(&run.dispatch_auth).map_err(ExecError::Denied)?;
            let result = with_goal_run_dispatch(run.goal_run_id.clone(), async {
                self.runtime
                    .prepare_managed_worktree(
                        run.binding.runner_client_id.clone(),
                        run.binding.project_root.clone(),
                        Some(run.binding.base_commit.clone()),
                        run.goal_run_id.clone(),
                        None,
                        auth.as_ref(),
                    )
                    .await
            })
            .await;
            let output = &result.output;
            let project = output
                .get("project")
                .or_else(|| output.get("project_id"))
                .and_then(|value| value.as_str().or_else(|| value.get("id").and_then(Value::as_str)))
                .map(str::to_string);
            let path = output
                .get("path")
                .or_else(|| output.get("root"))
                .or_else(|| output.pointer("/worktree/path"))
                .and_then(Value::as_str)
                .map(str::to_string);
            match (result.success, project, path) {
                (true, Some(project_id), Some(path)) => Ok(Workspace { project_id, path }),
                (true, _, _) => Err(ExecError::Unknown(format!("unrecognized worktree result: {output}"))),
                (false, _, _) => {
                    let kind = output.get("error_kind").and_then(Value::as_str).unwrap_or_default();
                    let message = result.error.clone().unwrap_or_else(|| kind.to_string());
                    if is_denial(kind) {
                        Err(ExecError::Denied(message))
                    } else {
                        Err(ExecError::NotStarted(message))
                    }
                }
            }
        })
    }

    fn start<'a>(&'a self, run: &'a GoalRunRecord, workspace: &'a Workspace, spec: ProcessSpec) -> BoxFut<'a, Result<StartResult, ExecError>> {
        Box::pin(async move {
            let mut arguments = json!({
                "project": workspace.project_id,
                "executable": spec.executable,
                "args": spec.args,
                "timeout_secs": spec.timeout_secs,
                "sync_wait_secs": 5,
            });
            if let Some(stdin) = spec.stdin {
                arguments["stdin"] = json!(stdin);
            }
            if let Some(cwd) = spec.cwd {
                arguments["cwd"] = json!(cwd);
            }
            let (success, output, error) = self.call(run, "run_process", arguments).await?;
            classify_start(success, &output, error)
        })
    }

    fn observe<'a>(&'a self, run: &'a GoalRunRecord, _workspace: &'a Workspace, job_id: &'a str) -> BoxFut<'a, Result<ObserveResult, ExecError>> {
        Box::pin(async move {
            let (_, output, error) = self
                .call(
                    run,
                    "observe_jobs",
                    json!({"items": [{"job_id": job_id}], "wait_secs": 20, "wake_on": "terminal", "tail_lines": 200}),
                )
                .await?;
            match output.get("items").and_then(Value::as_array).and_then(|items| items.first()) {
                Some(item) => classify_observation(item),
                None => Err(ExecError::Unknown(error.unwrap_or_else(|| "empty observation".to_string()))),
            }
        })
    }

    fn stop<'a>(&'a self, run: &'a GoalRunRecord, workspace: &'a Workspace, job_id: &'a str) -> BoxFut<'a, Result<(), ExecError>> {
        Box::pin(async move {
            let (success, _, error) = self
                .call(
                    run,
                    "stop_job",
                    json!({"project": workspace.project_id, "job_id": job_id, "confirm": true}),
                )
                .await?;
            if success {
                Ok(())
            } else {
                Err(ExecError::Unknown(error.unwrap_or_default()))
            }
        })
    }
}

impl ToolRuntime {
    /// Start the in-process Goal supervisor. Requires the durable store.
    pub(crate) fn start_goal_supervisor(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let db = self.communication_db.clone()?;
        let wake = Arc::new(Notify::new());
        self.goal_supervisor_wake = Some(wake.clone());
        let supervisor = GoalSupervisor::new(
            db,
            Arc::new(RuntimeGoalExecutor::new(self.clone())),
            GoalProvider::from_env(),
            GoalSupervisorConfig::default(),
            wake,
        );
        Some(supervisor.spawn())
    }
}
