//! The project open in the Chadex app, used as the default for `plan_start`
//! and direct `goal_run_start` when the model omits `project`.
//!
//! The Runner project registry is cumulative: every path ever activated or
//! auto-registered (nested repositories, benchmarks, Goal worktrees) stays
//! registered, so `list_projects` alone cannot say which project the user is
//! working in. The Chadex app can: its helper commits the selected project to
//! `desktop-state.json` (`project.runtime_project_id`) and hands that file's
//! path to the Server it spawns through `CHADEX_DESKTOP_STATE_FILE`.
//!
//! Authority: the default comes only from that operator-owned file, never from
//! model input, and it grants nothing. The id is resolved for the calling
//! principal exactly like an explicit `project`, so a default the caller could
//! not name itself fails instead of widening access.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::projects::ListProjectsOptions;
use super::tool_result::{RecoveryKind, ToolResult};
use super::ToolRuntime;
use crate::auth::AuthContext;

pub(crate) const APP_ACTIVE_PROJECT_FILE_ENV: &str = "CHADEX_DESKTOP_STATE_FILE";
const APP_STATE_MAX_BYTES: u64 = 1024 * 1024;
const MAX_LISTED_CANDIDATES: usize = 20;

/// Where the default project came from, reported back to the model.
pub(crate) const SOURCE_CHADEX_APP: &str = "chadex_app";
pub(crate) const SOURCE_ONLY_PROJECT: &str = "only_project";

pub(crate) fn app_active_project_file_from_env() -> Option<PathBuf> {
    std::env::var_os(APP_ACTIVE_PROJECT_FILE_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// `project.runtime_project_id` from the app state file, or `None` when the
/// file is missing, oversized, unreadable or has no selected project.
pub(crate) fn read_app_active_project(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > APP_STATE_MAX_BYTES {
        return None;
    }
    let state: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    state
        .pointer("/project/runtime_project_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.chars().count() <= 512)
        .map(str::to_string)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DefaultProject {
    pub(crate) project: String,
    pub(crate) source: &'static str,
}

impl DefaultProject {
    pub(crate) fn projection(&self) -> Value {
        json!({"project": self.project, "source": self.source})
    }
}

impl ToolRuntime {
    /// The app's selected project id as written by the operator side. Not yet
    /// checked against any caller.
    pub(crate) fn app_active_project_id(&self) -> Option<String> {
        self.app_active_project_file
            .as_deref()
            .and_then(read_app_active_project)
    }

    /// Resolve an omitted `project`: the project open in the Chadex app when
    /// this principal can use it; otherwise, only when no app project is set,
    /// the single non-worktree project this principal can see. Anything else
    /// is an error listing the candidates; it never guesses.
    pub(crate) async fn default_project_for_auth(
        &self,
        auth: Option<&AuthContext>,
    ) -> Result<DefaultProject, ToolResult> {
        let app_project = self.app_active_project_id();
        if let Some(id) = app_project.as_deref() {
            if let Ok(resolved) = self.resolve_project_input_for_auth(id, auth).await {
                return Ok(DefaultProject {
                    project: resolved.resolved_id,
                    source: SOURCE_CHADEX_APP,
                });
            }
        }
        let listing = self
            .list_projects_with_options(
                auth,
                ListProjectsOptions {
                    summary_only: true,
                    ..ListProjectsOptions::default()
                },
            )
            .await;
        let projects = listing
            .output
            .get("projects")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let primary: Vec<&Value> = projects
            .iter()
            .filter(|project| project["enabled"].as_bool().unwrap_or(false))
            .filter(|project| !project["managed_worktree"].as_bool().unwrap_or(false))
            .collect();
        if app_project.is_none() && primary.len() == 1 {
            if let Some(id) = primary[0]["id"].as_str() {
                return Ok(DefaultProject {
                    project: id.to_string(),
                    source: SOURCE_ONLY_PROJECT,
                });
            }
        }
        let candidates: Vec<Value> = primary
            .iter()
            .take(MAX_LISTED_CANDIDATES)
            .map(|project| {
                json!({
                    "id": project["id"],
                    "name": project["name"],
                })
            })
            .collect();
        let message = match app_project.as_deref() {
            Some(id) => format!(
                "project is required: the project open in the Chadex app ({id}) is not available to this connection. Ask the user which project to use, or to open it in the Chadex app, then pass project explicitly"
            ),
            None if primary.is_empty() => {
                "project is required and no project is available to this connection. Ask the user to open a project in the Chadex app".to_string()
            }
            None => "project is required: no project is open in the Chadex app and several are available. Ask the user which project to use (or to open it in the Chadex app), then pass project explicitly".to_string(),
        };
        Err(ToolResult::err_with_output(
            message,
            json!({
                "error_kind": "project_required",
                "state_changed": false,
                "app_active_project": app_project,
                "candidates": candidates,
                "candidate_count": primary.len(),
            }),
        )
        .with_recovery(RecoveryKind::FixInput))
    }
}
