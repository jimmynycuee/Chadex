use crate::chadex_core::runtime_compat::error::{DesktopError, DesktopResult};
use crate::chadex_core::runtime_compat::models::{OpenAiTunnelConfigSnapshot, TunnelConfigSource};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::process::Command;

const MAX_CONFIG_BYTES: u64 = 16 * 1024;
const MAX_TUNNEL_ID_BYTES: usize = 256;
const MAX_API_KEY_BYTES: usize = 8192;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedCredentials {
    tunnel_id: String,
    api_key: String,
}

#[derive(Clone)]
enum CredentialMode {
    Environment,
    Saved(SavedCredentials),
    MemoryOnly,
    Invalid,
}

#[derive(Clone)]
pub(crate) struct TunnelConfig {
    mode: CredentialMode,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            mode: CredentialMode::Environment,
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum TunnelConfigRequest {
    Save {
        #[serde(rename = "tunnelId")]
        tunnel_id: String,
        #[serde(rename = "apiKey")]
        api_key: Option<String>,
    },
    UseEnvironment,
}

impl TunnelConfig {
    pub fn load(path: &Path) -> Self {
        match read_saved_credentials(path) {
            Ok(Some(credentials)) => Self {
                mode: CredentialMode::Saved(credentials),
            },
            Ok(None) => Self::default(),
            Err(_) => Self {
                mode: CredentialMode::Invalid,
            },
        }
    }

    pub fn in_memory() -> Self {
        Self {
            mode: CredentialMode::MemoryOnly,
        }
    }

    pub fn update(&mut self, path: &Path, request: TunnelConfigRequest) -> DesktopResult<()> {
        let next = match request {
            TunnelConfigRequest::UseEnvironment => CredentialMode::Environment,
            TunnelConfigRequest::Save { tunnel_id, api_key } => {
                let tunnel_id = tunnel_id.trim().to_string();
                let api_key = api_key
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| value.trim().to_string())
                    .or_else(|| match &self.mode {
                        CredentialMode::Saved(saved) => Some(saved.api_key.clone()),
                        _ => None,
                    })
                    .ok_or_else(config_error)?;

                validate_credentials(&tunnel_id, &api_key)?;
                CredentialMode::Saved(SavedCredentials { tunnel_id, api_key })
            }
        };

        let serializable = match &next {
            CredentialMode::Environment => None,
            CredentialMode::Saved(saved) => Some(saved),
            CredentialMode::MemoryOnly | CredentialMode::Invalid => return Err(config_error()),
        };
        let bytes = serde_json::to_vec_pretty(&serializable).map_err(|_| config_error())?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(config_error());
        }

        crate::chadex_core::runtime_compat::state::write_atomic_file(path, &bytes).map_err(|_| {
            DesktopError::new(
                "tunnel_config_save_failed",
                "Could not save the Tunnel configuration",
                "Check access to the Desktop application data directory and retry.",
            )
        })?;
        self.mode = next;
        Ok(())
    }

    pub fn snapshot(&self) -> OpenAiTunnelConfigSnapshot {
        match &self.mode {
            CredentialMode::Environment => environment_snapshot(),
            CredentialMode::Saved(saved) => OpenAiTunnelConfigSnapshot {
                tunnel_id_present: true,
                api_key_present: true,
                source: TunnelConfigSource::File,
                saved_tunnel_id: Some(saved.tunnel_id.clone()),
                effective_tunnel_id: Some(saved.tunnel_id.clone()),
            },
            CredentialMode::MemoryOnly => OpenAiTunnelConfigSnapshot {
                source: TunnelConfigSource::Memory,
                ..Default::default()
            },
            CredentialMode::Invalid => OpenAiTunnelConfigSnapshot {
                source: TunnelConfigSource::Invalid,
                ..Default::default()
            },
        }
    }

    pub fn apply_to_command(&self, command: &mut Command) -> DesktopResult<()> {
        match &self.mode {
            CredentialMode::Environment => {}
            CredentialMode::Saved(saved) => {
                command
                    .env("CONTROL_PLANE_TUNNEL_ID", &saved.tunnel_id)
                    .env("CONTROL_PLANE_API_KEY", &saved.api_key);
            }
            CredentialMode::MemoryOnly => {
                command
                    .env_remove("CONTROL_PLANE_TUNNEL_ID")
                    .env_remove("CONTROL_PLANE_API_KEY");
            }
            CredentialMode::Invalid => return Err(config_error()),
        }
        Ok(())
    }
}

