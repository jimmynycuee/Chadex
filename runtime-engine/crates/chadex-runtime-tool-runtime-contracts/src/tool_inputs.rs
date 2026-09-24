//! Shared input types used by runtime tool calls.

use serde::{Deserialize, Deserializer, Serialize};

pub use webcodex_core::workflow_session_contract::{ExecutionPurpose, ExecutionShell, SessionMode};

/// Serde default helper: `true`. Used by `ToolCall` variants whose `allow_patch`
/// field defaults to true (matching the Runner-side Project TOML parser).
pub fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupDetail {
    Minimal,
    #[default]
    Standard,
    Full,
}

impl StartupDetail {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Standard => "standard",
            Self::Full => "full",
        }
    }
}

#[cfg(feature = "workspace-checkpoints")]
pub use webcodex_core::runtime_contract::{
    CHECKPOINT_KIND_VALUES, CHECKPOINT_VALIDATION_STATUS_VALUES,
};

#[cfg(feature = "workspace-checkpoints")]
pub fn is_checkpoint_kind(value: &str) -> bool {
    CHECKPOINT_KIND_VALUES.contains(&value)
}

#[cfg(feature = "workspace-checkpoints")]
pub fn is_checkpoint_validation_status(value: &str) -> bool {
    CHECKPOINT_VALIDATION_STATUS_VALUES.contains(&value)
}

// Exact edit primitives and kinds remain shared with the Runner wire contract,
// but the model-facing file-change DTO intentionally differs: models carry a
// short read revision while ToolRuntime translates it back to the wire SHA.
pub use webcodex_core::apply_edits_shared::{
    ApplyFileChangeKind, ApplyTextEditInput, ApplyTextEditKind, ApplyTextLineScope,
};

#[derive(Debug, Clone, Serialize)]
pub struct ApplyFileChangeInput {
    pub kind: ApplyFileChangeKind,
    pub path: String,
    pub to_path: Option<String>,
    pub content: Option<String>,
    pub edits: Vec<ApplyTextEditInput>,
    pub expected_read_revision: Option<u64>,
}

impl<'de> Deserialize<'de> for ApplyFileChangeInput {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Canonical {
            kind: ApplyFileChangeKind,
            path: String,
            #[serde(default)]
            to_path: Option<String>,
            #[serde(default)]
            content: Option<String>,
            #[serde(default)]
            edits: Vec<ApplyTextEditInput>,
            #[serde(default)]
            expected_read_revision: Option<u64>,
        }

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct ExactReplaceShorthand {
            path: String,
            old_text: String,
            new_text: String,
            #[serde(default)]
            expected_read_revision: Option<u64>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum WireInput {
            Canonical(Canonical),
            ExactReplace(ExactReplaceShorthand),
        }

        Ok(match WireInput::deserialize(deserializer)? {
            WireInput::Canonical(input) => Self {
                kind: input.kind,
                path: input.path,
                to_path: input.to_path,
                content: input.content,
                edits: input.edits,
                expected_read_revision: input.expected_read_revision,
            },
            WireInput::ExactReplace(input) => Self {
                kind: ApplyFileChangeKind::Edit,
                path: input.path,
                to_path: None,
                content: None,
                edits: vec![ApplyTextEditInput {
                    kind: ApplyTextEditKind::ReplaceExact,
                    old_text: Some(input.old_text),
                    new_text: Some(input.new_text),
                    anchor_text: None,
                    occurrence: None,
                    line_scope: None,
                }],
                expected_read_revision: input.expected_read_revision,
            },
        })
    }
}

#[cfg(feature = "workspace-checkpoints")]
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CheckpointValidationInput {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ListToolsOptions {
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub features: Option<String>,
    #[serde(default)]
    pub summary_only: bool,
    #[serde(default)]
    pub limit: Option<usize>,
}
