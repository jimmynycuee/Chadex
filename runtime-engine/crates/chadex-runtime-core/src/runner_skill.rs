//! Canonical Runner-local Skill runtime and management protocol.
//!
//! Configured live roots and the managed Skill store remain distinct Runner-local
//! sources and lifecycles. This module only unifies their cross-process request
//! family, source identity, and bounded read/list/resolve responses.

use crate::runtime_contract::{MAX_SKILL_READ_LINES, MAX_SKILL_RESOURCE_PATH_CHARS};
use crate::skill_metadata::{MAX_SKILL_DESCRIPTION_CHARS, MAX_SKILL_NAME_CHARS};
use crate::skill_store::{
    valid_lower_sha256, valid_package_revision, valid_skill_key, MAX_SKILL_STORE_VERSIONS_LIMIT,
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

pub const RUNNER_SKILL_REQUEST_KIND: &str = "skill";
pub const RUNNER_SKILL_RESPONSE_FORMAT: &str = "webcodex.runner_skill.v1";
pub const RUNNER_SKILL_REQUEST_MAX_BYTES: usize = 32 * 1024;
pub const RUNNER_SKILL_RESPONSE_MAX_BYTES: usize = 512 * 1024;
pub const MAX_RUNNER_SKILLS: usize = 512;
pub const MAX_RUNNER_SKILL_DIAGNOSTICS: usize = 8;
pub const MAX_RUNNER_SKILL_READ_TEXT_BYTES: usize = 48 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerSkillSource {
    Configured,
    Managed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunnerSkillDescriptor {
    Configured {
        skill_id: String,
        name: String,
        description: String,
        definition_revision: String,
    },
    Managed {
        skill_id: String,
        skill_key: String,
        name: String,
        description: String,
        package_revision: String,
        definition_revision: String,
    },
}

impl RunnerSkillDescriptor {
    pub fn source(&self) -> RunnerSkillSource {
        match self {
            Self::Configured { .. } => RunnerSkillSource::Configured,
            Self::Managed { .. } => RunnerSkillSource::Managed,
        }
    }

    pub fn skill_id(&self) -> &str {
        match self {
            Self::Configured { skill_id, .. } | Self::Managed { skill_id, .. } => skill_id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::Configured { name, .. } | Self::Managed { name, .. } => name,
        }
    }

    pub fn description(&self) -> &str {
        match self {
            Self::Configured { description, .. } | Self::Managed { description, .. } => description,
        }
    }

    pub fn definition_revision(&self) -> &str {
        match self {
            Self::Configured {
                definition_revision,
                ..
            }
            | Self::Managed {
                definition_revision,
                ..
            } => definition_revision,
        }
    }

    pub fn package_revision(&self) -> Option<&str> {
        match self {
            Self::Configured { .. } => None,
            Self::Managed {
                package_revision, ..
            } => Some(package_revision),
        }
    }

    pub fn skill_key(&self) -> Option<&str> {
        match self {
            Self::Configured { .. } => None,
            Self::Managed { skill_key, .. } => Some(skill_key),
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if !valid_runner_skill_id(self.skill_id())
            || self.name().is_empty()
            || self.name().chars().count() > MAX_SKILL_NAME_CHARS
            || self.description().chars().count() > MAX_SKILL_DESCRIPTION_CHARS
            || !valid_lower_sha256(self.definition_revision())
        {
            return Err("invalid Runner Skill descriptor");
        }
        if let Self::Managed {
            skill_key,
            package_revision,
            ..
        } = self
        {
            if !valid_skill_key(skill_key) || !valid_package_revision(package_revision) {
                return Err("invalid managed Runner Skill descriptor");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum RunnerSkillRequest {
    List,
    Resolve {
        skill_id: String,
    },
    Read {
        skill_id: String,
        expected_source: RunnerSkillSource,
        path: String,
        start_line: usize,
        limit: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_package_revision: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_definition_revision: Option<String>,
    },
    Versions {
        skill_key: String,
        #[serde(default)]
        offset: usize,
        limit: usize,
    },
    Install {
        skill_key: String,
        source_project_id: String,
        source_project_root: String,
        artifact_path: String,
        expected_artifact_sha256: String,
        idempotency_key: String,
        #[serde(default)]
        activate: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_state_revision: Option<String>,
    },
    Activate {
        skill_key: String,
        package_revision: String,
        expected_state_revision: String,
        idempotency_key: String,
    },
    RemoveRevision {
        skill_key: String,
        package_revision: String,
        expected_state_revision: String,
        idempotency_key: String,
    },
}

impl RunnerSkillRequest {
    pub fn requires_management_capability(&self) -> bool {
        matches!(
            self,
            Self::Versions { .. }
                | Self::Install { .. }
                | Self::Activate { .. }
                | Self::RemoveRevision { .. }
        )
    }

    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::Install { .. } | Self::Activate { .. } | Self::RemoveRevision { .. }
        )
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::List => Ok(()),
            Self::Resolve { skill_id } => {
                if valid_runner_skill_id(skill_id) {
                    Ok(())
                } else {
                    Err("invalid Runner Skill id")
                }
            }
            Self::Read {
                skill_id,
                expected_source,
                path,
                start_line,
                limit,
                expected_package_revision,
                expected_definition_revision,
            } => {
                if !valid_runner_skill_id(skill_id) {
                    return Err("invalid Runner Skill id");
                }
                normalize_runner_skill_resource_path(path)?;
                if *start_line == 0 || !(1..=MAX_SKILL_READ_LINES).contains(limit) {
                    return Err("invalid Runner Skill read range");
                }
                if *expected_source == RunnerSkillSource::Configured
                    && expected_package_revision.is_some()
                {
                    return Err("configured Runner Skill cannot pin package revision");
                }
                if expected_package_revision
                    .as_deref()
                    .is_some_and(|revision| !valid_package_revision(revision))
                {
                    return Err("invalid Runner Skill package revision");
                }
                if expected_definition_revision
                    .as_deref()
                    .is_some_and(|revision| !valid_lower_sha256(revision))
                {
                    return Err("invalid Runner Skill definition revision");
                }
                Ok(())
            }
            Self::Versions {
                skill_key, limit, ..
            } => {
                if valid_skill_key(skill_key)
                    && (1..=MAX_SKILL_STORE_VERSIONS_LIMIT).contains(limit)
                {
                    Ok(())
                } else {
                    Err("invalid Runner Skill versions request")
                }
            }
            Self::Install { skill_key, .. }
            | Self::Activate { skill_key, .. }
            | Self::RemoveRevision { skill_key, .. } => {
                if valid_skill_key(skill_key) {
                    Ok(())
                } else {
                    Err("invalid Runner Skill management request")
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerSkillListResponse {
    pub format: String,
    pub skills: Vec<RunnerSkillDescriptor>,
    pub invalid_count: usize,
    pub diagnostics: Vec<String>,
    pub discovery_truncated: bool,
}

impl RunnerSkillListResponse {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.format != RUNNER_SKILL_RESPONSE_FORMAT
            || self.skills.len() > MAX_RUNNER_SKILLS
            || self.diagnostics.len() > MAX_RUNNER_SKILL_DIAGNOSTICS
            || self
                .diagnostics
                .iter()
                .any(|reason| !valid_diagnostic_reason(reason))
        {
            return Err("invalid Runner Skill list response");
        }
        let mut seen = std::collections::BTreeSet::new();
        for skill in &self.skills {
            skill.validate()?;
            if !seen.insert(skill.skill_id()) {
                return Err("duplicate Runner Skill id");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerSkillResolveResponse {
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<RunnerSkillDescriptor>,
}

impl RunnerSkillResolveResponse {
    pub fn validate_for_request(&self, skill_id: &str) -> Result<(), &'static str> {
        if self.format != RUNNER_SKILL_RESPONSE_FORMAT {
            return Err("invalid Runner Skill resolve response");
        }
        if let Some(skill) = &self.skill {
            skill.validate()?;
            if skill.skill_id() != skill_id {
                return Err("Runner Skill resolve identity mismatch");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunnerSkillReadResponse {
    pub format: String,
    pub skill: RunnerSkillDescriptor,
    pub path: String,
    pub sha256: String,
    pub text: String,
    pub start_line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    pub returned_lines: usize,
    pub has_more: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_start_line: Option<usize>,
}

impl RunnerSkillReadResponse {
    pub fn validate_for_request(
        &self,
        skill_id: &str,
        expected_source: RunnerSkillSource,
        path: &str,
        start_line: usize,
        limit: usize,
    ) -> Result<(), &'static str> {
        let normalized_path = normalize_runner_skill_resource_path(path)?;
        self.skill.validate()?;
        if self.format != RUNNER_SKILL_RESPONSE_FORMAT
            || self.skill.skill_id() != skill_id
            || self.skill.source() != expected_source
            || self.path != normalized_path
            || !valid_lower_sha256(&self.sha256)
            || self.text.len() > MAX_RUNNER_SKILL_READ_TEXT_BYTES
            || self.start_line != start_line
            || self.returned_lines > limit
            || self.has_more != self.next_start_line.is_some()
            || (normalized_path == "SKILL.md" && self.sha256 != self.skill.definition_revision())
        {
            return Err("invalid Runner Skill read response");
        }
        Ok(())
    }
}

pub fn valid_runner_skill_id(value: &str) -> bool {
    value
        .strip_prefix("wc_skill_")
        .is_some_and(|suffix| crate::compact::decode::<16>(suffix).is_some())
}

pub fn normalize_runner_skill_resource_path(path: &str) -> Result<String, &'static str> {
    let trimmed = path.trim();
    #[cfg(windows)]
    if trimmed.contains(':') {
        return Err("invalid Runner Skill resource path");
    }
    if trimmed.is_empty()
        || trimmed.chars().count() > MAX_SKILL_RESOURCE_PATH_CHARS
        || trimmed.chars().any(char::is_control)
        || trimmed.starts_with('/')
        || trimmed.starts_with('\\')
        || trimmed.as_bytes().get(1) == Some(&b':')
    {
        return Err("invalid Runner Skill resource path");
    }
    let normalized = trimmed.replace('\\', "/");
    if normalized
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
        || Path::new(&normalized)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("invalid Runner Skill resource path");
    }
    Ok(normalized)
}

fn valid_diagnostic_reason(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> RunnerSkillDescriptor {
        RunnerSkillDescriptor::Configured {
            skill_id: "wc_skill_qqqqqqqqqqqqqqqqqqqqqg".to_string(),
            name: "configured".to_string(),
            description: "configured guidance".to_string(),
            definition_revision: "b".repeat(64),
        }
    }

    fn managed() -> RunnerSkillDescriptor {
        RunnerSkillDescriptor::Managed {
            skill_id: "wc_skill_zMzMzMzMzMzMzMzMzMzMzA".to_string(),
            skill_key: "managed".to_string(),
            name: "managed".to_string(),
            description: "managed guidance".to_string(),
            package_revision: "wc_skillpkg_3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d0".to_string(),
            definition_revision: "e".repeat(64),
        }
    }

    #[test]
    fn request_family_round_trips_all_runtime_and_management_operations() {
        let configured_id = configured().skill_id().to_string();
        let managed_id = managed().skill_id().to_string();
        let package_revision =
            "wc_skillpkg___________________________________________8".to_string();
        let state_revision =
            "wc_skillstate_qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqo".to_string();
        let requests = vec![
            RunnerSkillRequest::List,
            RunnerSkillRequest::Resolve {
                skill_id: configured_id.clone(),
            },
            RunnerSkillRequest::Read {
                skill_id: configured_id,
                expected_source: RunnerSkillSource::Configured,
                path: "references/guide.md".to_string(),
                start_line: 1,
                limit: 20,
                expected_package_revision: None,
                expected_definition_revision: Some("b".repeat(64)),
            },
            RunnerSkillRequest::Versions {
                skill_key: "managed".to_string(),
                offset: 0,
                limit: 20,
            },
            RunnerSkillRequest::Install {
                skill_key: "managed".to_string(),
                source_project_id: "agent:runner:demo".to_string(),
                source_project_root: "/repo".to_string(),
                artifact_path: "skill.zip".to_string(),
                expected_artifact_sha256: "c".repeat(64),
                idempotency_key: "install-key".to_string(),
                activate: true,
                expected_state_revision: None,
            },
            RunnerSkillRequest::Activate {
                skill_key: "managed".to_string(),
                package_revision: package_revision.clone(),
                expected_state_revision: state_revision.clone(),
                idempotency_key: "activate-key".to_string(),
            },
            RunnerSkillRequest::RemoveRevision {
                skill_key: "managed".to_string(),
                package_revision,
                expected_state_revision: state_revision,
                idempotency_key: "remove-key".to_string(),
            },
            RunnerSkillRequest::Read {
                skill_id: managed_id,
                expected_source: RunnerSkillSource::Managed,
                path: "SKILL.md".to_string(),
                start_line: 1,
                limit: 1,
                expected_package_revision: None,
                expected_definition_revision: None,
            },
        ];

        for (index, request) in requests.into_iter().enumerate() {
            request.validate().unwrap();
            let encoded = serde_json::to_string(&request).unwrap();
            assert!(encoded.len() <= RUNNER_SKILL_REQUEST_MAX_BYTES);
            let decoded: RunnerSkillRequest = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, request);
            assert_eq!(
                decoded.requires_management_capability(),
                (3..=6).contains(&index)
            );
            assert_eq!(decoded.is_mutation(), (4..=6).contains(&index));
        }
    }

    #[test]
    fn source_tagged_descriptors_preserve_source_specific_fields() {
        let configured_json = serde_json::to_value(configured()).unwrap();
        assert_eq!(configured_json["source"], "configured");
        assert!(configured_json.get("skill_key").is_none());
        assert!(configured_json.get("package_revision").is_none());

        let managed_json = serde_json::to_value(managed()).unwrap();
        assert_eq!(managed_json["source"], "managed");
        assert!(managed_json["skill_key"].is_string());
        assert!(managed_json["package_revision"].is_string());
    }

    #[test]
    fn read_request_pins_source_and_rejects_invalid_cross_source_revision_shape() {
        let request = RunnerSkillRequest::Read {
            skill_id: configured().skill_id().to_string(),
            expected_source: RunnerSkillSource::Configured,
            path: "SKILL.md".to_string(),
            start_line: 1,
            limit: 1,
            expected_package_revision: Some(
                "wc_skillpkg___________________________________________8".to_string(),
            ),
            expected_definition_revision: None,
        };
        assert!(request.validate().is_err());

        let managed_request = RunnerSkillRequest::Read {
            skill_id: managed().skill_id().to_string(),
            expected_source: RunnerSkillSource::Managed,
            path: "references/guide.md".to_string(),
            start_line: 1,
            limit: 20,
            expected_package_revision: None,
            expected_definition_revision: None,
        };
        managed_request.validate().unwrap();
    }

    #[test]
    fn list_and_resolve_responses_fail_closed_on_identity_inconsistency() {
        let duplicate = configured();
        let response = RunnerSkillListResponse {
            format: RUNNER_SKILL_RESPONSE_FORMAT.to_string(),
            skills: vec![duplicate.clone(), duplicate],
            invalid_count: 0,
            diagnostics: Vec::new(),
            discovery_truncated: false,
        };
        assert!(response.validate().is_err());

        let resolve = RunnerSkillResolveResponse {
            format: RUNNER_SKILL_RESPONSE_FORMAT.to_string(),
            skill: Some(managed()),
        };
        assert!(resolve
            .validate_for_request(configured().skill_id())
            .is_err());
    }

    #[test]
    fn resource_paths_are_relative_and_normalized() {
        assert_eq!(
            normalize_runner_skill_resource_path("references\\guide.md").unwrap(),
            "references/guide.md"
        );
        for invalid in [
            "../secret",
            "references/../secret",
            "/etc/passwd",
            "C:\\secret",
        ] {
            assert!(normalize_runner_skill_resource_path(invalid).is_err());
        }
    }
}
