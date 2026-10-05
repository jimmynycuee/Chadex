use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

pub(crate) const GLOBAL_INSTRUCTIONS_ENV: &str = "CHADEX_GLOBAL_INSTRUCTIONS_PATH";
const MAX_FILE_BYTES: u64 = 8 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct GlobalInstructionsSnapshot {
    pub(crate) status: String,
    pub(crate) fingerprint: Option<String>,
    pub(crate) content: String,
    pub(crate) truncated: bool,
    pub(crate) total_chars: usize,
}

impl GlobalInstructionsSnapshot {
    fn empty() -> Self {
        Self {
            status: "empty".into(),
            fingerprint: None,
            content: String::new(),
            truncated: false,
            total_chars: 0,
        }
    }

    fn unavailable() -> Self {
        Self {
            status: "unavailable".into(),
            fingerprint: None,
            content: String::new(),
            truncated: false,
            total_chars: 0,
        }
    }
}

pub(crate) fn load_global_instructions() -> GlobalInstructionsSnapshot {
    let Some(path) = std::env::var_os(GLOBAL_INSTRUCTIONS_ENV) else {
        return GlobalInstructionsSnapshot::empty();
    };
    if path.is_empty() {
        return GlobalInstructionsSnapshot::empty();
    }
    load_global_instructions_path(Path::new(&path))
}

pub(crate) fn load_global_instructions_path(path: &Path) -> GlobalInstructionsSnapshot {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return GlobalInstructionsSnapshot::empty()
        }
        Err(_) => return GlobalInstructionsSnapshot::unavailable(),
    };
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return GlobalInstructionsSnapshot::unavailable();
    }
    if metadata.len() > MAX_FILE_BYTES {
        return GlobalInstructionsSnapshot::unavailable();
    }
    let raw = match fs::read(path) {
        Ok(raw) => raw,
        Err(_) => return GlobalInstructionsSnapshot::unavailable(),
    };
    let text = match String::from_utf8(raw) {
        Ok(text) => text,
        Err(_) => return GlobalInstructionsSnapshot::unavailable(),
    };
    if text.trim().is_empty() {
        return GlobalInstructionsSnapshot::empty();
    }

    let total_chars = text.chars().count();
    let fingerprint = format!("{:x}", Sha256::digest(text.as_bytes()));

    GlobalInstructionsSnapshot {
        status: "loaded".into(),
        fingerprint: Some(fingerprint),
        content: text,
        truncated: false,
        total_chars,
    }
}

pub(crate) fn global_instructions_projection(
    snapshot: &GlobalInstructionsSnapshot,
    include_content: bool,
) -> Value {
    let mut value = json!({
        "status": snapshot.status,
        "scope": "all_chadex_projects",
        "storage": "chadex_managed",
        "precedence": {
            "safety_envelope": "non_overridable",
            "behavior_high_to_low": [
                "current_user_instruction",
                "repo_nested_agents",
                "repo_root_agents",
                "chadex_global_instructions",
                "chadex_builtin_baseline"
            ]
        },
        "note": "Chadex Global Instructions are an app-owned preference, not a repository AGENTS.md. System/platform safety and authority constraints remain non-overridable."
    });
    if snapshot.status == "loaded" {
        if let Some(fingerprint) = &snapshot.fingerprint {
            value["fingerprint"] = json!(fingerprint);
        }
        if include_content {
            value["content"] = json!(snapshot.content);
            value["content_included"] = json!(true);
        }
        if snapshot.truncated {
            value["truncated"] = json!(true);
            value["total_chars"] = json!(snapshot.total_chars);
        }
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_or_blank_global_instructions_are_safe_empty_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let missing = directory.path().join("missing.md");
        assert_eq!(load_global_instructions_path(&missing).status, "empty");
        let blank = directory.path().join("global-instructions.md");
        fs::write(&blank, " \n\t\n").unwrap();
        let snapshot = load_global_instructions_path(&blank);
        assert_eq!(snapshot.status, "empty");
        assert!(snapshot.content.is_empty());
        let oversized = directory.path().join("oversized.md");
        fs::write(&oversized, "x".repeat(MAX_FILE_BYTES as usize + 1)).unwrap();
        assert_eq!(
            load_global_instructions_path(&oversized).status,
            "unavailable"
        );
    }

    #[test]
    fn global_instructions_are_bounded_and_have_explicit_precedence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("global-instructions.md");
        fs::write(&path, "# Global\nPrefer concise output.\n").unwrap();
        let snapshot = load_global_instructions_path(&path);
        assert_eq!(snapshot.status, "loaded");
        assert!(snapshot.fingerprint.is_some());
        let projection = global_instructions_projection(&snapshot, true);
        assert_eq!(projection["scope"], "all_chadex_projects");
        assert_eq!(projection["storage"], "chadex_managed");
        assert_eq!(
            projection["precedence"]["safety_envelope"],
            "non_overridable"
        );
        assert_eq!(
            projection["precedence"]["behavior_high_to_low"][0],
            "current_user_instruction"
        );
        assert_eq!(
            projection["precedence"]["behavior_high_to_low"][3],
            "chadex_global_instructions"
        );
        assert_eq!(projection["content"], "# Global\nPrefer concise output.\n");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_global_instruction_file_is_not_trusted() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real.md");
        let link = directory.path().join("global-instructions.md");
        fs::write(&real, "secret ambient content").unwrap();
        symlink(&real, &link).unwrap();
        assert_eq!(load_global_instructions_path(&link).status, "unavailable");
    }
}
