//! Path helpers for the Skills page. The picker returns the canonical path in
//! the ordinary form (`C:\...`, `\\server\share\...`) that the helper reports
//! for external skill roots; Windows verbatim `\\?\` prefixes are removed.
use serde_json::Value;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

/// Same cap the runtime enforces on a Skill archive (`MAX_SKILL_STORE_ARCHIVE_BYTES`, 8 MiB).
pub const MAX_SKILL_ARCHIVE_BYTES: u64 = 8 * 1024 * 1024;
/// Staging directory for user-chosen ZIPs, relative to the project root.
const STAGING_DIR: [&str; 2] = [".chadex", "skill-imports"];
const GITIGNORE_ENTRY: &str = ".chadex/skill-imports/";
/// Copies left behind by a crash or force-quit are purged once they are this old.
const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// The picker result: the canonical absolute path of a regular `.zip` within the size cap.
/// The ZIP may live anywhere; `stage_install_params` copies it into the project at install time.
pub fn chosen_skill_archive(path: &Path) -> Result<String, String> {
    let canonical = std::fs::canonicalize(path).map_err(|_| "skill_archive_unreadable")?;
    preflight_archive(&canonical, MAX_SKILL_ARCHIVE_BYTES)?;
    canonical
        .into_os_string()
        .into_string()
        .map(|path| strip_verbatim_prefix(&path))
        .map_err(|_| "folder_path_invalid".to_string())
}

/// A leaf that is not a `.zip`, not a plain file (symlink, folder), unreadable, or over `limit`.
fn preflight_archive(archive: &Path, limit: u64) -> Result<u64, String> {
    let is_zip = archive
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return Err("skill_archive_not_zip".into());
    }
    let meta = std::fs::symlink_metadata(archive).map_err(|_| "skill_archive_unreadable")?;
    if !meta.is_file() {
        return Err("skill_archive_not_regular_file".into());
    }
    if meta.len() > limit {
        return Err("skill_archive_too_large".into());
    }
    Ok(meta.len())
}

/// A temporary copy of a user-chosen ZIP inside the project. Deleted on drop, so it is removed
/// whether the install succeeds, fails, or the future is cancelled.
#[derive(Debug)]
pub struct StagedArchive {
    path: PathBuf,
    /// Project-relative path with `/` separators, as `installSkill` expects.
    relative: String,
}

impl StagedArchive {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn relative(&self) -> &str {
        &self.relative
    }
}

