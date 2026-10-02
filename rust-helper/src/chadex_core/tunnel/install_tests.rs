use super::*;
use zip::write::SimpleFileOptions;

fn archive(path: &Path, name: &str, bytes: &[u8]) {
    let mut writer = zip::ZipWriter::new(File::create(path).unwrap());
    writer
        .start_file(name, SimpleFileOptions::default())
        .unwrap();
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap();
}

#[test]
fn extraction_and_hash_validation_use_the_named_binary() {
    let fixture = tempfile::tempdir().unwrap();
    let archive_path = fixture.path().join("client.zip");
    let destination = fixture.path().join("tunnel-client.exe");
    archive(&archive_path, "tunnel-client.exe", b"pinned binary bytes");
    let archive_hash = sha256_file(&archive_path).unwrap();
    verify_sha256(&archive_path, &archive_hash).unwrap();
    assert!(verify_sha256(&archive_path, &"0".repeat(64)).is_err());
    extract_tunnel_client(&archive_path, &destination, "tunnel-client.exe").unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"pinned binary bytes");
    let expected = format!("{:x}", Sha256::digest(b"pinned binary bytes"));
    verify_sha256(&destination, &expected).unwrap();
    fs::write(&destination, b"corrupted binary bytes").unwrap();
    assert!(verify_sha256(&destination, &expected).is_err());
}

#[test]
fn extraction_rejects_missing_invalid_and_oversized_members_without_writing() {
    let fixture = tempfile::tempdir().unwrap();
    let archive_path = fixture.path().join("client.zip");
    let destination = fixture.path().join("tunnel-client.exe");
    fs::write(&archive_path, b"not a zip").unwrap();
    assert!(extract_tunnel_client(&archive_path, &destination, "tunnel-client.exe").is_err());
    archive(&archive_path, "../tunnel-client.exe", b"wrong member");
    assert!(extract_tunnel_client(&archive_path, &destination, "tunnel-client.exe").is_err());
    archive(&archive_path, "tunnel-client.exe", &[]);
    // Inflate only the central-directory declared length; no 64 MiB fixture needed.
    let mut bytes = fs::read(&archive_path).unwrap();
    let central = bytes.windows(4).position(|w| w == b"PK\x01\x02").unwrap();
    bytes[central + 24..central + 28]
        .copy_from_slice(&((MAX_BINARY_BYTES + 1) as u32).to_le_bytes());
    fs::write(&archive_path, bytes).unwrap();
    assert!(extract_tunnel_client(&archive_path, &destination, "tunnel-client.exe").is_err());
    assert!(!destination.exists());
}

#[test]
fn extraction_and_private_write_refuse_existing_file() {
    let fixture = tempfile::tempdir().unwrap();
    let archive_path = fixture.path().join("client.zip");
    let destination = fixture.path().join("tunnel-client.exe");
    archive(&archive_path, "tunnel-client.exe", b"new bytes");
    write_private_file(&destination, b"existing bytes").unwrap();
    assert!(extract_tunnel_client(&archive_path, &destination, "tunnel-client.exe").is_err());
    assert!(write_private_file(&destination, b"overwrite").is_err());
    assert_eq!(fs::read(&destination).unwrap(), b"existing bytes");
}

#[test]
fn verified_candidate_replaces_corrupt_cache_and_missing_candidate_preserves_it() {
    let fixture = tempfile::tempdir().unwrap();
    let candidate = fixture.path().join("candidate.exe");
    let destination = fixture.path().join("tunnel-client.exe");
    write_private_file(&candidate, b"verified candidate").unwrap();
    write_private_file(&destination, b"corrupt cache").unwrap();
    let expected = sha256_file(&candidate).unwrap();
    verify_sha256(&candidate, &expected).unwrap();
    let identity = tunnel_client_file_identity(&destination).unwrap();
    install_verified_tunnel_client(&candidate, &destination).unwrap();
    assert!(!candidate.exists());
    verify_sha256(&destination, &expected).unwrap();
    assert_ne!(tunnel_client_file_identity(&destination).unwrap(), identity);
    assert!(install_verified_tunnel_client(&candidate, &destination).is_err());
    verify_sha256(&destination, &expected).unwrap();
}

#[test]
fn version_must_match_the_pinned_token_exactly() {
    for version in ["0.0.12\n", "0.0.12 (commit abc)\r\n"] {
        assert!(pinned_tunnel_version(version), "{version:?}");
    }
    for version in ["", "0.0.120", "0.0.12-evil", "0.0.13", "error: 0.0.12"] {
        assert!(!pinned_tunnel_version(version), "{version:?}");
    }
}

#[tokio::test]
async fn version_launch_rejects_non_executable_and_missing_binary() {
    let fixture = tempfile::tempdir().unwrap();
    let candidate = fixture.path().join(tunnel_client_binary_name());
    assert!(verify_tunnel_client(&candidate).await.is_err());
    write_private_file(&candidate, b"not an executable").unwrap();
    make_private_executable(&candidate).unwrap();
    assert!(verify_tunnel_client(&candidate).await.is_err());
}

#[cfg(unix)]
#[test]
fn private_install_files_retain_unix_permissions_and_reject_symlink_identity() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let fixture = tempfile::tempdir().unwrap();
    let directory = fixture.path().join("private");
    create_private_dir(&directory).unwrap();
    assert_eq!(
        fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let binary = directory.join("client");
    write_private_file(&binary, b"binary").unwrap();
    assert_eq!(
        fs::metadata(&binary).unwrap().permissions().mode() & 0o777,
        0o600
    );
    make_private_executable(&binary).unwrap();
    assert_eq!(
        fs::metadata(&binary).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let link = directory.join("link");
    symlink(&binary, &link).unwrap();
    assert!(tunnel_client_file_identity(&link).is_err());
}
