use std::{fs, path::Path};

pub const FILE_NAME: &str = "global-instructions.md";
pub const MAX_BYTES: usize = 8 * 1024;

fn existing_metadata(path: &Path) -> Result<Option<fs::Metadata>, String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("global_instructions_read_failed".into()),
    }
}

fn validate_existing_file(metadata: &fs::Metadata) -> Result<(), String> {
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err("global_instructions_invalid_file".into());
    }
    if metadata.len() > MAX_BYTES as u64 {
        return Err("global_instructions_too_large".into());
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<String, String> {
    let Some(metadata) = existing_metadata(path)? else {
        return Ok(String::new());
    };
    validate_existing_file(&metadata)?;
    let bytes = fs::read(path).map_err(|_| "global_instructions_read_failed")?;
    if bytes.len() > MAX_BYTES {
        return Err("global_instructions_too_large".into());
    }
    String::from_utf8(bytes).map_err(|_| "global_instructions_invalid_utf8".into())
}

pub fn save(path: &Path, value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    if bytes.len() > MAX_BYTES {
        return Err("global_instructions_too_large".into());
    }
    if let Some(metadata) = existing_metadata(path)? {
        validate_existing_file(&metadata)?;
    }
    let parent = path.parent().ok_or("global_instructions_write_failed")?;
    fs::create_dir_all(parent).map_err(|_| "global_instructions_write_failed")?;
    let staged = path.with_extension("staged");
    let backup = path.with_extension("previous");
    if staged.exists() {
        fs::remove_file(&staged).map_err(|_| "global_instructions_write_failed")?;
    }
    fs::write(&staged, bytes).map_err(|_| "global_instructions_write_failed")?;
    if backup.exists() {
        fs::remove_file(&backup).map_err(|_| "global_instructions_backup_failed")?;
    }
    if path.exists() {
        fs::rename(path, &backup).map_err(|_| "global_instructions_backup_failed")?;
    }
    if fs::rename(&staged, path).is_err() {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        let _ = fs::remove_file(&staged);
        return Err("global_instructions_replace_failed".into());
    }
    if backup.exists() {
        let _ = fs::remove_file(backup);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_blank_and_unicode_values_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        assert_eq!(load(&path).unwrap(), "");
        save(&path, "").unwrap();
        assert_eq!(load(&path).unwrap(), "");
        save(&path, "# Global\n所有 Chadex 專案共用。\n").unwrap();
        assert_eq!(load(&path).unwrap(), "# Global\n所有 Chadex 專案共用。\n");
    }

    #[test]
    fn oversized_values_fail_without_replacing_current_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(FILE_NAME);
        save(&path, "stable").unwrap();
        assert_eq!(
            save(&path, &"x".repeat(MAX_BYTES + 1)),
            Err("global_instructions_too_large".into())
        );
        assert_eq!(load(&path).unwrap(), "stable");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_storage_is_rejected() {
        use std::os::unix::fs::symlink;
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real.md");
        let path = directory.path().join(FILE_NAME);
        fs::write(&real, "ambient").unwrap();
        symlink(&real, &path).unwrap();
        assert_eq!(load(&path), Err("global_instructions_invalid_file".into()));
        assert_eq!(
            save(&path, "replacement"),
            Err("global_instructions_invalid_file".into())
        );
        assert_eq!(fs::read_to_string(real).unwrap(), "ambient");
    }
}