impl Drop for StagedArchive {
    fn drop(&mut self) {
        // The now-empty `skill-imports` directory stays.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// True for a drive path, UNC path, or rooted path: what the picker returns, as opposed to the
/// project-relative path the runtime expects.
fn is_absolute_archive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    Path::new(path).is_absolute()
        || path.starts_with('\\')
        || path.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// For `installSkill`: when `artifact_path` is an absolute path chosen by the picker, copy that
/// ZIP into `<project>/.chadex/skill-imports/` and rewrite `artifact_path` to the project-relative
/// copy. The returned guard deletes the copy when dropped, so keep it alive until the install
/// request has finished. A project-relative `artifact_path` is passed through unchanged.
pub fn stage_install_params(
    mut params: Value,
    limit: u64,
) -> Result<(Value, Option<StagedArchive>), String> {
    let Some(archive) = params
        .get("artifact_path")
        .and_then(Value::as_str)
        .filter(|path| is_absolute_archive_path(path))
        .map(PathBuf::from)
    else {
        return Ok((params, None));
    };
    let project = params
        .get("path")
        .and_then(Value::as_str)
        .ok_or("skill_archive_project_unavailable")?;
    let staged = stage_archive(Path::new(project), &archive, limit)?;
    params["artifact_path"] = Value::String(staged.relative().to_string());
    Ok((params, Some(staged)))
}

/// Copies `archive` to `<project>/.chadex/skill-imports/<unique>.zip`. The size is checked before
/// copying and again while copying, the destination is created exclusively under a unique name,
/// and a failed copy leaves nothing behind.
pub fn stage_archive(project: &Path, archive: &Path, limit: u64) -> Result<StagedArchive, String> {
    preflight_archive(archive, limit)?;
    let mut dir = std::fs::canonicalize(project)
        .ok()
        .filter(|path| path.is_dir())
        .ok_or("skill_archive_project_unavailable")?;
    for part in STAGING_DIR {
        dir.push(part);
        ensure_real_directory(&dir)?;
    }
    purge_stale(&dir);
    // Best effort: a read-only .gitignore must not block an install whose copy is removed right after.
    let _ = ensure_gitignore_entry(project);

    let mut source = std::fs::File::open(archive).map_err(|_| "skill_archive_unreadable")?;
    let meta = source.metadata().map_err(|_| "skill_archive_unreadable")?;
    if !meta.is_file() {
        return Err("skill_archive_not_regular_file".into());
    }
    if meta.len() > limit {
        return Err("skill_archive_too_large".into());
    }
    let (mut output, path, name) = create_unique(&dir)?;
    // From here the guard owns the file: any early return deletes it.
    let staged = StagedArchive {
        path,
        relative: format!("{}/{}/{name}", STAGING_DIR[0], STAGING_DIR[1]),
    };
    // Read one byte past the cap so a file that grew after the preflight is caught.
    let copied = std::io::copy(&mut (&mut source).take(limit + 1), &mut output)
        .map_err(|_| "skill_archive_copy_failed")?;
    if copied > limit {
        return Err("skill_archive_too_large".into());
    }
    output.sync_all().map_err(|_| "skill_archive_copy_failed")?;
    Ok(staged)
}

fn create_unique(dir: &Path) -> Result<(std::fs::File, PathBuf, String), String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let mut hasher = RandomState::new().build_hasher();
        hasher.write_u128(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default(),
        );
        hasher.write_u64(COUNTER.fetch_add(1, Ordering::Relaxed));
        hasher.write_u32(std::process::id());
        let name = format!("{:016x}.zip", hasher.finish());
        let path = dir.join(&name);
        // create_new never overwrites or follows an existing entry.
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path, name)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err("skill_archive_copy_failed".into()),
        }
    }
    Err("skill_archive_copy_failed".into())
}

/// Creates `path` if missing; refuses anything that is not a plain directory (a symlink or
/// junction would redirect the staged copy outside the project).
fn ensure_real_directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(()),
        Ok(_) => Err("skill_archive_unsafe_directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err("skill_archive_copy_failed".into()),
            }
            match std::fs::symlink_metadata(path) {
                Ok(meta) if meta.is_dir() => Ok(()),
                _ => Err("skill_archive_unsafe_directory".into()),
            }
        }
        Err(_) => Err("skill_archive_copy_failed".into()),
    }
}

