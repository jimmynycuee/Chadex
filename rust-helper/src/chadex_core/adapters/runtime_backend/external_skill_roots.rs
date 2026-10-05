//! Connect external Skill folders by editing `[skills]` in the local Runner's
//! runner.toml and hot-reloading it. The file edit is compare-and-swapped on
//! the whole-file revision, backed up first, and rolled back when the Runner
//! rejects or cannot apply the candidate.

use super::*;
use crate::chadex_core::external_skills::{
    current_skill_roots, persist_if_unchanged, read_runner_config, render_skill_roots,
    restore_if_unchanged, runner_config_revision, validate_requested_roots, RootRejection,
    SYSTEM_SKILL_ROOT_DENYLIST,
};
use crate::chadex_core::runtime_compat::state::ChadexRunnerConfigTarget;

const RELOAD_ATTEMPTS: usize = 3;

impl RuntimeBackendAdapter {
    pub(crate) async fn external_skill_roots(&self) -> ChadexResult<Value> {
        let target = self.runner_config_target().await?;
        let state = blocking(move || {
            let content = read_runner_config(&target.runner_config)?;
            current_skill_roots(&content)
        })
        .await?
        .map_err(config_error)?;
        Ok(serde_json::to_value(state).unwrap_or(Value::Null))
    }

    pub(crate) async fn set_external_skill_roots(
        &self,
        roots: Vec<PathBuf>,
        script_roots: Vec<PathBuf>,
        expected_revision: String,
        verify_project_path: Option<String>,
    ) -> ChadexResult<Value> {
        let target = self.runner_config_target().await?;
        let home = chadex_runtime_runner_config::paths::home_dir().ok_or_else(|| {
            ChadexError::new(
                "home_directory_unavailable",
                "Chadex could not determine the home directory",
                "Check that HOME is set for Chadex and retry.",
            )
        })?;
        let config_path = target.runner_config.clone();
        let prepared = blocking(move || -> Result<_, ChadexError> {
            let original = read_runner_config(&config_path).map_err(config_error)?;
            let revision = runner_config_revision(&original);
            if revision != expected_revision {
                return Err(revision_conflict(&revision));
            }
            validate_requested_roots(&roots, &script_roots, &home, SYSTEM_SKILL_ROOT_DENYLIST)
                .map_err(root_rejection)?;
            let candidate =
                render_skill_roots(&original, &roots, &script_roots).map_err(config_error)?;
            Ok((original, candidate))
        })
        .await??;
        let (original, candidate) = prepared;
        if candidate == original {
            // Nothing to write, but a previous failed rollback may have left the
            // Runner on a different snapshot than the file; reloading the file
            // as-is resynchronises it.
            let token = read_probe_token(&target.user_token_file)
                .await
                .ok_or_else(runtime_unreachable)?;
            let generation = reload_runner_config(&self.probe_client, &target, &token).await?;
            let mut state = self.external_skill_roots().await?;
            state["generation"] = json!(generation);
            return Ok(state);
        }

        let token = read_probe_token(&target.user_token_file)
            .await
            .ok_or_else(runtime_unreachable)?;
        let verify = || async {
            if let Some(path) = verify_project_path.as_deref() {
                self.skill_catalog(path).await.map_err(|error| {
                    ChadexError::new(
                        "external_skill_roots_unverified",
                        "The Runner reloaded, but the Skill catalog could not be read back",
                        "The previous Skill folders were restored. Retry, or check the folder contents.",
                    )
                    .with_details(json!({ "catalog_error": error.code }))
                })?;
            }
            Ok(())
        };
        let generation = apply_runner_config_candidate(
            &self.probe_client,
            &target,
            &token,
            original,
            candidate,
            verify,
        )
        .await?;
        let mut state = self.external_skill_roots().await?;
        state["generation"] = json!(generation);
        Ok(state)
    }

    async fn runner_config_target(&self) -> ChadexResult<ChadexRunnerConfigTarget> {
        self.app
            .chadex_runner_config_target()
            .await
            .map_err(map_desktop_error)?
            .ok_or_else(|| {
                ChadexError::new(
                    "runtime_not_ready",
                    "External Skill folders need the local runtime to be set up and idle",
                    "Finish connecting the local runtime, wait for the current operation, then retry.",
                )
            })
    }
}