pub(crate) fn environment_snapshot() -> OpenAiTunnelConfigSnapshot {
    let tunnel_id = std::env::var("CONTROL_PLANE_TUNNEL_ID")
        .ok()
        .filter(|value| valid_tunnel_id(value));

    OpenAiTunnelConfigSnapshot {
        tunnel_id_present: std::env::var_os("CONTROL_PLANE_TUNNEL_ID")
            .is_some_and(|value| !value.is_empty()),
        api_key_present: std::env::var_os("CONTROL_PLANE_API_KEY")
            .is_some_and(|value| !value.is_empty()),
        source: TunnelConfigSource::Environment,
        saved_tunnel_id: None,
        effective_tunnel_id: tunnel_id,
    }
}

fn read_saved_credentials(path: &Path) -> DesktopResult<Option<SavedCredentials>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(config_error()),
    };

    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(config_error());
    }

    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|_| config_error())?
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| config_error())?;
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(config_error());
    }

    let saved: Option<SavedCredentials> =
        serde_json::from_slice(&bytes).map_err(|_| config_error())?;
    if let Some(credentials) = &saved {
        validate_credentials(&credentials.tunnel_id, &credentials.api_key)?;
    }
    Ok(saved)
}

fn validate_credentials(tunnel_id: &str, api_key: &str) -> DesktopResult<()> {
    if !valid_tunnel_id(tunnel_id)
        || api_key.is_empty()
        || api_key.len() > MAX_API_KEY_BYTES
        || !api_key.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(config_error());
    }
    Ok(())
}

