//! Native NTFS/Win32 tests. Junctions do not require Developer Mode or elevation.
//! The real-asset test is a separate, explicitly selected network gate:
//! cargo test --locked --manifest-path rust-helper/Cargo.toml \
//!   chadex_core::tunnel::windows_tests::windows_official_asset_install_reuse_replacement_version \
//!   -- --ignored --exact
//! No credentials or environment switches are required. CI must check LASTEXITCODE.

use super::*;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::{
    GetAce, GetSecurityDescriptorControl, ACCESS_ALLOWED_ACE, DACL_SECURITY_INFORMATION,
    SE_DACL_PROTECTED,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
};

fn sid_text(sid: windows_sys::Win32::Security::PSID) -> String {
    let mut text = std::ptr::null_mut();
    assert_ne!(unsafe { ConvertSidToStringSidW(sid, &mut text) }, 0);
    unsafe {
        let mut length = 0;
        while *text.add(length) != 0 {
            length += 1;
        }
        let result = String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap();
        LocalFree(text as _);
        result
    }
}

fn assert_private_dacl(path: &Path, directory: bool) {
    let file = fs::OpenOptions::new()
        .access_mode(0x0002_0000) // READ_CONTROL
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
        .unwrap();
    let mut descriptor = std::ptr::null_mut();
    let mut dacl = std::ptr::null_mut();
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                file.as_raw_handle() as _,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut dacl,
                std::ptr::null_mut(),
                &mut descriptor,
            )
        },
        0
    );
    // Copy ACE values before freeing Windows' allocated descriptor.
    let mut control = 0;
    let mut revision = 0;
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) },
        0
    );
    assert!(!dacl.is_null(), "a NULL DACL grants everyone access");
    let mut aces = Vec::new();
    unsafe {
        for index in 0..(*dacl).AceCount {
            let mut ace = std::ptr::null_mut();
            assert_ne!(GetAce(dacl, index as u32, &mut ace), 0);
            let ace = &*(ace.cast::<ACCESS_ALLOWED_ACE>());
            aces.push((
                ace.Header.AceType,
                ace.Header.AceFlags,
                ace.Mask,
                sid_text((&ace.SidStart as *const u32).cast_mut().cast()),
            ));
        }
        LocalFree(descriptor as _);
    }
    assert_ne!(
        control & SE_DACL_PROTECTED,
        0,
        "DACL must disable inheritance"
    );
    assert_eq!(
        aces.len(),
        2,
        "only current user and SYSTEM may have access"
    );
    let current_user = windows_support::current_user_sid().unwrap();
    let mut sids = Vec::new();
    for (kind, flags, mask, sid) in aces {
        assert_eq!(kind, 0, "must be ACCESS_ALLOWED_ACE_TYPE");
        assert_eq!(
            flags,
            if directory { 3 } else { 0 },
            "only directories use OI|CI; no inherited ACEs"
        );
        assert_eq!(mask, FILE_ALL_ACCESS);
        sids.push(sid);
    }
    sids.sort();
    let mut expected = vec![current_user, "S-1-5-18".to_string()];
    expected.sort();
    assert_eq!(sids, expected);
}

#[test]
fn windows_private_directory_file_and_session_have_protected_dacl() {
    let fixture = tempfile::tempdir().unwrap();
    let directory = fixture.path().join("private 空間");
    create_private_dir(&directory).unwrap();
    assert_private_dacl(&directory, true);
    create_private_dir(&directory).unwrap(); // Protection of existing directories.
    assert_private_dacl(&directory, true);
    let file = directory.join("private-file");
    write_private_file(&file, b"local test bytes").unwrap();
    assert_private_dacl(&file, false);
    assert!(write_private_file(&file, b"overwrite").is_err());
    assert_eq!(fs::read(&file).unwrap(), b"local test bytes");
    let existing = directory.join("existing-file");
    fs::write(&existing, b"existing inherited file").unwrap();
    windows_support::protect_private_file(&existing).unwrap();
    assert_private_dacl(&existing, false);
    let session = TunnelSession::create(fixture.path(), "synthetic-test-token").unwrap();
    assert_private_dacl(&session.directory, true);
    assert_private_dacl(&session.authorization_file, false);
    let session_dir = session.directory.clone();
    drop(session);
    assert!(!session_dir.exists());
}