/// Persist `candidate` (compare-and-swapped against `original`, with a backup),
/// hot-reload it, run `verify`, and restore `original` if any step fails.
pub(super) async fn apply_runner_config_candidate<F, Fut>(
    client: &Client,
    target: &ChadexRunnerConfigTarget,
    token: &str,
    original: String,
    candidate: String,
    verify: F,
) -> ChadexResult<u64>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ChadexResult<()>>,
{
    let config_path = target.runner_config.clone();
    let (expected, written) = (original.clone(), candidate.clone());
    blocking(move || persist_if_unchanged(&config_path, &expected, &written))
        .await?
        .map_err(|code| {
            if code == "runner_config_concurrent_change" {
                revision_conflict("")
            } else {
                config_error(code)
            }
        })?;

    let applied = async {
        let generation = reload_runner_config(client, target, token).await?;
        verify().await?;
        Ok::<u64, ChadexError>(generation)
    }
    .await;
    let error = match applied {
        Ok(generation) => {
            discard_backup(&target.runner_config).await;
            return Ok(generation);
        }
        Err(error) => error,
    };
    let config_path = target.runner_config.clone();
    let restored = blocking(move || restore_if_unchanged(&config_path, &candidate, &original))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false);
    // The candidate may already be live (reload succeeded, or its response was
    // lost) unless the Runner definitively refused it. When it may be live the
    // restored file must be reloaded too; only report a rollback when both the
    // file and the Runner are known to be back on the previous settings.
    let resync_required = !definitely_not_applied(&error);
    let resynced = restored
        && (reload_runner_config(client, target, token).await.is_ok() || !resync_required);
    if !resynced {
        return Err(ChadexError::new(
            "external_skill_roots_state_unknown",
            if restored {
                "The previous Skill folders were written back, but the Runner could not reload them"
            } else {
                "runner.toml changed while Chadex was undoing a failed update, so it was left as is"
            },
            "The Runner may still use the new Skill folders. Reload the Skill folder settings and apply them again.",
        )
        .with_details(json!({
            "cause": error.code,
            "file_restored": restored,
            "runner_resynced": false,
        })));
    }
    discard_backup(&target.runner_config).await;
    let mut details = error.details.clone().unwrap_or_else(|| json!({}));
    details["rolled_back"] = json!(true);
    Err(error.with_details(details))
}

const NOT_APPLIED: &str = "runner_applied";

/// Errors raised before the Runner swapped in the candidate snapshot.
fn not_applied(error: ChadexError) -> ChadexError {
    let mut details = error.details.clone().unwrap_or_else(|| json!({}));
    details[NOT_APPLIED] = json!(false);
    error.with_details(details)
}

fn definitely_not_applied(error: &ChadexError) -> bool {
    error
        .details
        .as_ref()
        .and_then(|details| details.get(NOT_APPLIED))
        .and_then(Value::as_bool)
        == Some(false)
}

/// The backup holds a full runner.toml (including its token); keep it only
/// while an update is in flight or its outcome is unknown.
async fn discard_backup(config: &Path) {
    let backup = crate::chadex_core::external_skills::runner_config_backup_path(config);
    let _ = tokio::fs::remove_file(backup).await;
}