fn valid_tunnel_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TUNNEL_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn config_error() -> DesktopError {
    DesktopError::new(
        "tunnel_config_invalid",
        "Tunnel configuration is missing or invalid",
        "Enter a valid Tunnel ID and API key, then save again. Invalid saved configuration never falls back to environment credentials.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_path(root: &Path) -> std::path::PathBuf {
        let secrets = root.join("secrets");
        std::fs::create_dir_all(&secrets).unwrap();
        secrets.join("tunnel-config.json")
    }

    fn save(id: &str, key: Option<&str>) -> TunnelConfigRequest {
        TunnelConfigRequest::Save {
            tunnel_id: id.to_string(),
            api_key: key.map(str::to_string),
        }
    }

    #[test]
    fn memory_mode_is_inert_and_never_exposes_credentials() {
        let config = TunnelConfig::in_memory();
        let snapshot = config.snapshot();
        assert_eq!(snapshot.source, TunnelConfigSource::Memory);
        assert!(!snapshot.is_configured());
        assert!(snapshot.effective_tunnel_id.is_none());

        let mut command = Command::new("/usr/bin/true");
        command
            .env("CONTROL_PLANE_TUNNEL_ID", "inherited")
            .env("CONTROL_PLANE_API_KEY", "inherited");
        config.apply_to_command(&mut command).unwrap();
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(env[std::ffi::OsStr::new("CONTROL_PLANE_TUNNEL_ID")], None);
        assert_eq!(env[std::ffi::OsStr::new("CONTROL_PLANE_API_KEY")], None);
    }

    #[test]
    fn validation_rejects_unsafe_values() {
        for id in ["", "spaces are bad", "bad/slash"] {
            assert!(validate_credentials(id, "key").is_err());
        }
        assert!(validate_credentials("valid_id-1", "").is_err());
        assert!(validate_credentials("valid_id-1", "line\nbreak").is_err());
        assert!(validate_credentials("valid_id-1", "key").is_ok());
    }

    #[test]
    fn missing_file_means_environment_mode() {
        let temp = tempfile::tempdir().unwrap();
        let config = TunnelConfig::load(&temp.path().join("missing.json"));
        assert!(matches!(config.mode, CredentialMode::Environment));
    }

    #[test]
    fn saved_credentials_roundtrip_without_projecting_the_key() {
        let temp = tempfile::tempdir().unwrap();
        let path = config_path(temp.path());
        let mut config = TunnelConfig::default();
        config
            .update(&path, save("tunnel_saved", Some("test-only-api-key")))
            .unwrap();

        let mut loaded = TunnelConfig::load(&path);
        let snapshot = loaded.snapshot();
        assert_eq!(snapshot.source, TunnelConfigSource::File);
        assert_eq!(
            snapshot.effective_tunnel_id.as_deref(),
            Some("tunnel_saved")
        );
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("test-only-api-key"));

        let mut command = Command::new("/usr/bin/true");
        loaded.apply_to_command(&mut command).unwrap();
        let env: std::collections::HashMap<_, _> = command.get_envs().collect();
        assert_eq!(
            env[std::ffi::OsStr::new("CONTROL_PLANE_TUNNEL_ID")].unwrap(),
            "tunnel_saved"
        );
        assert_eq!(
            env[std::ffi::OsStr::new("CONTROL_PLANE_API_KEY")].unwrap(),
            "test-only-api-key"
        );

        loaded.update(&path, save("tunnel_changed", None)).unwrap();
        let changed = TunnelConfig::load(&path);
        match changed.mode {
            CredentialMode::Saved(saved) => {
                assert_eq!(saved.tunnel_id, "tunnel_changed");
                assert_eq!(saved.api_key, "test-only-api-key");
            }
            _ => panic!("expected saved credentials"),
        }

        loaded
            .update(&path, TunnelConfigRequest::UseEnvironment)
            .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "null");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1,
            "retired credentials must not leave a backup file"
        );
    }

    #[test]
    fn invalid_saved_data_fails_closed_and_can_be_repaired() {
        let temp = tempfile::tempdir().unwrap();
        let path = config_path(temp.path());
        std::fs::write(&path, b"not-json-with-test-secret").unwrap();

        let mut config = TunnelConfig::load(&path);
        assert_eq!(config.snapshot().source, TunnelConfigSource::Invalid);
        let error = config
            .apply_to_command(&mut Command::new("/usr/bin/true"))
            .unwrap_err();
        assert!(!error.to_string().contains("test-secret"));

        config
            .update(&path, save("tunnel_fixed", Some("test-key")))
            .unwrap();
        assert!(TunnelConfig::load(&path).snapshot().is_configured());

        std::fs::write(&path, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        assert_eq!(
            TunnelConfig::load(&path).snapshot().source,
            TunnelConfigSource::Invalid
        );
    }

    #[test]
    fn failed_updates_keep_the_previous_saved_credentials() {
        let temp = tempfile::tempdir().unwrap();
        let path = config_path(temp.path());
        let mut config = TunnelConfig::default();
        config
            .update(&path, save("tunnel_one", Some("old-test-key")))
            .unwrap();

        assert!(config
            .update(&path, save("invalid/id", Some("key")))
            .is_err());
        assert!(config
            .update(&path, save("tunnel_one", Some("invalid\nkey")))
            .is_err());

        let blocked = temp.path().join("directory");
        std::fs::create_dir(&blocked).unwrap();
        assert!(config
            .update(&blocked, save("tunnel_two", Some("new-test-key")))
            .is_err());

        assert_eq!(
            config.snapshot().saved_tunnel_id.as_deref(),
            Some("tunnel_one")
        );
        match TunnelConfig::load(&path).mode {
            CredentialMode::Saved(saved) => assert_eq!(saved.api_key, "old-test-key"),
            _ => panic!("expected saved credentials"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn credential_file_is_private_and_symlinks_are_rejected() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let path = config_path(temp.path());
        let mut config = TunnelConfig::default();
        config
            .update(&path, save("tunnel_one", Some("test-key")))
            .unwrap();

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let link = temp.path().join("link.json");
        symlink(&path, &link).unwrap();
        assert_eq!(
            TunnelConfig::load(&link).snapshot().source,
            TunnelConfigSource::Invalid
        );
    }

    #[tokio::test]
    async fn chadex_backend_uses_memory_only_tunnel_configuration() {
        let temp = tempfile::tempdir().unwrap();
        let app = crate::chadex_core::runtime_compat::state::RuntimeStateManager::new_for_chadex_backend(
            temp.path().to_path_buf(),
            temp.path().join("resources"),
        )
        .unwrap();
        let snapshot = app.get_state();

        assert_eq!(
            snapshot.openai_tunnel_config.source,
            TunnelConfigSource::Memory
        );
        assert!(!snapshot.openai_tunnel_configured);
        assert!(!snapshot.openai_tunnel_config.api_key_present);
        assert!(!snapshot.openai_tunnel_config.tunnel_id_present);
        assert!(snapshot.openai_tunnel_config.effective_tunnel_id.is_none());
        app.shutdown().await;
    }
}