#[test]
fn windows_runner_config_and_backup_carry_the_private_dacl() {
    use crate::chadex_core::external_skills::{
        persist_if_unchanged, restore_if_unchanged, runner_config_backup_path,
    };
    let fixture = tempfile::tempdir().unwrap();
    let config = fixture.path().join("runner.toml");
    // The original file inherits the (open) parent ACL, as a hand-made one would.
    fs::write(&config, "client_id = \"c\"\n").unwrap();
    persist_if_unchanged(&config, "client_id = \"c\"\n", "client_id = \"d\"\n").unwrap();
    assert_eq!(fs::read_to_string(&config).unwrap(), "client_id = \"d\"\n");
    assert_private_dacl(&config, false);
    let backup = runner_config_backup_path(&config);
    assert_eq!(fs::read_to_string(&backup).unwrap(), "client_id = \"c\"\n");
    assert_private_dacl(&backup, false);
    // Rollback replaces runner.toml again and keeps it private.
    assert!(restore_if_unchanged(&config, "client_id = \"d\"\n", "client_id = \"c\"\n").unwrap());
    assert_eq!(fs::read_to_string(&config).unwrap(), "client_id = \"c\"\n");
    assert_private_dacl(&config, false);
    let leftovers = fs::read_dir(fixture.path())
        .unwrap()
        .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(leftovers, 0);
}

#[test]
fn windows_runner_config_is_not_written_through_a_junction_directory() {
    use crate::chadex_core::external_skills::persist_if_unchanged;
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("real");
    fs::create_dir(&target).unwrap();
    let junction = Junction::new(&fixture.path().join("linked"), &target);
    let config = junction.0.join("runner.toml");
    // runner.toml cannot be read through the link, so nothing may be written.
    fs::write(target.join("runner.toml"), "a = 1\n").unwrap();
    let before = fs::read_dir(&target).unwrap().count();
    assert_eq!(
        persist_if_unchanged(&config, "a = 1\n", "a = 2\n"),
        Err("runner_config_path_is_link")
    );
    assert_eq!(fs::read_to_string(target.join("runner.toml")).unwrap(), "a = 1\n");
    assert_eq!(fs::read_dir(&target).unwrap().count(), before);
}

struct Junction(PathBuf);

impl Junction {
    fn new(link: &Path, target: &Path) -> Self {
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "junction setup failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        Self(link.to_owned())
    }
}

impl Drop for Junction {
    fn drop(&mut self) {
        // Remove the junction itself before TempDir cleanup can traverse it.
        fs::remove_dir(&self.0).expect("remove test junction");
    }
}

#[tokio::test]
async fn windows_reparse_directory_and_ancestor_are_rejected_without_target_mutation() {
    let fixture = tempfile::tempdir().unwrap();
    let target = fixture.path().join("target 空間");
    fs::create_dir(&target).unwrap();
    let binary = target.join("tunnel-client.exe");
    fs::write(&binary, b"untouched").unwrap();
    let _junction = Junction::new(&fixture.path().join("junction 空間"), &target);
    let link = &_junction.0;
    assert!(windows_support::protect_private_directory(link)
        .unwrap_err()
        .contains("reparse"));
    assert!(create_private_dir(&link.join("must-not-create")).is_err());
    assert!(!target.join("must-not-create").exists());
    assert!(write_private_file(&link.join("must-not-write"), b"no").is_err());
    assert!(!target.join("must-not-write").exists());
    assert!(tunnel_client_file_identity(&link.join("tunnel-client.exe")).is_err());
    assert!(verify_tunnel_client(&link.join("tunnel-client.exe"))
        .await
        .is_err());
    let candidate = fixture.path().join("candidate.exe");
    write_private_file(&candidate, b"candidate").unwrap();
    assert!(install_verified_tunnel_client(&candidate, &link.join("tunnel-client.exe")).is_err());
    assert!(install_verified_tunnel_client(&link.join("tunnel-client.exe"), &candidate).is_err());
    let asset = tunnel_client_asset().unwrap();
    assert!(resolve_managed_tunnel_client(link, &asset).await.is_err());
    assert!(
        !target.join("tools").exists(),
        "reject before installing or downloading"
    );
    assert_eq!(fs::read(&binary).unwrap(), b"untouched");
    assert_eq!(fs::read(&candidate).unwrap(), b"candidate");
}

#[test]
fn windows_identity_rejects_oversized_and_non_regular_paths_and_tracks_changes() {
    let fixture = tempfile::tempdir().unwrap();
    let file = fixture.path().join("binary.exe");
    write_private_file(&file, b"abc").unwrap();
    let first = windows_support::file_identity(&file, 3).unwrap();
    assert_eq!(first.length, 3);
    assert_eq!(windows_support::file_identity(&file, 3).unwrap(), first);
    assert!(windows_support::file_identity(&file, 2).is_err());
    assert!(windows_support::file_identity(fixture.path(), 3).is_err());
    fs::write(&file, b"abcd").unwrap();
    assert_ne!(windows_support::file_identity(&file, 4).unwrap(), first);
}

#[test]
fn windows_oversized_corrupt_cache_is_still_replaceable() {
    let fixture = tempfile::tempdir().unwrap();
    let destination = fixture.path().join("corrupt.exe");
    let candidate = fixture.path().join("candidate.exe");
    let corrupt = File::create(&destination).unwrap();
    corrupt.set_len(MAX_BINARY_BYTES + 1).unwrap();
    drop(corrupt);
    assert!(tunnel_client_file_identity(&destination).is_err());
    write_private_file(&candidate, b"verified bytes").unwrap();
    install_verified_tunnel_client(&candidate, &destination).unwrap();
    assert_eq!(fs::read(&destination).unwrap(), b"verified bytes");
    assert_private_dacl(&destination, false);
}

