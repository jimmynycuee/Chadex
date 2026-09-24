use super::{RecoveryKind, ToolResult, ToolRuntime};
use crate::auth::AuthContext;
use crate::projects::ProjectConfig;
use crate::runner_protocol::{RunnerProjectLineage, RunnerProjectSummary, RunnerView};
use serde_json::{json, Value};
use std::collections::HashMap;

const PROJECT_RESOLUTION_CACHE_MAX_ENTRIES: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectResolutionCacheKey {
    project: String,
    access: String,
}

#[derive(Debug, Default)]
pub(crate) struct ProjectResolutionCache {
    epoch: u64,
    entries: HashMap<ProjectResolutionCacheKey, ResolvedProject>,
}

impl ProjectResolutionCache {
    fn get(&mut self, epoch: u64, key: &ProjectResolutionCacheKey) -> Option<ResolvedProject> {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.entries.clear();
        }
        self.entries.get(key).cloned()
    }

    fn insert(&mut self, epoch: u64, key: ProjectResolutionCacheKey, resolved: ResolvedProject) {
        if self.epoch != epoch {
            self.epoch = epoch;
            self.entries.clear();
        } else if self.entries.len() >= PROJECT_RESOLUTION_CACHE_MAX_ENTRIES {
            self.entries.clear();
        }
        self.entries.insert(key, resolved);
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectResolverCandidate {
    pub(crate) id: String,
    pub(crate) client_id: String,
    pub(crate) agent_project_id: String,
    pub(crate) name: Option<String>,
    pub(crate) path: String,
    pub(crate) allow_patch: bool,
    pub(crate) root_fingerprint: Option<String>,
    pub(crate) lineage: Option<RunnerProjectLineage>,
    pub(crate) connected: bool,
    pub(crate) status: String,
    pub(crate) last_seen: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectKnowledgeUnavailableReason {
    RunnerUnavailable,
    InventoryIncomplete,
    SourceUnavailable,
    SourceIdentityUnavailable,
    Unauthorized,
}

impl ProjectKnowledgeUnavailableReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::RunnerUnavailable => "runner_unavailable",
            Self::InventoryIncomplete => "inventory_incomplete",
            Self::SourceUnavailable => "source_unavailable",
            Self::SourceIdentityUnavailable => "source_identity_unavailable",
            Self::Unauthorized => "unauthorized",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AuthorizedProjectKnowledgeSource {
    pub(crate) source: ResolvedProject,
    pub(crate) base_sha: String,
}

#[derive(Debug, Clone)]
pub(crate) enum ProjectKnowledgeSourceResolution {
    NotAssociated,
    Available(AuthorizedProjectKnowledgeSource),
    Unavailable(ProjectKnowledgeUnavailableReason),
    Stale,
}

#[derive(Debug, Clone)]
pub(crate) struct ResolvedProject {
    pub(crate) input: String,
    pub(crate) resolved_id: String,
    pub(crate) config: ProjectConfig,
    /// Root identity from the same Runner project-inventory snapshot that
    /// produced this resolution. It is descriptive only and never authorizes
    /// access by itself.
    pub(crate) root_fingerprint: Option<String>,
    /// Descriptive Runner-owned lineage only. It never changes execution cwd,
    /// ProjectConfig, Session authority, Plugin placement, or tool permission.
    pub(crate) knowledge_association: Option<RunnerProjectLineage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectResolverErrorKind {
    UnknownProject,
    AmbiguousProject,
}

impl ProjectResolverErrorKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::UnknownProject => "unknown_project",
            Self::AmbiguousProject => "ambiguous_project",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ProjectResolverError {
    pub(crate) kind: ProjectResolverErrorKind,
    pub(crate) project: String,
    pub(crate) candidates: Vec<ProjectResolverCandidate>,
}

impl ProjectResolverError {
    fn candidate_payload(candidate: &ProjectResolverCandidate) -> Value {
        json!({
            "id": candidate.id,
            "client_id": candidate.client_id,
            "agent_project_id": candidate.agent_project_id,
            "name": candidate.name,
            "path": candidate.path,
            "connected": candidate.connected,
            "status": candidate.status,
            "last_seen": candidate.last_seen,
        })
    }

    fn to_output(&self) -> Value {
        let candidates: Vec<Value> = self
            .candidates
            .iter()
            .map(Self::candidate_payload)
            .collect();
        json!({
            "error_kind": self.kind.as_str(),
            "project": self.project,
            "hint": "Use a full runtime project id in the form agent:<client_id>:<project_id> from list_projects.",
            "candidates": candidates,
        })
    }

    pub(crate) fn to_message(&self) -> String {
        let mut message = format!(
            "{} '{}'. Use a full runtime project id in the form agent:<client_id>:<project_id> from list_projects.",
            match self.kind {
                ProjectResolverErrorKind::UnknownProject => "unknown_project",
                ProjectResolverErrorKind::AmbiguousProject => "ambiguous_project",
            },
            self.project
        );
        if self.candidates.is_empty() {
            return message;
        }
        let candidate_summary = self
            .candidates
            .iter()
            .map(|candidate| {
                format!(
                    "{} [{}] {} ({})",
                    candidate.id, candidate.client_id, candidate.path, candidate.status
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        message.push_str(" Candidates: ");
        message.push_str(&candidate_summary);
        message
    }

    pub(crate) fn into_tool_result(self) -> ToolResult {
        ToolResult::err_with_output(self.to_message(), self.to_output())
            .with_recovery(RecoveryKind::FixInput)
    }
}

impl std::fmt::Display for ProjectResolverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_message())
    }
}

impl From<ProjectResolverError> for String {
    fn from(value: ProjectResolverError) -> Self {
        value.to_message()
    }
}

pub(crate) fn runner_project_runtime_id(client_id: &str, project_id: &str) -> String {
    format!("agent:{}:{}", client_id, project_id)
}

impl ToolRuntime {
    fn project_resolution_access_key(auth: Option<&AuthContext>) -> String {
        let Some(access) = crate::runner_http::runner_access_from_auth(auth) else {
            return "internal".to_string();
        };
        format!(
            "global={};bypass={};user={:?};group={:?}",
            access.global_visibility, access.owner_bypass, access.username, access.group
        )
    }

    fn exact_project_cache_key(
        project: &str,
        auth: Option<&AuthContext>,
    ) -> ProjectResolutionCacheKey {
        ProjectResolutionCacheKey {
            project: project.to_string(),
            access: Self::project_resolution_access_key(auth),
        }
    }

    fn cached_exact_project(
        &self,
        project: &str,
        auth: Option<&AuthContext>,
        epoch: u64,
    ) -> Option<ResolvedProject> {
        let key = Self::exact_project_cache_key(project, auth);
        self.project_resolution_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(epoch, &key)
    }

    fn cache_exact_project(
        &self,
        project: &str,
        auth: Option<&AuthContext>,
        epoch: u64,
        resolved: ResolvedProject,
    ) {
        let key = Self::exact_project_cache_key(project, auth);
        self.project_resolution_cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(epoch, key, resolved);
    }

    fn project_candidate_from_view(
        client: &RunnerView,
        project: &RunnerProjectSummary,
    ) -> ProjectResolverCandidate {
        ProjectResolverCandidate {
            id: runner_project_runtime_id(&client.client_id, &project.id),
            client_id: client.client_id.clone(),
            agent_project_id: project.id.clone(),
            name: project.name.clone(),
            path: project.path.clone(),
            allow_patch: project.allow_patch,
            root_fingerprint: project.root_fingerprint.clone(),
            lineage: project.lineage.clone(),
            connected: client.connected,
            status: client.status.clone(),
            last_seen: client.last_seen,
        }
    }

    fn project_config_from_candidate(candidate: &ProjectResolverCandidate) -> ProjectConfig {
        ProjectConfig {
            path: candidate.path.clone(),
            client_id: candidate.client_id.clone(),
            allow_patch: candidate.allow_patch,
        }
    }

    fn resolved_from_candidate(
        input: &str,
        candidate: &ProjectResolverCandidate,
    ) -> ResolvedProject {
        ResolvedProject {
            input: input.to_string(),
            resolved_id: candidate.id.clone(),
            config: Self::project_config_from_candidate(candidate),
            root_fingerprint: candidate.root_fingerprint.clone(),
            knowledge_association: candidate.lineage.clone(),
        }
    }

    fn sort_resolver_candidates(candidates: &mut [ProjectResolverCandidate]) {
        candidates.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then_with(|| a.status.cmp(&b.status))
                .then_with(|| b.last_seen.cmp(&a.last_seen))
                .then_with(|| a.id.cmp(&b.id))
        });
    }

    async fn agent_project_candidates_for_auth(
        &self,
        auth: Option<&AuthContext>,
    ) -> Vec<ProjectResolverCandidate> {
        let mut candidates = Vec::new();
        let access = crate::runner_http::runner_access_from_auth(auth);
        for client in self
            .runner_registry
            .list_runners_for_auth(access.as_ref())
            .await
        {
            for project in client.projects.iter().filter(|project| !project.disabled) {
                candidates.push(Self::project_candidate_from_view(&client, project));
            }
        }
        Self::sort_resolver_candidates(&mut candidates);
        candidates
    }

    /// Return the broadest caller-visible registered ancestor for instruction
    /// inheritance. Only projects already visible through the caller's ordinary
    /// Runner/Project authority participate, so instruction lookup never expands
    /// filesystem authority beyond registered roots.
    pub(crate) async fn instruction_hierarchy_root_for_auth(
        &self,
        target: &ProjectConfig,
        auth: Option<&AuthContext>,
    ) -> ProjectConfig {
        self.agent_project_candidates_for_auth(auth)
            .await
            .into_iter()
            .filter(|candidate| candidate.client_id == target.client_id)
            .filter_map(|candidate| {
                let config = Self::project_config_from_candidate(&candidate);
                let relative =
                    super::helpers::project_relative_runner_cwd(&config, &target.path).ok()?;
                let depth = if relative == "." {
                    0
                } else {
                    relative
                        .split('/')
                        .filter(|component| !component.is_empty())
                        .count()
                };
                Some((config, depth))
            })
            .max_by_key(|(_, depth)| *depth)
            .map(|(config, _)| config)
            .unwrap_or_else(|| target.clone())
    }

    pub(crate) async fn resolve_project_input_for_auth(
        &self,
        project: &str,
        auth: Option<&AuthContext>,
    ) -> Result<ResolvedProject, ProjectResolverError> {
        let raw = project.trim();
        if raw.is_empty() {
            return Err(ProjectResolverError {
                kind: ProjectResolverErrorKind::UnknownProject,
                project: project.to_string(),
                candidates: self.agent_project_candidates_for_auth(auth).await,
            });
        }

        if raw.starts_with("agent:") {
            let Some(rest) = raw.strip_prefix("agent:") else {
                unreachable!();
            };
            let Some((client_id, agent_project_id)) = rest.split_once(':') else {
                let all_candidates = self.agent_project_candidates_for_auth(auth).await;
                return Err(ProjectResolverError {
                    kind: ProjectResolverErrorKind::UnknownProject,
                    project: raw.to_string(),
                    candidates: all_candidates,
                });
            };
            if client_id.trim().is_empty() || agent_project_id.trim().is_empty() {
                let all_candidates = self.agent_project_candidates_for_auth(auth).await;
                return Err(ProjectResolverError {
                    kind: ProjectResolverErrorKind::UnknownProject,
                    project: raw.to_string(),
                    candidates: all_candidates,
                });
            }

            let routing_epoch = self.runner_registry.project_routing_epoch();
            if let Some(cached) = self.cached_exact_project(raw, auth, routing_epoch) {
                return Ok(cached);
            }

            let access = crate::runner_http::runner_access_from_auth(auth);
            let Some(client) = self
                .runner_registry
                .get_runner_view_for_auth(client_id, access.as_ref())
                .await
            else {
                return Err(ProjectResolverError {
                    kind: ProjectResolverErrorKind::UnknownProject,
                    project: raw.to_string(),
                    candidates: Vec::new(),
                });
            };
            if let Some(project_summary) = client
                .projects
                .iter()
                .filter(|project| !project.disabled)
                .find(|project| project.id == agent_project_id)
            {
                let candidate = Self::project_candidate_from_view(&client, project_summary);
                let resolved = Self::resolved_from_candidate(project, &candidate);
                self.cache_exact_project(raw, auth, routing_epoch, resolved.clone());
                return Ok(resolved);
            }
            let mut same_client: Vec<ProjectResolverCandidate> = client
                .projects
                .iter()
                .filter(|project| !project.disabled)
                .map(|project| Self::project_candidate_from_view(&client, project))
                .collect();
            Self::sort_resolver_candidates(&mut same_client);
            return Err(ProjectResolverError {
                kind: ProjectResolverErrorKind::UnknownProject,
                project: raw.to_string(),
                candidates: same_client,
            });
        }

        let all_candidates = self.agent_project_candidates_for_auth(auth).await;

        if let Some((client_id, agent_project_id)) = raw.split_once(':') {
            if !client_id.trim().is_empty() && !agent_project_id.trim().is_empty() {
                let mut matches: Vec<ProjectResolverCandidate> = all_candidates
                    .iter()
                    .filter(|candidate| {
                        candidate.client_id == client_id
                            && candidate.agent_project_id == agent_project_id
                    })
                    .cloned()
                    .collect();
                Self::sort_resolver_candidates(&mut matches);
                match matches.len() {
                    1 => {
                        let candidate = matches.remove(0);
                        return Ok(Self::resolved_from_candidate(project, &candidate));
                    }
                    0 => {
                        let mut same_client: Vec<ProjectResolverCandidate> = all_candidates
                            .iter()
                            .filter(|candidate| candidate.client_id == client_id)
                            .cloned()
                            .collect();
                        Self::sort_resolver_candidates(&mut same_client);
                        return Err(ProjectResolverError {
                            kind: ProjectResolverErrorKind::UnknownProject,
                            project: raw.to_string(),
                            candidates: same_client,
                        });
                    }
                    _ => {
                        return Err(ProjectResolverError {
                            kind: ProjectResolverErrorKind::AmbiguousProject,
                            project: raw.to_string(),
                            candidates: matches,
                        });
                    }
                }
            }
        }

        let mut short_id_matches: Vec<ProjectResolverCandidate> = all_candidates
            .iter()
            .filter(|candidate| candidate.agent_project_id == raw)
            .cloned()
            .collect();
        Self::sort_resolver_candidates(&mut short_id_matches);
        match short_id_matches.len() {
            1 => {
                let candidate = short_id_matches.remove(0);
                return Ok(Self::resolved_from_candidate(project, &candidate));
            }
            n if n > 1 => {
                return Err(ProjectResolverError {
                    kind: ProjectResolverErrorKind::AmbiguousProject,
                    project: raw.to_string(),
                    candidates: short_id_matches,
                });
            }
            _ => {}
        }

        let mut name_matches: Vec<ProjectResolverCandidate> = all_candidates
            .iter()
            .filter(|candidate| candidate.name.as_deref() == Some(raw))
            .cloned()
            .collect();
        Self::sort_resolver_candidates(&mut name_matches);
        match name_matches.len() {
            1 => {
                let candidate = name_matches.remove(0);
                Ok(Self::resolved_from_candidate(project, &candidate))
            }
            n if n > 1 => Err(ProjectResolverError {
                kind: ProjectResolverErrorKind::AmbiguousProject,
                project: raw.to_string(),
                candidates: name_matches,
            }),
            _ => Err(ProjectResolverError {
                kind: ProjectResolverErrorKind::UnknownProject,
                project: raw.to_string(),
                candidates: all_candidates,
            }),
        }
    }

    /// Re-observe and reauthorize a managed-worktree repository knowledge source.
    /// Association identity is descriptive only; every call consults the current
    /// exact Runner inventory and ordinary Project authorization again.
    pub(crate) async fn resolve_project_knowledge_source_for_auth(
        &self,
        target: &ResolvedProject,
        auth: Option<&AuthContext>,
    ) -> ProjectKnowledgeSourceResolution {
        let Some(RunnerProjectLineage::ManagedWorktreeSource {
            source_project_id,
            source_root_fingerprint,
            base_sha,
        }) = target.knowledge_association.as_ref()
        else {
            return ProjectKnowledgeSourceResolution::NotAssociated;
        };

        let access = crate::runner_http::runner_access_from_auth(auth);
        let Some(runner) = self
            .runner_registry
            .get_runner_view_for_auth(&target.config.client_id, access.as_ref())
            .await
        else {
            return ProjectKnowledgeSourceResolution::Unavailable(
                ProjectKnowledgeUnavailableReason::Unauthorized,
            );
        };
        if !runner.connected {
            return ProjectKnowledgeSourceResolution::Unavailable(
                ProjectKnowledgeUnavailableReason::RunnerUnavailable,
            );
        }
        if runner
            .project_inventory
            .as_ref()
            .is_none_or(|status| status.sync_state != "complete")
        {
            return ProjectKnowledgeSourceResolution::Unavailable(
                ProjectKnowledgeUnavailableReason::InventoryIncomplete,
            );
        }
        let Some(source_summary) = runner
            .projects
            .iter()
            .find(|project| project.id == *source_project_id && !project.disabled)
        else {
            return ProjectKnowledgeSourceResolution::Unavailable(
                ProjectKnowledgeUnavailableReason::SourceUnavailable,
            );
        };
        let Some(current_root_fingerprint) = source_summary.root_fingerprint.as_deref() else {
            return ProjectKnowledgeSourceResolution::Unavailable(
                ProjectKnowledgeUnavailableReason::SourceIdentityUnavailable,
            );
        };
        if current_root_fingerprint != source_root_fingerprint {
            return ProjectKnowledgeSourceResolution::Stale;
        }

        let source_runtime_id =
            runner_project_runtime_id(&target.config.client_id, source_project_id);
        let Ok(source) = self
            .resolve_project_input_for_auth(&source_runtime_id, auth)
            .await
        else {
            return ProjectKnowledgeSourceResolution::Unavailable(
                ProjectKnowledgeUnavailableReason::Unauthorized,
            );
        };
        if source.config.client_id != target.config.client_id
            || source.resolved_id != source_runtime_id
        {
            return ProjectKnowledgeSourceResolution::Stale;
        }
        // The initial Runner view and canonical Project resolution are two
        // observations. Re-pin the final resolved source to the persisted root
        // identity so a same-id re-registration between those observations
        // cannot silently retarget knowledge reads.
        match source.root_fingerprint.as_deref() {
            None => {
                return ProjectKnowledgeSourceResolution::Unavailable(
                    ProjectKnowledgeUnavailableReason::SourceIdentityUnavailable,
                );
            }
            Some(current) if current != source_root_fingerprint => {
                return ProjectKnowledgeSourceResolution::Stale;
            }
            Some(_) => {}
        }
        ProjectKnowledgeSourceResolution::Available(AuthorizedProjectKnowledgeSource {
            source,
            base_sha: base_sha.clone(),
        })
    }

    pub(crate) async fn project_knowledge_association_diagnostic(
        &self,
        target: &ResolvedProject,
        auth: Option<&AuthContext>,
    ) -> Option<Value> {
        if target.knowledge_association.is_none() {
            return None;
        }
        let projection = match self
            .resolve_project_knowledge_source_for_auth(target, auth)
            .await
        {
            ProjectKnowledgeSourceResolution::NotAssociated => return None,
            ProjectKnowledgeSourceResolution::Available(available) => json!({
                "kind": "managed_worktree_source",
                "status": "available",
                "source_project": available.source.resolved_id,
                "base_sha": available.base_sha,
                "read_through": false,
            }),
            ProjectKnowledgeSourceResolution::Stale => json!({
                "kind": "managed_worktree_source",
                "status": "stale",
                "read_through": false,
            }),
            ProjectKnowledgeSourceResolution::Unavailable(reason) => {
                tracing::debug!(
                    target: "webcodex::project_knowledge",
                    reason = reason.as_str(),
                    "repository knowledge association unavailable"
                );
                json!({
                    "kind": "managed_worktree_source",
                    "status": "unavailable",
                    "read_through": false,
                })
            }
        };
        Some(projection)
    }

    pub(crate) async fn resolve_project_input(
        &self,
        project: &str,
    ) -> Result<ResolvedProject, ProjectResolverError> {
        self.resolve_project_input_for_auth(project, None).await
    }

    pub(crate) async fn resolve_project(
        &self,
        project: &str,
    ) -> Result<ProjectConfig, ProjectResolverError> {
        self.resolve_project_input(project)
            .await
            .map(|resolved| resolved.config)
    }

    pub(crate) async fn resolve_project_for_auth(
        &self,
        project: &str,
        auth: Option<&AuthContext>,
    ) -> Result<ProjectConfig, ProjectResolverError> {
        self.resolve_project_input_for_auth(project, auth)
            .await
            .map(|resolved| resolved.config)
    }
}