/// `check` validates the on-disk candidate and reports the active
/// generation; `reload` then swaps it in only if that generation is still
/// current. A generation race is retried from a fresh check.
pub(super) async fn reload_runner_config(
    client: &Client,
    target: &ChadexRunnerConfigTarget,
    token: &str,
) -> ChadexResult<u64> {
    let cancellation =
        CancellationContext::new(CancellationSignal::new(), CancellationSignal::new());
    for _ in 0..RELOAD_ATTEMPTS {
        let checked = operator_call(client, target, token, "/api/runners/config/check", &cancellation)
            .await?;
        if !checked.success || checked.output.get("valid").and_then(Value::as_bool) != Some(true)
        {
            return Err(not_applied(ChadexError::new(
                "runner_config_rejected",
                "The Runner rejected the new Skill folder settings",
                "The previous settings were restored. Check the folders and retry.",
            )
            .with_details(json!({ "runner_error": operator_error_code(&checked) }))));
        }
        if restart_required(&checked) {
            return Err(not_applied(restart_required_error()));
        }
        let generation = checked
            .output
            .get("current_generation")
            .and_then(Value::as_u64)
            .ok_or_else(runtime_unreachable)?;
        let reloaded = operator_call_with(
            client,
            target,
            token,
            "/api/runners/config/reload",
            json!({
                "client_id": target.runner_client_id,
                "expected_generation": generation,
            }),
            &cancellation,
        )
        .await?;
        if reloaded.success {
            if restart_required(&reloaded) {
                return Err(restart_required_error());
            }
            return Ok(reloaded
                .output
                .get("current_generation")
                .and_then(Value::as_u64)
                .unwrap_or(generation.saturating_add(1)));
        }
        if operator_error_code(&reloaded) != Some("config_generation_conflict") {
            return Err(not_applied(ChadexError::new(
                "runner_config_reload_failed",
                "The Runner could not apply the new Skill folder settings",
                "The previous settings were restored. Retry in a moment.",
            )
            .with_details(json!({ "runner_error": operator_error_code(&reloaded) }))));
        }
    }
    Err(not_applied(ChadexError::new(
        "runner_config_reload_failed",
        "The Runner configuration kept changing while Chadex applied Skill folders",
        "The previous settings were restored. Retry in a moment.",
    )))
}

async fn operator_call(
    client: &Client,
    target: &ChadexRunnerConfigTarget,
    token: &str,
    endpoint: &str,
    cancellation: &CancellationContext,
) -> ChadexResult<OperatorToolResult> {
    operator_call_with(
        client,
        target,
        token,
        endpoint,
        json!({ "client_id": target.runner_client_id }),
        cancellation,
    )
    .await
}

