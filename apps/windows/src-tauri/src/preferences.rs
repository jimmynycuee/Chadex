use serde::{Deserialize, Serialize};
use std::{fs, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub restore_project: bool,
    pub launch_at_login: bool,
    pub notifications: bool,
    pub ferret_visible: bool,
    pub ferret_motion: bool,
    pub theme: String,
    pub recent_projects: Vec<String>,
    pub last_project: Option<String>,
    pub tunnel_id: String,
    pub prepare_service_on_launch: bool,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            restore_project: true,
            launch_at_login: false,
            notifications: true,
            ferret_visible: true,
            ferret_motion: true,
            // Dark is the designed default; "system" and "light" stay available.
            theme: "dark".into(),
            recent_projects: Vec::new(),
            last_project: None,
            tunnel_id: String::new(),
            prepare_service_on_launch: true,
        }
    }
}

impl Preferences {
    pub fn load(path: &Path) -> Result<Self, String> {
        // Recover the narrow crash window between moving the old copy aside
        // and installing the staged copy. Never guess an incomplete JSON file.
        if !path.exists() && path.with_extension("previous").is_file() {
            fs::rename(path.with_extension("previous"), path)
                .map_err(|_| "preferences_restore_failed")?;
        }
        match fs::read(path) {
            Ok(bytes) if bytes.len() <= 64 * 1024 => serde_json::from_slice(&bytes)
                .map_err(|_| "preferences_invalid: 設定檔無法讀取，請透過診斷檢查。".into()),
            Ok(_) => Err("preferences_too_large".into()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err("preferences_read_failed".into()),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !["system", "dark", "light"].contains(&self.theme.as_str())
            || self.recent_projects.len() > 20
            || self.tunnel_id.len() > 200
            || self
                .tunnel_id
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
            || self.recent_projects.iter().any(|p| p.len() > 8192)
            || self.last_project.as_ref().is_some_and(|p| p.len() > 8192)
        {
            return Err("preferences_invalid".into());
        }
        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| "preferences_encode_failed")?;
        let staged = path.with_extension("staged");
        fs::write(&staged, bytes).map_err(|_| "preferences_write_failed")?;
        // Windows rename cannot replace an existing file. Move the current copy
        // aside and restore it on failure, rather than truncating a live config.
        let backup = path.with_extension("previous");
        if backup.exists() {
            fs::remove_file(&backup).map_err(|_| "preferences_backup_failed")?;
        }
        if path.exists() {
            fs::rename(path, &backup).map_err(|_| "preferences_backup_failed")?;
        }
        if fs::rename(&staged, path).is_err() {
            if backup.exists() {
                let _ = fs::rename(&backup, path);
            }
            return Err("preferences_replace_failed".into());
        }
        if backup.exists() {
            let _ = fs::remove_file(backup);
        }
        Ok(())
    }

    pub fn selected(&mut self, path: String) {
        self.recent_projects.retain(|p| p != &path);
        self.recent_projects.insert(0, path.clone());
        self.recent_projects.truncate(20);
        self.last_project = Some(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prewarm_setting_defaults_on_for_older_preference_files() {
        let old: Preferences = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert!(old.prepare_service_on_launch);
        assert!(Preferences::default().prepare_service_on_launch);
    }
    #[test]
    fn new_installs_default_to_dark_but_keep_a_saved_choice() {
        assert_eq!(Preferences::default().theme, "dark");
        let saved: Preferences = serde_json::from_str(r#"{"theme":"system"}"#).unwrap();
        assert_eq!(saved.theme, "system");
    }
    #[test]
    fn credentials_cannot_enter_preferences() {
        assert!(serde_json::from_str::<Preferences>(r#"{"api_key":"value"}"#).is_err());
        let json = serde_json::to_string(&Preferences::default()).unwrap();
        assert!(!json.contains("credential"));
    }
    #[test]
    fn selection_is_bounded_and_deduplicated() {
        let mut prefs = Preferences::default();
        for n in 0..25 {
            prefs.selected(format!("C:/project/{n}"));
        }
        prefs.selected("C:/project/20".into());
        assert_eq!(prefs.recent_projects.len(), 20);
        assert_eq!(prefs.recent_projects[0], "C:/project/20");
        assert_eq!(
            prefs
                .recent_projects
                .iter()
                .filter(|p| *p == "C:/project/20")
                .count(),
            1
        );
    }
    #[test]
    fn preferences_survive_restart_and_interrupted_replace() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("desktop-preferences.json");
        let mut prefs = Preferences::default();
        prefs.selected("C:/專案 with spaces".into());
        prefs.ferret_visible = false;
        prefs.save(&path).unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), prefs);
        fs::rename(&path, path.with_extension("previous")).unwrap();
        fs::write(path.with_extension("staged"), b"incomplete").unwrap();
        assert_eq!(Preferences::load(&path).unwrap(), prefs);
    }
}
