use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct GraphifyStatus {
    pub available: bool,
    pub path: Option<String>,
    pub source: String,
}

impl GraphifyStatus {
    pub(crate) fn detect() -> Self {
        let mut candidates: Vec<(PathBuf, &'static str)> = Vec::new();

        if let Some(path) = env::var_os("CHADEX_GRAPHIFY_BIN") {
            if !path.is_empty() {
                candidates.push((PathBuf::from(path), "CHADEX_GRAPHIFY_BIN"));
            }
        }

        if let Some(path) = env::var_os("PATH") {
            candidates.extend(
                env::split_paths(&path).map(|directory| (directory.join("graphify"), "PATH")),
            );
        }

        if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
            candidates.push((
                home.join(".local").join("bin").join("graphify"),
                "user_local_bin",
            ));

            let python_root = home.join("Library").join("Python");
            let mut python_versions: Vec<PathBuf> = fs::read_dir(&python_root)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect();
            python_versions
                .sort_by(|left, right| right.to_string_lossy().cmp(&left.to_string_lossy()));
            candidates.extend(
                python_versions
                    .into_iter()
                    .map(|version| (version.join("bin").join("graphify"), "user_python_bin")),
            );
        }

        candidates.extend([
            (PathBuf::from("/opt/homebrew/bin/graphify"), "homebrew"),
            (PathBuf::from("/usr/local/bin/graphify"), "usr_local"),
        ]);

        let mut seen = std::collections::HashSet::new();
        for (path, source) in candidates {
            if !seen.insert(path.clone()) || !is_executable(&path) {
                continue;
            }
            return Self {
                available: true,
                path: Some(path.to_string_lossy().into_owned()),
                source: source.to_string(),
            };
        }

        Self::unavailable()
    }

    pub(crate) fn unavailable() -> Self {
        Self {
            available: false,
            path: None,
            source: "not_found".to_string(),
        }
    }
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn unavailable_status_is_explicit() {
        let status = GraphifyStatus::unavailable();
        assert!(!status.available);
        assert_eq!(status.path, None);
        assert_eq!(status.source, "not_found");
    }

    #[test]
    fn detects_an_executable_override() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("graphify");
        fs::write(&executable, "#!/bin/sh\nprintf graphify\n").unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).unwrap();

        let previous = env::var_os("CHADEX_GRAPHIFY_BIN");
        env::set_var("CHADEX_GRAPHIFY_BIN", &executable);
        let status = GraphifyStatus::detect();
        match previous {
            Some(value) => env::set_var("CHADEX_GRAPHIFY_BIN", value),
            None => env::remove_var("CHADEX_GRAPHIFY_BIN"),
        }

        assert!(status.available);
        assert_eq!(
            status.path.as_deref(),
            Some(executable.to_string_lossy().as_ref())
        );
        assert_eq!(status.source, "CHADEX_GRAPHIFY_BIN");
    }
}