async fn operator_call_with(
    client: &Client,
    target: &ChadexRunnerConfigTarget,
    token: &str,
    endpoint: &str,
    body: Value,
    cancellation: &CancellationContext,
) -> ChadexResult<OperatorToolResult> {
    let value = post_local_operator_json(
        client,
        &target.server_url,
        endpoint,
        token,
        body,
        cancellation,
    )
    .await
    .map_err(|_| runtime_unreachable())?
    .ok_or_else(|| {
        ChadexError::new(
            "runner_config_reload_unsupported",
            "This runtime cannot reload its configuration in place",
            "Update Chadex, then retry.",
        )
    })?;
    serde_json::from_value(value).map_err(|_| runtime_unreachable())
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> ChadexResult<T> {
    tokio::task::spawn_blocking(work).await.map_err(|_| {
        ChadexError::new(
            "runner_config_unavailable",
            "Chadex could not read the local Runner configuration",
            "Retry. If this keeps happening, restart Chadex.",
        )
    })
}

fn restart_required(result: &OperatorToolResult) -> bool {
    result
        .output
        .get("restart_required")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn restart_required_error() -> ChadexError {
    ChadexError::new(
        "runner_config_restart_required",
        "runner.toml has other pending changes that need a runtime restart",
        "The previous Skill folders were restored. Restart the local runtime, then retry.",
    )
}

fn runtime_unreachable() -> ChadexError {
    ChadexError::new(
        "runtime_unreachable",
        "Chadex could not reach the local runtime to apply Skill folders",
        "Make sure the local runtime is running, then retry.",
    )
}

fn revision_conflict(current: &str) -> ChadexError {
    let error = ChadexError::new(
        "external_skill_roots_conflict",
        "runner.toml changed since the Skill folders were last loaded",
        "Reload the Skill folder settings and apply your change again.",
    );
    if current.is_empty() {
        error
    } else {
        error.with_details(json!({ "current_revision": current }))
    }
}

fn config_error(code: &'static str) -> ChadexError {
    ChadexError::new(
        code,
        "Chadex could not read or update the local Runner configuration",
        "Check that runner.toml exists and is a regular file, then retry.",
    )
}

fn root_rejection(rejection: RootRejection) -> ChadexError {
    let error = ChadexError::new(
        rejection.code,
        "This folder cannot be connected as a Skill source",
        "Choose an existing Skill folder that is not a link, a system folder, your home folder or a credential folder.",
    );
    match rejection.path {
        Some(path) => error.with_details(json!({ "path": path })),
        None => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::routing::post;
    use axum::{Json, Router};
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct FakeRunner {
        check_invalid: bool,
        reload_conflicts: usize,
        reload_fails: bool,
        /// Fail every reload after this many have succeeded.
        fail_reloads_after: Option<usize>,
        successful_reloads: usize,
        generation: u64,
        calls: Vec<String>,
        reloaded_contents: Vec<String>,
        config_path: PathBuf,
    }

    type Shared = Arc<StdMutex<FakeRunner>>;

    async fn check(State(state): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
        let mut runner = state.lock().unwrap();
        assert_eq!(body["client_id"], "client-1");
        runner.calls.push("check".into());
        Json(json!({
            "success": true,
            "output": {
                "valid": !runner.check_invalid,
                "restart_required": false,
                "current_generation": runner.generation,
            }
        }))
    }

    async fn reload(State(state): State<Shared>, Json(body): Json<Value>) -> Json<Value> {
        let mut runner = state.lock().unwrap();
        runner.calls.push("reload".into());
        if runner.reload_conflicts > 0 {
            runner.reload_conflicts -= 1;
            runner.generation += 1;
            return Json(json!({"success": false, "output": {"error_code": "config_generation_conflict"}}));
        }
        if runner.reload_fails
            || runner
                .fail_reloads_after
                .is_some_and(|limit| runner.successful_reloads >= limit)
        {
            return Json(json!({"success": false, "output": {"error_code": "config_validation_failed"}}));
        }
        assert_eq!(body["expected_generation"], runner.generation);
        let content = std::fs::read_to_string(&runner.config_path).unwrap();
        runner.reloaded_contents.push(content);
        runner.successful_reloads += 1;
        runner.generation += 1;
        Json(json!({"success": true, "output": {"current_generation": runner.generation, "restart_required": false}}))
    }

    struct Harness {
        _dir: tempfile::TempDir,
        state: Shared,
        target: ChadexRunnerConfigTarget,
        client: Client,
    }

    const ORIGINAL: &str = "# keep me\nclient_id = \"client-1\"\n\n[policy]\nallowed_roots = [\"/p\"]\n";

    async fn harness(configure: impl FnOnce(&mut FakeRunner)) -> Harness {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("runner.toml");
        std::fs::write(&config_path, ORIGINAL).unwrap();
        let mut runner = FakeRunner {
            generation: 7,
            config_path: config_path.clone(),
            ..FakeRunner::default()
        };
        configure(&mut runner);
        let state = Arc::new(StdMutex::new(runner));
        let app = Router::new()
            .route("/api/runners/config/check", post(check))
            .route("/api/runners/config/reload", post(reload))
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Harness {
            _dir: dir,
            state,
            target: ChadexRunnerConfigTarget {
                server_url: format!("http://127.0.0.1:{port}"),
                user_token_file: PathBuf::new(),
                runner_config: config_path,
                runner_client_id: "client-1".into(),
            },
            client: Client::builder().no_proxy().build().unwrap(),
        }
    }

    fn candidate() -> String {
        render_skill_roots(ORIGINAL, &[PathBuf::from("/skills")], &[]).unwrap()
    }

    async fn apply(h: &Harness, original: &str, verify_ok: bool) -> ChadexResult<u64> {
        apply_runner_config_candidate(
            &h.client,
            &h.target,
            "token",
            original.to_string(),
            candidate(),
            || async move {
                if verify_ok {
                    Ok(())
                } else {
                    Err(ChadexError::new("external_skill_roots_unverified", "x", "y"))
                }
            },
        )
        .await
    }

    fn on_disk(h: &Harness) -> String {
        std::fs::read_to_string(&h.target.runner_config).unwrap()
    }

    fn rolled_back(error: &ChadexError) -> bool {
        error.details.as_ref().unwrap()["rolled_back"] == true
    }

    #[tokio::test]
    async fn applies_candidate_and_reloads_with_generation_fence() {
        let h = harness(|_| {}).await;
        let generation = apply(&h, ORIGINAL, true).await.unwrap();
        assert_eq!(generation, 8);
        assert_eq!(on_disk(&h), candidate());
        assert!(on_disk(&h).starts_with(ORIGINAL));
        let runner = h.state.lock().unwrap();
        assert_eq!(runner.calls, vec!["check", "reload"]);
        assert_eq!(runner.reloaded_contents, vec![candidate()]);
        let backup = crate::chadex_core::external_skills::runner_config_backup_path(
            &h.target.runner_config,
        );
        assert!(!backup.exists(), "backup with the token must not outlive the update");
    }

    #[tokio::test]
    async fn failed_resync_after_restore_reports_unknown_state() {
        // First reload applies the candidate; verification fails; the reload of
        // the restored file fails, so the Runner may still run the candidate.
        let h = harness(|runner| runner.fail_reloads_after = Some(1)).await;
        let error = apply(&h, ORIGINAL, false).await.unwrap_err();
        assert_eq!(error.code, "external_skill_roots_state_unknown");
        let details = error.details.unwrap();
        assert_eq!(details["cause"], "external_skill_roots_unverified");
        assert_eq!(details["file_restored"], true);
        assert_eq!(details["runner_resynced"], false);
        assert_eq!(on_disk(&h), ORIGINAL);
        let backup = crate::chadex_core::external_skills::runner_config_backup_path(
            &h.target.runner_config,
        );
        assert!(backup.exists(), "keep the backup while the outcome is unknown");
    }

    #[tokio::test]
    async fn concurrent_edit_during_rollback_is_kept_and_reported() {
        let h = harness(|_| {}).await;
        let path = h.target.runner_config.clone();
        let error = apply_runner_config_candidate(
            &h.client,
            &h.target,
            "token",
            ORIGINAL.to_string(),
            candidate(),
            || async move {
                std::fs::write(&path, "operator = \"edit\"\n").unwrap();
                Err(ChadexError::new("external_skill_roots_unverified", "x", "y"))
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "external_skill_roots_state_unknown");
        assert_eq!(error.details.unwrap()["file_restored"], false);
        assert_eq!(on_disk(&h), "operator = \"edit\"\n");
    }

    #[tokio::test]
    async fn generation_conflict_is_retried_from_a_fresh_check() {
        let h = harness(|runner| runner.reload_conflicts = 1).await;
        apply(&h, ORIGINAL, true).await.unwrap();
        assert_eq!(
            h.state.lock().unwrap().calls,
            vec!["check", "reload", "check", "reload"]
        );
    }

    #[tokio::test]
    async fn rejected_candidate_is_rolled_back() {
        let h = harness(|runner| runner.check_invalid = true).await;
        let error = apply(&h, ORIGINAL, true).await.unwrap_err();
        assert_eq!(error.code, "runner_config_rejected");
        assert!(rolled_back(&error));
        assert_eq!(on_disk(&h), ORIGINAL);
    }

    #[tokio::test]
    async fn failed_reload_is_rolled_back() {
        let h = harness(|runner| runner.reload_fails = true).await;
        let error = apply(&h, ORIGINAL, true).await.unwrap_err();
        assert_eq!(error.code, "runner_config_reload_failed");
        assert!(rolled_back(&error));
        assert_eq!(on_disk(&h), ORIGINAL);
    }

    #[tokio::test]
    async fn failed_verification_restores_and_reloads_original() {
        let h = harness(|_| {}).await;
        let error = apply(&h, ORIGINAL, false).await.unwrap_err();
        assert_eq!(error.code, "external_skill_roots_unverified");
        assert!(rolled_back(&error));
        assert_eq!(on_disk(&h), ORIGINAL);
        let runner = h.state.lock().unwrap();
        assert_eq!(runner.calls, vec!["check", "reload", "check", "reload"]);
        assert_eq!(runner.reloaded_contents, vec![candidate(), ORIGINAL.to_string()]);
    }

    #[tokio::test]
    async fn concurrent_edit_is_refused_before_any_write_or_reload() {
        let h = harness(|_| {}).await;
        let error = apply(&h, "stale content\n", true).await.unwrap_err();
        assert_eq!(error.code, "external_skill_roots_conflict");
        assert_eq!(on_disk(&h), ORIGINAL);
        assert!(h.state.lock().unwrap().calls.is_empty());
    }
}
