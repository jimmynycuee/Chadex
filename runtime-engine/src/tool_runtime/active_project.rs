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
pub(crate) const SOURCE_ACTIVE_PLAN: &str = "active_plan";

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

    /// One sentence for the model's `note`/`next`: which project was used
    /// and that the user must be told.
    pub(crate) fn tell_user(&self, name: Option<&str>, path: Option<&str>) -> String {
        let why = match self.source {
            SOURCE_ACTIVE_PLAN => "the project of your active plan",
            SOURCE_CHADEX_APP => "the project open in the Chadex app",
            _ => "the only project available to this connection",
        };
        let label = match (name, path) {
            (Some(name), Some(path)) => format!("{name} ({path})"),
            (None, Some(path)) => format!("{} ({path})", self.project),
            (Some(name), None) => format!("{name} ({})", self.project),
            (None, None) => self.project.clone(),
        };
        format!(
            "No project was named, so Chadex used {why}: {label}. Tell the user which project you are working in."
        )
    }
}

/// Whether a principal may take its default from the Chadex app: the app's
/// local operator (unauthenticated in-process runtimes, the local bootstrap
/// credential the ChatGPT tunnel uses) or the user who owns the Runner. Other
/// principals that merely can see that Runner (another user with global
/// visibility, a shared key) never inherit the app owner's selection.
fn may_use_app_default(auth: Option<&AuthContext>, runner_owner: Option<&str>) -> bool {
    let Some(auth) = auth else {
        return true;
    };
    if auth.is_bootstrap {
        return true;
    }
    let owner = runner_owner.map(str::trim).filter(|owner| !owner.is_empty());
    let user = auth
        .username
        .as_deref()
        .map(str::trim)
        .filter(|user| !user.is_empty());
    owner.is_some() && owner == user
}

impl ToolRuntime {
    /// The app's selected project id as written by the operator side. Not yet
    /// checked against any caller.
    pub(crate) fn app_active_project_id(&self) -> Option<String> {
        self.app_active_project_file
            .as_deref()
            .and_then(read_app_active_project)
    }

    /// The app's project resolved for `auth`, or `None` when no app project is
    /// set, this principal cannot use it, or it may not inherit the app's
    /// selection. `Some(Err(()))` means a project is set but unavailable here.
    /// `list_projects` marks exactly what this resolves to.
    pub(crate) async fn app_active_project_for_auth(
        &self,
        auth: Option<&AuthContext>,
    ) -> Option<Result<String, ()>> {
        let id = self.app_active_project_id()?;
        let Ok(resolved) = self.resolve_project_input_for_auth(&id, auth).await else {
            return Some(Err(()));
        };
        let access = crate::runner_http::runner_access_from_auth(auth);
        let owner = self
            .runner_registry
            .get_runner_semantic_view_for_auth(&resolved.config.client_id, access.as_ref())
            .await
            .and_then(|client| client.view.owner.clone());
        if !may_use_app_default(auth, owner.as_deref()) {
            return Some(Err(()));
        }
        Some(Ok(resolved.resolved_id))
    }

    /// Resolve an omitted `project`: the project open in the Chadex app when
    /// this principal can use it; otherwise, only when no app project is set,
    /// the single non-worktree project this principal can see. Anything else
    /// is an error listing the candidates; it never guesses.
    pub(crate) async fn default_project_for_auth(
        &self,
        auth: Option<&AuthContext>,
    ) -> Result<DefaultProject, ToolResult> {
        let app_project = self.app_active_project_for_auth(auth).await;
        if let Some(Ok(project)) = &app_project {
            return Ok(DefaultProject {
                project: project.clone(),
                source: SOURCE_CHADEX_APP,
            });
        }
        let app_project_set = app_project.is_some();
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
        if !app_project_set && primary.len() == 1 {
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
        // Never echo the app's project id here: this principal may not use it.
        let message = if app_project_set {
            "project is required: the project open in the Chadex app is not available to this connection. Ask the user which project to use, then pass project explicitly".to_string()
        } else if primary.is_empty() {
            "project is required and no project is available to this connection. Ask the user to open a project in the Chadex app".to_string()
        } else {
            "project is required: no project is open in the Chadex app and several are available. Ask the user which project to use (or to open it in the Chadex app), then pass project explicitly".to_string()
        };
        Err(ToolResult::err_with_output(
            message,
            json!({
                "error_kind": "project_required",
                "state_changed": false,
                "app_project_set": app_project_set,
                "candidates": candidates,
                "candidate_count": primary.len(),
            }),
        )
        .with_recovery(RecoveryKind::FixInput))
    }
}
