//! Path helpers for the Skills page. The picker returns the canonical path in
//! the ordinary form (`C:\...`, `\\server\share\...`) that the helper reports
//! for external skill roots; Windows verbatim `\\?\` prefixes are removed.
use std::path::{Path, PathBuf};

pub fn canonical_skill_root(path: &Path) -> Result<String, String> {
    let canonical = std::fs::canonicalize(path).map_err(|_| "skill_root_not_found")?;
    if !canonical.is_dir() {
        return Err("skill_root_not_directory".into());
    }
    canonical
        .into_os_string()
        .into_string()
        .map(|path| strip_verbatim_prefix(&path))
        .map_err(|_| "folder_path_invalid".to_string())
}

/// `\\?\C:\x` -> `C:\x`, `\\?\UNC\server\share` -> `\\server\share`; anything else unchanged.
fn strip_verbatim_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    if let Some(rest) = path.strip_prefix(r"\\?\") {
        let bytes = rest.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            return rest.to_string();
        }
    }
    path.to_string()
}

/// Project-relative archive path with `/` separators, as `installSkill` expects.
pub fn project_relative_archive(project: &Path, archive: &Path) -> Result<String, String> {
    let outside = || "skill_archive_outside_project".to_string();
    let project: PathBuf = std::fs::canonicalize(project).map_err(|_| "project_path_unavailable")?;
    let archive = std::fs::canonicalize(archive).map_err(|_| "skill_archive_unavailable")?;
    let relative = archive.strip_prefix(&project).map_err(|_| outside())?;
    let parts: Vec<_> = relative
        .components()
        .map(|part| part.as_os_str().to_str().map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or("folder_path_invalid")?;
    if parts.is_empty() {
        return Err(outside());
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_inside_project_becomes_relative_with_forward_slashes() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        let zip = nested.join("skill.zip");
        std::fs::write(&zip, b"zip").unwrap();
        assert_eq!(project_relative_archive(root.path(), &zip).unwrap(), "a/b/skill.zip");
    }

    #[test]
    fn archive_outside_project_is_rejected() {
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let zip = other.path().join("skill.zip");
        std::fs::write(&zip, b"zip").unwrap();
        assert_eq!(
            project_relative_archive(project.path(), &zip).unwrap_err(),
            "skill_archive_outside_project"
        );
    }

    #[test]
    fn skill_root_is_exact_canonical_directory() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("skills");
        std::fs::create_dir(&dir).unwrap();
        let dotted = dir.join(".").join("..").join("skills");
        assert_eq!(
            canonical_skill_root(&dotted).unwrap(),
            strip_verbatim_prefix(std::fs::canonicalize(&dir).unwrap().to_str().unwrap())
        );
        let file = root.path().join("f");
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(canonical_skill_root(&file).unwrap_err(), "skill_root_not_directory");
        assert_eq!(canonical_skill_root(&root.path().join("nope")).unwrap_err(), "skill_root_not_found");
    }

    #[test]
    fn verbatim_prefix_is_stripped_only_for_disk_and_unc() {
        assert_eq!(strip_verbatim_prefix(r"\\?\C:\Skills"), r"C:\Skills");
        assert_eq!(strip_verbatim_prefix(r"\\?\UNC\srv\share\s"), r"\\srv\share\s");
        assert_eq!(strip_verbatim_prefix(r"\\?\Volume{x}\s"), r"\\?\Volume{x}\s");
        assert_eq!(strip_verbatim_prefix("/Users/a/skills"), "/Users/a/skills");
    }
}