fn build_local_version_client(directory: &Path) -> PathBuf {
    // A real local PE exercises CreateProcess/exit status and cache identity.
    // The separate official-asset integration below retains network coverage.
    let source = directory.join("version_client.rs");
    fs::write(&source, r#"
use std::{env, fs, io::Write};
fn main() {
    assert_eq!(env::args().nth(1).as_deref(), Some("--version"));
    let root = env::current_exe().unwrap().parent().unwrap().to_owned();
    let mut calls = fs::OpenOptions::new().create(true).append(true).open(root.join("calls")).unwrap();
    writeln!(calls, "version").unwrap();
    match fs::read_to_string(root.join("mode")).unwrap_or_default().as_str() {
        "wrong" => println!("0.0.120"),
        "failure" => { println!("0.0.12"); std::process::exit(7); }
        _ => println!("0.0.12"),
    }
}
"#).unwrap();
    let binary = directory.join("local-client.exe");
    let output =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg("--edition=2021")
            .arg("--crate-name=local_version_client")
            .arg(&source)
            .arg("-o")
            .arg(&binary)
            .output()
            .unwrap();
    assert!(
        output.status.success(),
        "local PE fixture compile failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    binary
}

#[tokio::test]
async fn windows_native_version_launch_and_manager_cache_invalidation() {
    let fixture = tempfile::tempdir().unwrap();
    let directory = fixture.path().join("native PE 空間");
    create_private_dir(&directory).unwrap();
    let binary = build_local_version_client(&directory);
    let manager = TunnelManager::new_with_tunnel_client(
        fixture.path().join("managed"),
        Arc::new(VerificationTracker::default()),
        Arc::new(PerformanceTraceStore::default()),
        binary.clone(),
    );
    assert_eq!(
        manager.resolve_tunnel_client_for_start().await.unwrap(),
        binary
    );
    assert_eq!(
        manager.resolve_tunnel_client_for_start().await.unwrap(),
        binary
    );
    assert_eq!(
        fs::read_to_string(directory.join("calls"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    // PE permits trailing bytes; append changes identity while retaining a valid executable.
    fs::OpenOptions::new()
        .append(true)
        .open(&binary)
        .unwrap()
        .write_all(b"identity change")
        .unwrap();
    assert_eq!(
        manager.resolve_tunnel_client_for_start().await.unwrap(),
        binary
    );
    assert_eq!(
        fs::read_to_string(directory.join("calls"))
            .unwrap()
            .lines()
            .count(),
        2
    );
    fs::write(directory.join("mode"), "wrong").unwrap();
    assert!(verify_tunnel_client(&binary).await.is_err());
    fs::write(directory.join("mode"), "failure").unwrap();
    assert!(verify_tunnel_client(&binary).await.is_err());
}

#[tokio::test]
#[ignore = "opt-in: downloads the pinned official Windows asset; requires GitHub HTTPS, no secrets"]
async fn windows_official_asset_install_reuse_replacement_version() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path().join("official install 空間");
    let asset = tunnel_client_asset().unwrap();
    assert!(asset.target.starts_with("windows-"));
    // Calls the same production downloader, bounded extraction, hashes, DACL and launcher.
    let binary = resolve_managed_tunnel_client(&root, &asset).await.unwrap();
    verify_sha256(&binary, asset.binary_sha256).unwrap();
    verify_tunnel_client(&binary).await.unwrap();
    assert_private_dacl(binary.parent().unwrap(), true);
    assert_private_dacl(&binary, false);
    let original = tunnel_client_file_identity(&binary).unwrap();
    assert_eq!(
        resolve_managed_tunnel_client(&root, &asset).await.unwrap(),
        binary
    );
    assert_eq!(
        tunnel_client_file_identity(&binary).unwrap(),
        original,
        "cache reuse must not replace the file"
    );
    fs::write(&binary, b"corrupt cached PE").unwrap();
    let corrupt = tunnel_client_file_identity(&binary).unwrap();
    assert!(verify_sha256(&binary, asset.binary_sha256).is_err());
    assert_eq!(
        resolve_managed_tunnel_client(&root, &asset).await.unwrap(),
        binary
    );
    assert_ne!(tunnel_client_file_identity(&binary).unwrap(), corrupt);
    verify_sha256(&binary, asset.binary_sha256).unwrap();
    verify_tunnel_client(&binary).await.unwrap();
    assert_private_dacl(&binary, false);
    assert!(
        fs::read_dir(root.join("tools")).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tunnel-install-")),
        "installer must clean its temporary directory"
    );
}
