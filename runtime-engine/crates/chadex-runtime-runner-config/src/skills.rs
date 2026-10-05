//! Shared validation for the Runner `[skills]` table, used by the Runner when
//! it loads `runner.toml` and by Chadex before it writes the table.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const MAX_CONFIGURED_SKILL_ROOTS: usize = 16;
pub const MAX_CONFIGURED_SKILL_ROOT_PATH_BYTES: usize = 4096;

pub fn configured_skill_root_identity(root: &Path) -> String {
    let lexical = root.components().collect::<PathBuf>();
    crate::paths::normalize_path_identity(&lexical)
}

/// Validate `skills.roots` and `skills.script_roots`. Script execution is
/// opt-in per root, so every `script_roots` entry must name a configured root.
pub fn validate_configured_skill_roots(
    roots: &[PathBuf],
    script_roots: &[PathBuf],
) -> Result<(), String> {
    if roots.len() > MAX_CONFIGURED_SKILL_ROOTS {
        return Err(format!(
            "skills.roots may contain at most {MAX_CONFIGURED_SKILL_ROOTS} entries"
        ));
    }
    let mut identities = HashSet::with_capacity(roots.len());
    for root in roots {
        validate_root_path(root, "skills.roots")?;
        if !identities.insert(configured_skill_root_identity(root)) {
            return Err("skills.roots contains duplicate path identities".to_string());
        }
    }
    let mut script_identities = HashSet::with_capacity(script_roots.len());
    for root in script_roots {
        validate_root_path(root, "skills.script_roots")?;
        let identity = configured_skill_root_identity(root);
        if !identities.contains(&identity) {
            return Err(
                "skills.script_roots entries must also be listed in skills.roots".to_string(),
            );
        }
        if !script_identities.insert(identity) {
            return Err("skills.script_roots contains duplicate path identities".to_string());
        }
    }
    Ok(())
}

fn validate_root_path(root: &Path, field: &str) -> Result<(), String> {
    let text = root.to_string_lossy();
    if text.is_empty() || text.len() > MAX_CONFIGURED_SKILL_ROOT_PATH_BYTES || text.contains('\0')
    {
        return Err(format!(
            "{field} entries must be non-empty paths of at most {MAX_CONFIGURED_SKILL_ROOT_PATH_BYTES} bytes"
        ));
    }
    if !root.is_absolute() || crate::paths::project_path_has_parent_traversal(root) {
        return Err(format!(
            "{field} entries must be absolute paths without parent traversal"
        ));
    }
    #[cfg(windows)]
    if crate::paths::windows_project_path_kind(root)
        == Some(crate::paths::WindowsProjectPathKind::UnsupportedNamespace)
    {
        return Err(format!(
            "{field} contains an unsupported Windows path namespace"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn script_roots_must_be_a_subset_of_roots() {
        let a = PathBuf::from("/skills/a");
        let b = PathBuf::from("/skills/b");
        assert!(validate_configured_skill_roots(&[a.clone(), b.clone()], &[a.clone()]).is_ok());
        assert!(validate_configured_skill_roots(&[a.clone()], &[]).is_ok());
        assert_eq!(
            validate_configured_skill_roots(&[a.clone()], &[b.clone()]),
            Err("skills.script_roots entries must also be listed in skills.roots".to_string())
        );
        assert_eq!(
            validate_configured_skill_roots(&[a.clone()], &[a.clone(), PathBuf::from("/skills/./a")]),
            Err("skills.script_roots contains duplicate path identities".to_string())
        );
        assert!(validate_configured_skill_roots(&[a.clone()], &[PathBuf::from("rel")]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn roots_are_bounded_absolute_and_unique() {
        let many = (0..=MAX_CONFIGURED_SKILL_ROOTS)
            .map(|index| PathBuf::from(format!("/skills/{index}")))
            .collect::<Vec<_>>();
        assert!(validate_configured_skill_roots(&many, &[]).is_err());
        assert!(validate_configured_skill_roots(&[PathBuf::from("/a/../b")], &[]).is_err());
        assert_eq!(
            validate_configured_skill_roots(
                &[PathBuf::from("/skills/a"), PathBuf::from("/skills/a/")],
                &[]
            ),
            Err("skills.roots contains duplicate path identities".to_string())
        );
    }
}
