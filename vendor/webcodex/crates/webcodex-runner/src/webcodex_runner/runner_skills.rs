use super::config::{RunnerPolicy, SkillsConfig};
use super::configured_skills;
use super::output::CommandResult;
use super::skill_store::SkillStore;
use std::collections::BTreeSet;
use std::time::Instant;
use webcodex_core::runner_skill::{
    RunnerSkillDescriptor, RunnerSkillListResponse, RunnerSkillReadResponse, RunnerSkillRequest,
    RunnerSkillResolveResponse, RunnerSkillSource, RUNNER_SKILL_RESPONSE_FORMAT,
    RUNNER_SKILL_RESPONSE_MAX_BYTES,
};

pub(crate) fn handle_runner_skill_request(
    config: &SkillsConfig,
    client_id: &str,
    server_url: &str,
    policy: &RunnerPolicy,
    request: RunnerSkillRequest,
) -> CommandResult {
    let started = Instant::now();
    if request.validate().is_err() {
        return error_result(started, "skill_invalid_request");
    }
    let management = request.requires_management_capability();
    let store = match SkillStore::for_runner(client_id, server_url) {
        Ok(store) => store,
        Err(_) => {
            return error_result(
                started,
                if management {
                    "skill_store_unavailable"
                } else {
                    "skill_catalog_unavailable"
                },
            )
        }
    };

    let result = match request {
        RunnerSkillRequest::List => list_runner_skills(config, &store)
            .and_then(|response| serialize_bounded(response, "skill_response_invalid")),
        RunnerSkillRequest::Resolve { skill_id } => resolve_runner_skill(config, &store, &skill_id)
            .map(|skill| RunnerSkillResolveResponse {
                format: RUNNER_SKILL_RESPONSE_FORMAT.to_string(),
                skill,
            })
            .and_then(|response| {
                response
                    .validate_for_request(&skill_id)
                    .map_err(|_| "skill_response_invalid".to_string())?;
                serialize_bounded(response, "skill_response_invalid")
            }),
        RunnerSkillRequest::Read {
            skill_id,
            expected_source,
            path,
            start_line,
            limit,
            expected_package_revision,
            expected_definition_revision,
        } => read_runner_skill(
            config,
            &store,
            &skill_id,
            expected_source,
            &path,
            start_line,
            limit,
            expected_package_revision.as_deref(),
            expected_definition_revision.as_deref(),
        )
        .and_then(|response| serialize_bounded(response, "skill_response_invalid")),
        RunnerSkillRequest::Versions {
            skill_key,
            offset,
            limit,
        } => store
            .versions(&skill_key, offset, limit)
            .and_then(|response| serialize_bounded(response, "skill_store_response_invalid")),
        RunnerSkillRequest::Install {
            skill_key,
            source_project_id,
            source_project_root,
            artifact_path,
            expected_artifact_sha256,
            idempotency_key,
            activate,
            expected_state_revision,
        } => store
            .install(
                policy,
                &skill_key,
                &source_project_id,
                &source_project_root,
                &artifact_path,
                &expected_artifact_sha256,
                &idempotency_key,
                activate,
                expected_state_revision.as_deref(),
            )
            .and_then(|response| serialize_bounded(response, "skill_store_response_invalid")),
        RunnerSkillRequest::Activate {
            skill_key,
            package_revision,
            expected_state_revision,
            idempotency_key,
        } => store
            .activate(
                &skill_key,
                &package_revision,
                &expected_state_revision,
                &idempotency_key,
            )
            .and_then(|response| serialize_bounded(response, "skill_store_response_invalid")),
        RunnerSkillRequest::RemoveRevision {
            skill_key,
            package_revision,
            expected_state_revision,
            idempotency_key,
        } => store
            .remove_revision(
                &skill_key,
                &package_revision,
                &expected_state_revision,
                &idempotency_key,
            )
            .and_then(|response| serialize_bounded(response, "skill_store_response_invalid")),
    };

    match result {
        Ok(stdout) => CommandResult {
            exit_code: Some(0),
            stdout: Some(stdout),
            stderr: Some(String::new()),
            duration_ms: Some(started.elapsed().as_millis() as u64),
            error: None,
        },
        Err(code) => error_result(started, &code),
    }
}