fn purge_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_zip = path.extension().is_some_and(|ext| ext == "zip");
        let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
        let old = meta
            .modified()
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age > STALE_AFTER);
        if is_zip && meta.is_file() && old {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// True when some line already ignores `.chadex/skill-imports/` (directly or via `.chadex`).
fn gitignore_covers(text: &str) -> bool {
    const COVERING: [&str; 8] = [
        ".chadex",
        ".chadex/",
        "/.chadex",
        "/.chadex/",
        ".chadex/skill-imports",
        ".chadex/skill-imports/",
        "/.chadex/skill-imports",
        "/.chadex/skill-imports/",
    ];
    text.lines().any(|line| COVERING.contains(&line.trim()))
}

/// Appends `.chadex/skill-imports/` to the project's `.gitignore` unless already covered. Existing
/// bytes are preserved; only the entry (and a separating newline, in the file's own line-ending
/// style) is appended. A missing file is created.
pub fn ensure_gitignore_entry(project: &Path) -> Result<(), String> {
    let path = project.join(".gitignore");
    let existing = match std::fs::symlink_metadata(&path) {
        Ok(meta) if meta.is_file() => std::fs::read(&path).map_err(|_| "skill_archive_gitignore_failed")?,
        Ok(_) => return Err("skill_archive_gitignore_failed".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(_) => return Err("skill_archive_gitignore_failed".into()),
    };
    let text = String::from_utf8_lossy(&existing);
    if gitignore_covers(&text) {
        return Ok(());
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut addition = String::new();
    if existing.last().is_some_and(|byte| *byte != b'\n') {
        addition.push_str(newline);
    }
    addition.push_str(GITIGNORE_ENTRY);
    addition.push_str(newline);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|_| "skill_archive_gitignore_failed")?;
    file.write_all(addition.as_bytes())
        .map_err(|_| "skill_archive_gitignore_failed".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _root: tempfile::TempDir,
        project: PathBuf,
        downloads: PathBuf,
    }

    fn fixture() -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let project = root.path().join("project");
        let downloads = root.path().join("Downloads");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        Fixture { _root: root, project, downloads }
    }

    fn zip(fx: &Fixture, name: &str, bytes: usize) -> PathBuf {
        let path = fx.downloads.join(name);
        std::fs::write(&path, vec![7u8; bytes]).unwrap();
        path
    }

    fn staged_names(fx: &Fixture) -> Vec<String> {
        std::fs::read_dir(fx.project.join(".chadex").join("skill-imports"))
            .map(|dir| dir.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn chooser_accepts_zip_outside_any_project_and_returns_absolute_path() {
        let fx = fixture();
        let zip = zip(&fx, "skill.zip", 10);
        let chosen = chosen_skill_archive(&zip).unwrap();
        assert!(is_absolute_archive_path(&chosen));
        assert_eq!(
            std::fs::canonicalize(&chosen).unwrap(),
            std::fs::canonicalize(&zip).unwrap()
        );
    }

    #[test]
    fn chooser_rejects_non_zip_missing_oversized_and_folders() {
        let fx = fixture();
        let txt = zip(&fx, "skill.txt", 10);
        assert_eq!(chosen_skill_archive(&txt).unwrap_err(), "skill_archive_not_zip");
        assert_eq!(
            chosen_skill_archive(&fx.downloads.join("nope.zip")).unwrap_err(),
            "skill_archive_unreadable"
        );
        let dir = fx.downloads.join("folder.zip");
        std::fs::create_dir(&dir).unwrap();
        assert_eq!(chosen_skill_archive(&dir).unwrap_err(), "skill_archive_not_regular_file");
        let big = zip(&fx, "big.zip", MAX_SKILL_ARCHIVE_BYTES as usize + 1);
        assert_eq!(chosen_skill_archive(&big).unwrap_err(), "skill_archive_too_large");
        let exact = zip(&fx, "exact.zip", MAX_SKILL_ARCHIVE_BYTES as usize);
        assert!(chosen_skill_archive(&exact).is_ok());
    }

    #[test]
    fn size_limit_matches_the_runtime_cap() {
        assert_eq!(MAX_SKILL_ARCHIVE_BYTES, 8 * 1024 * 1024);
    }

    #[test]
    fn stage_copies_to_unique_project_relative_path_and_guard_deletes_it() {
        let fx = fixture();
        let source = zip(&fx, "skill.zip", 1000);
        let first = stage_archive(&fx.project, &source, MAX_SKILL_ARCHIVE_BYTES).unwrap();
        let second = stage_archive(&fx.project, &source, MAX_SKILL_ARCHIVE_BYTES).unwrap();
        assert!(first.relative().starts_with(".chadex/skill-imports/"));
        assert!(first.relative().ends_with(".zip"));
        assert!(!first.relative().contains('\\'));
        assert_ne!(first.relative(), second.relative());
        assert_eq!(std::fs::read(first.path()).unwrap(), std::fs::read(&source).unwrap());
        assert!(source.exists(), "the original is left alone");
        let (first_path, second_path) = (first.path().to_path_buf(), second.path().to_path_buf());
        drop(first);
        assert!(!first_path.exists());
        assert!(second_path.exists());
        drop(second);
        assert!(staged_names(&fx).is_empty());
        assert!(fx.project.join(".chadex").join("skill-imports").is_dir(), "empty dir stays");
    }

    #[test]
    fn stage_never_touches_existing_files_in_the_staging_directory() {
        let fx = fixture();
        let dir = fx.project.join(".chadex").join("skill-imports");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("keep.zip"), b"keep").unwrap();
        let staged = stage_archive(&fx.project, &zip(&fx, "a.zip", 5), 100).unwrap();
        assert_eq!(std::fs::read(dir.join("keep.zip")).unwrap(), b"keep");
        assert_ne!(staged.path().file_name().unwrap(), "keep.zip");
    }

    #[test]
    fn stage_rejects_oversized_before_creating_anything() {
        let fx = fixture();
        let source = zip(&fx, "a.zip", 101);
        assert_eq!(stage_archive(&fx.project, &source, 100).unwrap_err(), "skill_archive_too_large");
        assert!(!fx.project.join(".chadex").exists());
        assert!(!fx.project.join(".gitignore").exists());
        let exact = zip(&fx, "b.zip", 100);
        assert!(stage_archive(&fx.project, &exact, 100).is_ok());
    }

    #[test]
    fn stage_rejects_bad_sources_and_missing_project() {
        let fx = fixture();
        let txt = zip(&fx, "a.txt", 3);
        assert_eq!(stage_archive(&fx.project, &txt, 100).unwrap_err(), "skill_archive_not_zip");
        assert_eq!(
            stage_archive(&fx.project, &fx.downloads.join("nope.zip"), 100).unwrap_err(),
            "skill_archive_unreadable"
        );
        let ok = zip(&fx, "ok.zip", 3);
        assert_eq!(
            stage_archive(&fx.project.join("gone"), &ok, 100).unwrap_err(),
            "skill_archive_project_unavailable"
        );
        assert!(staged_names(&fx).is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn stage_rejects_symlink_sources_and_symlinked_staging_directories() {
        let fx = fixture();
        let real = zip(&fx, "real.zip", 3);
        let link = fx.downloads.join("link.zip");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(stage_archive(&fx.project, &link, 100).unwrap_err(), "skill_archive_not_regular_file");

        let outside = fx.downloads.join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, fx.project.join(".chadex")).unwrap();
        assert_eq!(stage_archive(&fx.project, &real, 100).unwrap_err(), "skill_archive_unsafe_directory");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);

        std::fs::remove_file(fx.project.join(".chadex")).unwrap();
        std::fs::create_dir(fx.project.join(".chadex")).unwrap();
        std::os::unix::fs::symlink(&outside, fx.project.join(".chadex").join("skill-imports")).unwrap();
        assert_eq!(stage_archive(&fx.project, &real, 100).unwrap_err(), "skill_archive_unsafe_directory");
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    }

    #[test]
    fn stale_leftovers_are_purged_but_fresh_and_foreign_files_stay() {
        let fx = fixture();
        let dir = fx.project.join(".chadex").join("skill-imports");
        std::fs::create_dir_all(&dir).unwrap();
        let old = SystemTime::now() - STALE_AFTER * 3;
        for name in ["old.zip", "old.txt"] {
            let file = std::fs::File::create(dir.join(name)).unwrap();
            file.set_modified(old).unwrap();
        }
        std::fs::write(dir.join("fresh.zip"), b"x").unwrap();
        drop(stage_archive(&fx.project, &zip(&fx, "a.zip", 3), 100).unwrap());
        let mut names = staged_names(&fx);
        names.sort();
        assert_eq!(names, ["fresh.zip", "old.txt"]);
    }

    #[test]
    fn install_params_with_absolute_archive_are_rewritten_and_cleaned_up() {
        let fx = fixture();
        let source = zip(&fx, "skill.zip", 20);
        let params = serde_json::json!({
            "path": fx.project.to_str().unwrap(),
            "skill_key": "demo",
            "artifact_path": source.to_str().unwrap(),
        });
        let (rewritten, staged) = stage_install_params(params, MAX_SKILL_ARCHIVE_BYTES).unwrap();
        let staged = staged.expect("staged");
        assert_eq!(rewritten["artifact_path"], staged.relative());
        assert_eq!(rewritten["skill_key"], "demo");
        assert!(fx.project.join(staged.relative()).is_file());
        drop(staged);
        assert!(staged_names(&fx).is_empty());
    }

    #[test]
    fn install_params_with_relative_archive_pass_through_untouched() {
        let fx = fixture();
        let params = serde_json::json!({
            "path": fx.project.to_str().unwrap(), "skill_key": "demo", "artifact_path": "skills/pack.zip",
        });
        let (out, staged) = stage_install_params(params.clone(), 100).unwrap();
        assert_eq!(out, params);
        assert!(staged.is_none());
        assert!(!fx.project.join(".chadex").exists());
    }

    #[test]
    fn install_params_errors_do_not_leave_a_copy() {
        let fx = fixture();
        let big = zip(&fx, "big.zip", 101);
        let params = serde_json::json!({
            "path": fx.project.to_str().unwrap(), "skill_key": "demo", "artifact_path": big.to_str().unwrap(),
        });
        assert_eq!(stage_install_params(params, 100).unwrap_err(), "skill_archive_too_large");
        assert!(staged_names(&fx).is_empty());
        let no_project = serde_json::json!({ "skill_key": "demo", "artifact_path": big.to_str().unwrap() });
        assert_eq!(stage_install_params(no_project, 100).unwrap_err(), "skill_archive_project_unavailable");
    }

    #[test]
    fn absolute_archive_detection_covers_windows_and_posix_forms() {
        for path in [r"C:\Users\a\Downloads\s.zip", r"c:/x/s.zip", r"\\server\share\s.zip", "/tmp/s.zip"] {
            assert!(is_absolute_archive_path(path), "{path}");
        }
        for path in ["skills/pack.zip", r".chadex\skill-imports\a.zip", "a.zip"] {
            assert!(!is_absolute_archive_path(path), "{path}");
        }
    }

    #[test]
    fn gitignore_is_created_appended_and_not_duplicated() {
        let fx = fixture();
        let file = fx.project.join(".gitignore");
        ensure_gitignore_entry(&fx.project).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ".chadex/skill-imports/\n");
        ensure_gitignore_entry(&fx.project).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), ".chadex/skill-imports/\n");

        std::fs::write(&file, "node_modules/\n*.log").unwrap();
        ensure_gitignore_entry(&fx.project).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "node_modules/\n*.log\n.chadex/skill-imports/\n");

        std::fs::write(&file, "dist/\n").unwrap();
        ensure_gitignore_entry(&fx.project).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "dist/\n.chadex/skill-imports/\n");
    }

    #[test]
    fn gitignore_keeps_crlf_style_and_non_utf8_bytes() {
        let fx = fixture();
        let file = fx.project.join(".gitignore");
        std::fs::write(&file, b"dist/\r\nbuild").unwrap();
        ensure_gitignore_entry(&fx.project).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"dist/\r\nbuild\r\n.chadex/skill-imports/\r\n");
        std::fs::write(&file, b"caf\xe9/\n").unwrap();
        ensure_gitignore_entry(&fx.project).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"caf\xe9/\n.chadex/skill-imports/\n");
    }

    #[test]
    fn gitignore_already_covered_is_left_alone() {
        let fx = fixture();
        let file = fx.project.join(".gitignore");
        for line in [".chadex/skill-imports/", "/.chadex/skill-imports", ".chadex/", "  .chadex  "] {
            let original = format!("a\n{line}\nb\n");
            std::fs::write(&file, &original).unwrap();
            ensure_gitignore_entry(&fx.project).unwrap();
            assert_eq!(std::fs::read_to_string(&file).unwrap(), original, "{line}");
        }
        assert!(!gitignore_covers("# .chadex/skill-imports/\n.chadex-other/\n"));
    }

    #[test]
    fn gitignore_failure_does_not_block_staging() {
        let fx = fixture();
        std::fs::create_dir(fx.project.join(".gitignore")).unwrap();
        assert!(ensure_gitignore_entry(&fx.project).is_err());
        let staged = stage_archive(&fx.project, &zip(&fx, "a.zip", 3), 100).unwrap();
        assert!(staged.path().is_file());
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
