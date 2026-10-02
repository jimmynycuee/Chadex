use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize)]
pub struct DesktopPaths {
    pub helper: PathBuf,
    pub runtime: PathBuf,
    pub data: PathBuf,
}

impl DesktopPaths {
    pub fn resolve(resource: &Path, data: PathBuf) -> Self {
        #[cfg(debug_assertions)]
        if let Some(root) = std::env::var_os("CHADEX_DESKTOP_DEV_RESOURCES") {
            return Self::under(Path::new(&root), data);
        }
        Self::under(resource, data)
    }

    fn under(root: &Path, data: PathBuf) -> Self {
        Self {
            helper: root.join("helper").join(if cfg!(windows) {
                "chadex-helper.exe"
            } else {
                "chadex-helper"
            }),
            runtime: root.join("chadex-runtime"),
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn production_paths_are_rooted_in_resources() {
        let paths = DesktopPaths::under(Path::new("resources"), "data".into());
        assert_eq!(paths.runtime, Path::new("resources/chadex-runtime"));
        assert!(paths.helper.starts_with("resources/helper"));
        assert_eq!(paths.data, Path::new("data"));
    }
}