fn list_runner_skills(
    config: &SkillsConfig,
    store: &SkillStore,
) -> Result<RunnerSkillListResponse, String> {
    let configured = configured_skills::discover(config)?;
    let managed = store.list_active()?;
    let _managed_namespace_revision = managed.namespace_revision;
    let mut skills = configured
        .skills
        .into_iter()
        .map(|skill| skill.descriptor)
        .collect::<Vec<_>>();
    skills.extend(managed.skills);

    ensure_unique_skill_ids(&skills)?;
    let mut response = RunnerSkillListResponse {
        format: RUNNER_SKILL_RESPONSE_FORMAT.to_string(),
        skills,
        invalid_count: configured.invalid_count,
        diagnostics: configured.diagnostics,
        discovery_truncated: configured.discovery_truncated,
    };
    loop {
        response
            .validate()
            .map_err(|_| "skill_response_invalid".to_string())?;
        let encoded =
            serde_json::to_string(&response).map_err(|_| "skill_response_invalid".to_string())?;
        if encoded.len() <= RUNNER_SKILL_RESPONSE_MAX_BYTES {
            return Ok(response);
        }
        if response.skills.pop().is_none() {
            return Err("skill_response_too_large".to_string());
        }
        response.discovery_truncated = true;
    }
}

fn ensure_unique_skill_ids(skills: &[RunnerSkillDescriptor]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for skill in skills {
        if !seen.insert(skill.skill_id()) {
            return Err("skill_catalog_unavailable".to_string());
        }
    }
    Ok(())
}

fn resolve_runner_skill(
    config: &SkillsConfig,
    store: &SkillStore,
    skill_id: &str,
) -> Result<Option<RunnerSkillDescriptor>, String> {
    let configured = configured_skills::resolve_live_skill(config, skill_id)?;
    let managed = store.resolve_active(skill_id)?;
    resolve_candidates(configured.map(|skill| skill.descriptor), managed)
}

fn resolve_candidates(
    configured: Option<RunnerSkillDescriptor>,
    managed: Option<RunnerSkillDescriptor>,
) -> Result<Option<RunnerSkillDescriptor>, String> {
    match (configured, managed) {
        (None, None) => Ok(None),
        (Some(skill), None) | (None, Some(skill)) => Ok(Some(skill)),
        (Some(_), Some(_)) => Err("skill_catalog_unavailable".to_string()),
    }
}

fn require_resolved_source(
    resolved: Option<&RunnerSkillDescriptor>,
    expected_source: RunnerSkillSource,
) -> Result<(), String> {
    match resolved {
        Some(skill) if skill.source() == expected_source => Ok(()),
        Some(_) | None => Err("skill_source_changed".to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn read_runner_skill(
    config: &SkillsConfig,
    store: &SkillStore,
    skill_id: &str,
    expected_source: RunnerSkillSource,
    path: &str,
    start_line: usize,
    limit: usize,
    expected_package_revision: Option<&str>,
    expected_definition_revision: Option<&str>,
) -> Result<RunnerSkillReadResponse, String> {
    let resolved = resolve_runner_skill(config, store, skill_id)?;
    require_resolved_source(resolved.as_ref(), expected_source)?;

    let response = match expected_source {
        RunnerSkillSource::Configured => configured_skills::read_resource(
            config,
            skill_id,
            path,
            start_line,
            limit,
            expected_definition_revision,
        )?,
        RunnerSkillSource::Managed => store.read_resource(
            skill_id,
            path,
            start_line,
            limit,
            expected_package_revision,
            expected_definition_revision,
        )?,
    };
    response
        .validate_for_request(skill_id, expected_source, path, start_line, limit)
        .map_err(|_| "skill_response_invalid".to_string())?;

    // The unified wire family must not allow a target to silently switch source
    // while the source-specific read is in flight. Re-resolve only identity/source;
    // managed revisions remain pinned exclusively by explicit caller expectations.
    let after = resolve_runner_skill(config, store, skill_id)?;
    require_resolved_source(after.as_ref(), expected_source)?;
    Ok(response)
}

fn serialize_bounded<T: serde::Serialize>(value: T, invalid_code: &str) -> Result<String, String> {
    let output = serde_json::to_string(&value).map_err(|_| invalid_code.to_string())?;
    if output.len() > RUNNER_SKILL_RESPONSE_MAX_BYTES {
        return Err("skill_response_too_large".to_string());
    }
    Ok(output)
}

fn error_result(started: Instant, code: &str) -> CommandResult {
    CommandResult {
        exit_code: None,
        stdout: None,
        stderr: None,
        duration_ms: Some(started.elapsed().as_millis() as u64),
        error: Some(code.to_string()),
    }
}

#[cfg(test)]
#[path = "runner_skills_tests.rs"]
mod tests;
