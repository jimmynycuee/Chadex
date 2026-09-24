use super::settings::DESKTOP_STATE_MAX_BYTES;
use crate::chadex_core::runtime_compat::activity::{ActivityEventKind, ActivityLevel, ActivityLog};
use crate::chadex_core::runtime_compat::error::{DesktopError, DesktopResult};
use crate::chadex_core::runtime_compat::models::StoredDesktopConfig;
#[cfg(unix)]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_STATE_TEMP_ID: AtomicU64 = AtomicU64::new(1);
#[derive(Debug)]
pub(super) enum StoredConfigFile {
    Missing,
    Valid {
        config: StoredDesktopConfig,
        bytes: Vec<u8>,
    },
    Corrupt,
}

pub(super) fn load_config(
    path: &Path,
    activity: &ActivityLog,
) -> DesktopResult<StoredDesktopConfig> {
    let backup_path = desktop_state_backup_path(path);
    match read_stored_config(path)? {
        StoredConfigFile::Valid { config, .. } => Ok(config),
        StoredConfigFile::Missing => match read_stored_config(&backup_path)? {
            StoredConfigFile::Missing => Ok(StoredDesktopConfig::default()),
            StoredConfigFile::Valid { config, bytes } => {
                recover_config_from_backup(path, &bytes, activity)?;
                Ok(config)
            }
            StoredConfigFile::Corrupt => Err(desktop_state_corrupt()),
        },
        StoredConfigFile::Corrupt => match read_stored_config(&backup_path)? {
            StoredConfigFile::Valid { config, bytes } => {
                recover_config_from_backup(path, &bytes, activity)?;
                Ok(config)
            }
            StoredConfigFile::Missing | StoredConfigFile::Corrupt => Err(desktop_state_corrupt()),
        },
    }
}

pub(super) fn recover_config_from_backup(
    primary_path: &Path,
    bytes: &[u8],
    activity: &ActivityLog,
) -> DesktopResult<()> {
    write_atomic_file(primary_path, bytes).map_err(|error| {
        desktop_state_unavailable("Desktop could not restore the previous known-good state")
            .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
    })?;
    activity.push(
        ActivityEventKind::StateRecovered,
        "desktop_state",
        ActivityLevel::Warning,
        "Recovered Desktop state from the previous known-good snapshot",
    );
    Ok(())
}

pub(super) fn read_stored_config(path: &Path) -> DesktopResult<StoredConfigFile> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(StoredConfigFile::Missing)
        }
        Err(error) => {
            return Err(
                desktop_state_unavailable("Desktop could not inspect its saved state")
                    .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) })),
            )
        }
    };
    if !metadata.is_file() || metadata.len() > DESKTOP_STATE_MAX_BYTES {
        return Ok(StoredConfigFile::Corrupt);
    }
    let bytes = std::fs::read(path).map_err(|error| {
        desktop_state_unavailable("Desktop could not read its saved state")
            .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
    })?;
    match serde_json::from_slice::<StoredDesktopConfig>(&bytes) {
        Ok(config) => Ok(StoredConfigFile::Valid { config, bytes }),
        Err(_) => Ok(StoredConfigFile::Corrupt),
    }
}

pub(super) fn save_config_atomically(path: &Path, encoded: &[u8]) -> DesktopResult<()> {
    if encoded.len() as u64 > DESKTOP_STATE_MAX_BYTES {
        return Err(DesktopError::new(
            "desktop_state_invalid",
            "Desktop state exceeded its bounded persistence size",
            "Retry after reducing the saved Desktop configuration.",
        ));
    }

    if let StoredConfigFile::Valid { bytes, .. } = read_stored_config(path)? {
        let backup = desktop_state_backup_path(path);
        write_atomic_file(&backup, &bytes).map_err(|error| {
            desktop_state_unavailable("Desktop could not preserve the previous known-good state")
                .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
        })?;
    }

    write_atomic_file(path, encoded).map_err(|error| {
        desktop_state_unavailable("Desktop could not persist its non-secret runtime state")
            .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
    })
}

pub(super) fn desktop_state_backup_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("desktop-state.json");
    path.with_file_name(format!("{file_name}.bak"))
}

pub(super) fn state_temp_path(path: &Path) -> PathBuf {
    let id = NEXT_STATE_TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("desktop-state.json");
    path.with_file_name(format!(".{file_name}.{}.{}.tmp", std::process::id(), id))
}

pub(crate) fn write_atomic_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_atomic_file_with_hook(path, bytes, |_| Ok(()))
}

pub(crate) fn write_atomic_file_with_hook<F>(
    path: &Path,
    bytes: &[u8],
    before_replace: F,
) -> io::Result<()>
where
    F: FnOnce(&Path) -> io::Result<()>,
{
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temp_path = state_temp_path(path);
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp_path)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        before_replace(&temp_path)?;
        atomic_replace(&temp_path, path)?;
        sync_state_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

#[cfg(not(windows))]
pub(super) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)
}

#[cfg(windows)]
pub(super) fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let result = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
pub(super) fn sync_state_directory(path: &Path) -> io::Result<()> {
    let directory = File::open(path)?;
    match directory.sync_all() {
        Ok(()) => Ok(()),
        Err(error)
            if error.kind() == io::ErrorKind::Unsupported
                || error.raw_os_error() == Some(libc::EINVAL) =>
        {
            // Some Unix filesystems (notably macOS variants) do not support
            // directory fsync. The file itself has already been synced and the
            // same-directory rename is atomic, so treat this specific platform
            // limitation as best-effort durability rather than a false save
            // failure after replacement has already succeeded.
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
pub(super) fn sync_state_directory(_path: &Path) -> io::Result<()> {
    // Windows uses MOVEFILE_WRITE_THROUGH for the replacement. Opening a
    // directory for FlushFileBuffers would require broader sharing semantics
    // than the app-data policy needs here.
    Ok(())
}

pub(super) fn desktop_state_corrupt() -> DesktopError {
    DesktopError::new(
        "desktop_state_corrupt",
        "Desktop saved state is corrupt and no valid recovery snapshot is available",
        "Restore or remove the Desktop state files explicitly, then restart WebCodex Desktop.",
    )
    .with_details(serde_json::json!({ "category": "state_corrupt" }))
}

pub(super) fn desktop_state_unavailable(message: &'static str) -> DesktopError {
    DesktopError::new(
        "desktop_state_unavailable",
        message,
        "Check local app-data permissions and retry.",
    )
}
