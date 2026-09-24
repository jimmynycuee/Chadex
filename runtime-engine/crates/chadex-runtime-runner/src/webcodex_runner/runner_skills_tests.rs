use super::*;
use crate::webcodex_runner::config::{RunnerPolicy, SkillsConfig};
use crate::webcodex_runner::skill_store::SkillStore;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Cursor, Write};
use tempfile::TempDir;
use webcodex_core::runner_skill::{RunnerSkillDescriptor, RunnerSkillSource};
use zip::write::SimpleFileOptions;

fn write_configured_skill(root: &std::path::Path) {
    let package = root.join("configured");
    fs::create_dir_all(package.join("references")).unwrap();
    fs::write(
        package.join("SKILL.md"),
        "---\nname: configured\ndescription: configured guidance\n---\nbody\n",
    )
    .unwrap();
    fs::write(package.join("references/guide.md"), "configured resource\n").unwrap();
}

fn configured_fixture() -> (TempDir, SkillsConfig, RunnerSkillDescriptor) {
    let root = tempfile::tempdir().unwrap();
    write_configured_skill(root.path());
    let config = SkillsConfig {
        roots: vec![root.path().to_path_buf()],
    };
    let descriptor = configured_skills::discover(&config)
        .unwrap()
        .skills
        .remove(0)
        .descriptor;
    (root, config, descriptor)
}

fn archive_bytes() -> Vec<u8> {
    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    writer.start_file("SKILL.md", options).unwrap();
    writer
        .write_all(b"---\nname: managed\ndescription: managed guidance\n---\nbody\n")
        .unwrap();
    writer.start_file("references/guide.md", options).unwrap();
    writer.write_all(b"managed resource\n").unwrap();
    writer.finish().unwrap().into_inner()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn managed_fixture() -> (TempDir, SkillStore, RunnerSkillDescriptor) {
    let root = tempfile::tempdir().unwrap();
    let source = tempfile::tempdir().unwrap();
    let store = SkillStore::for_test(root.path().join("store"), "runner-test");
    let archive = archive_bytes();
    let archive_sha = sha256_hex(&archive);
    fs::write(source.path().join("skill.zip"), &archive).unwrap();
    let policy = RunnerPolicy {
        allow_cwd_anywhere: true,
        ..RunnerPolicy::default()
    };
    let installed = store
        .install(
            &policy,
            "managed",
            "project-test",
            source.path().to_string_lossy().as_ref(),
            "skill.zip",
            &archive_sha,
            "install-managed-fixture",
            true,
            None,
        )
        .unwrap();
    let descriptor = store
        .resolve_active(&installed.skill_id)
        .unwrap()
        .expect("installed Skill must be active");
    (root, store, descriptor)
}

#[test]
fn configured_only_runtime_list_resolve_and_read_use_canonical_source() {
    let (_root, config, configured) = configured_fixture();
    let store_root = tempfile::tempdir().unwrap();
    let store = SkillStore::for_test(store_root.path().join("store"), "runner-empty");

    let listed = list_runner_skills(&config, &store).unwrap();
    assert_eq!(listed.skills.len(), 1);
    assert_eq!(listed.skills[0].source(), RunnerSkillSource::Configured);

    let resolved = resolve_runner_skill(&config, &store, configured.skill_id())
        .unwrap()
        .expect("configured target");
    assert_eq!(resolved.source(), RunnerSkillSource::Configured);
    assert_eq!(resolved.skill_id(), configured.skill_id());

    let read = read_runner_skill(
        &config,
        &store,
        configured.skill_id(),
        RunnerSkillSource::Configured,
        "references/guide.md",
        1,
        20,
        None,
        Some(configured.definition_revision()),
    )
    .unwrap();
    assert_eq!(read.skill.source(), RunnerSkillSource::Configured);
    assert_eq!(read.text, "configured resource");
}

#[test]
fn managed_only_runtime_resolve_read_and_explicit_revision_checks_are_preserved() {
    let (_root, store, managed) = managed_fixture();
    let config = SkillsConfig::default();

    let listed = list_runner_skills(&config, &store).unwrap();
    assert_eq!(listed.skills.len(), 1);
    assert_eq!(listed.skills[0].source(), RunnerSkillSource::Managed);
    let resolved = resolve_runner_skill(&config, &store, managed.skill_id())
        .unwrap()
        .expect("managed target");
    assert_eq!(resolved.source(), RunnerSkillSource::Managed);

    let read = read_runner_skill(
        &config,
        &store,
        managed.skill_id(),
        RunnerSkillSource::Managed,
        "references/guide.md",
        1,
        20,
        None,
        None,
    )
    .unwrap();
    assert_eq!(read.text, "managed resource");

    assert_eq!(
        read_runner_skill(
            &config,
            &store,
            managed.skill_id(),
            RunnerSkillSource::Managed,
            "SKILL.md",
            1,
            20,
            Some(&"wc_skillpkg___________________________________________8".to_string()),
            None,
        )
        .unwrap_err(),
        "skill_package_changed"
    );
    assert_eq!(
        read_runner_skill(
            &config,
            &store,
            managed.skill_id(),
            RunnerSkillSource::Managed,
            "SKILL.md",
            1,
            20,
            None,
            Some(&"f".repeat(64)),
        )
        .unwrap_err(),
        "skill_definition_changed"
    );
}

#[test]
fn mixed_list_keeps_configured_and_managed_source_identity_explicit() {
    let (_configured_root, config, configured) = configured_fixture();
    let (_managed_root, store, managed) = managed_fixture();
    let listed = list_runner_skills(&config, &store).unwrap();
    assert_eq!(listed.skills.len(), 2);
    assert!(listed
        .skills
        .iter()
        .any(|skill| skill.skill_id() == configured.skill_id()
            && skill.source() == RunnerSkillSource::Configured));
    assert!(listed
        .skills
        .iter()
        .any(|skill| skill.skill_id() == managed.skill_id()
            && skill.source() == RunnerSkillSource::Managed));
    assert!(resolve_runner_skill(
        &config,
        &store,
        &"wc_skill_AAAAAAAAAAAAAAAAAAAAAA".to_string()
    )
    .unwrap()
    .is_none());
}

#[test]
fn duplicate_target_and_source_identity_change_fail_closed_without_priority() {
    let duplicate_id = "wc_skill_qqqqqqqqqqqqqqqqqqqqqg".to_string();
    let configured = RunnerSkillDescriptor::Configured {
        skill_id: duplicate_id.clone(),
        name: "configured".to_string(),
        description: "configured".to_string(),
        definition_revision: "b".repeat(64),
    };
    let managed = RunnerSkillDescriptor::Managed {
        skill_id: duplicate_id,
        skill_key: "managed".to_string(),
        name: "managed".to_string(),
        description: "managed".to_string(),
        package_revision: "wc_skillpkg_zMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMw".to_string(),
        definition_revision: "d".repeat(64),
    };

    assert_eq!(
        resolve_candidates(Some(configured.clone()), Some(managed.clone())).unwrap_err(),
        "skill_catalog_unavailable"
    );
    assert_eq!(
        ensure_unique_skill_ids(&[configured.clone(), managed.clone()]).unwrap_err(),
        "skill_catalog_unavailable"
    );
    assert_eq!(
        require_resolved_source(Some(&managed), RunnerSkillSource::Configured).unwrap_err(),
        "skill_source_changed"
    );
    assert_eq!(
        require_resolved_source(None, RunnerSkillSource::Configured).unwrap_err(),
        "skill_source_changed"
    );
}
