//! Flattens a downloaded Skill ZIP that wraps its package in one folder.
//!
//! The runtime installs a ZIP only when `SKILL.md` is at the archive root, but
//! packages shared on the web are commonly compressed from their folder
//! (`my-skill/SKILL.md`, ...). Exactly that shape is rewritten here, before the
//! runtime sees it:
//!
//! * the root has no `SKILL.md`,
//! * every entry (ignoring Finder's `__MACOSX/` tree and `.DS_Store` files) is
//!   inside one top-level folder, and
//! * that folder contains `SKILL.md`.
//!
//! Anything else is returned unchanged ([`Flatten::Unchanged`]) so the runtime
//! reports its own error as before. While rewriting, the runtime's archive
//! bounds are enforced again with the shared `chadex-runtime-core` limits, and
//! a violation is reported with the runtime's own error code. The runtime still
//! validates the rewritten ZIP in full; these checks only keep the helper from
//! inflating or re-packing anything the runtime would refuse.

use crate::chadex_core::ChadexError;
use chadex_runtime_core::skill_store::{
    MAX_SKILL_STORE_ARCHIVE_BYTES, MAX_SKILL_STORE_FILE_BYTES, MAX_SKILL_STORE_FILE_COUNT,
    MAX_SKILL_STORE_PATH_CHARS, MAX_SKILL_STORE_PATH_DEPTH, MAX_SKILL_STORE_TOTAL_BYTES,
};
use std::collections::BTreeSet;
use std::io::{Cursor, Read, Write};
use std::path::{Component, Path};
use zip::write::SimpleFileOptions;
use zip::ZipArchive;

const SKILL_DEFINITION: &str = "SKILL.md";
const FINDER_RESOURCE_FORK_DIR: &str = "__MACOSX";
const FINDER_METADATA_FILE: &str = ".DS_Store";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Flatten {
    /// Not the single-wrapper-folder shape: install the original bytes.
    Unchanged,
    /// The package with its wrapper folder removed, as a new ZIP.
    Flattened(Vec<u8>),
}

fn rejected(code: &'static str) -> ChadexError {
    ChadexError::new(
        code,
        "Chadex could not install the Skill",
        "Check the Skill ZIP and choose it again.",
    )
}

/// Finder adds these when it compresses a folder; they are never part of the
/// package, so they neither block nor survive flattening.
fn is_finder_artifact(name: &str) -> bool {
    let trimmed = name.trim_end_matches('/');
    trimmed.split('/').next() == Some(FINDER_RESOURCE_FORK_DIR)
        || trimmed.rsplit('/').next() == Some(FINDER_METADATA_FILE)
}

/// The runtime's archive path rule (`normalize_archive_path` in the runner
/// skill store): relative, `/`-separated, no empty, `.` or `..` component, no
/// backslash, drive prefix or control character, bounded length and depth.
fn valid_package_path(path: &str) -> bool {
    !path.is_empty()
        && path.chars().count() <= MAX_SKILL_STORE_PATH_CHARS
        && !path.contains(['\\', '\0'])
        && !path.starts_with('/')
        && path.as_bytes().get(1) != Some(&b':')
        && !path.chars().any(char::is_control)
        && path.split('/').count() <= MAX_SKILL_STORE_PATH_DEPTH
        && path
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Only regular files and directories; symlinks and other special entries are
/// refused like the runtime does (`reject_archive_special_entry`).
fn special_entry(mode: Option<u32>, is_dir: bool) -> bool {
    let Some(kind) = mode.map(|mode| mode & 0o170000) else {
        return false;
    };
    kind != 0 && kind != if is_dir { 0o040000 } else { 0o100000 }
}

/// The single wrapper folder, when the archive has exactly the flattenable
/// shape. Reads only the central directory (names), nothing is inflated.
fn wrapper_folder(names: &[(String, bool)]) -> Option<String> {
    let mut wrapper: Option<&str> = None;
    let mut has_definition = false;
    for (name, is_dir) in names {
        if is_finder_artifact(name) {
            continue;
        }
        let (top, rest) = match name.split_once('/') {
            Some((top, rest)) => (top, rest),
            // A file at the root (including a root SKILL.md) keeps the archive as is.
            None => return None,
        };
        if top.is_empty() || top == "." || top == ".." {
            return None;
        }
        match wrapper {
            Some(existing) if existing != top => return None,
            _ => wrapper = Some(top),
        }
        if rest == SKILL_DEFINITION && !is_dir {
            has_definition = true;
        }
    }
    wrapper.filter(|_| has_definition).map(str::to_owned)
}

pub(super) fn flatten_single_folder_archive(bytes: &[u8]) -> Result<Flatten, ChadexError> {
    if bytes.len() > MAX_SKILL_STORE_ARCHIVE_BYTES {
        return Ok(Flatten::Unchanged);
    }
    let Ok(mut archive) = ZipArchive::new(Cursor::new(bytes)) else {
        return Ok(Flatten::Unchanged);
    };
    if archive.len() > MAX_SKILL_STORE_FILE_COUNT.saturating_mul(2) {
        return Ok(Flatten::Unchanged);
    }
    let mut names = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index_raw(index) else {
            return Ok(Flatten::Unchanged);
        };
        let Ok(name) = std::str::from_utf8(entry.name_raw()) else {
            return Ok(Flatten::Unchanged);
        };
        names.push((name.to_owned(), entry.is_dir()));
    }
    let Some(wrapper) = wrapper_folder(&names) else {
        return Ok(Flatten::Unchanged);
    };
    let prefix = format!("{wrapper}/");

    let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let mut casefold = BTreeSet::<String>::new();
    let mut file_count = 0usize;
    let mut total_bytes = 0usize;
    for (index, (name, is_dir)) in names.iter().enumerate() {
        if is_finder_artifact(name) {
            continue;
        }
        let relative = name
            .strip_prefix(&prefix)
            .ok_or_else(|| rejected("skill_install_archive_path_invalid"))?;
        let relative = if *is_dir {
            relative.strip_suffix('/').unwrap_or(relative)
        } else {
            relative
        };
        let mut entry = archive
            .by_index(index)
            .map_err(|_| rejected("skill_install_archive_malformed"))?;
        if special_entry(entry.unix_mode(), *is_dir) {
            return Err(rejected("skill_install_archive_special_entry"));
        }
        if *is_dir {
            // The wrapper folder itself, or a sub-folder: files carry their paths.
            if !relative.is_empty() && !valid_package_path(relative) {
                return Err(rejected("skill_install_archive_path_invalid"));
            }
            continue;
        }
        if !valid_package_path(relative) || !valid_package_path(name) {
            return Err(rejected("skill_install_archive_path_invalid"));
        }
        file_count += 1;
        if file_count > MAX_SKILL_STORE_FILE_COUNT {
            return Err(rejected("skill_install_file_count_exceeded"));
        }
        let size = usize::try_from(entry.size())
            .map_err(|_| rejected("skill_install_file_too_large"))?;
        if size > MAX_SKILL_STORE_FILE_BYTES {
            return Err(rejected("skill_install_file_too_large"));
        }
        total_bytes = total_bytes
            .checked_add(size)
            .filter(|total| *total <= MAX_SKILL_STORE_TOTAL_BYTES)
            .ok_or_else(|| rejected("skill_install_total_too_large"))?;
        if !casefold.insert(relative.to_lowercase()) {
            return Err(rejected("skill_install_duplicate_path"));
        }
        // Never trust the declared size: inflate at most one byte past it.
        let mut body = Vec::with_capacity(size);
        entry
            .by_ref()
            .take((MAX_SKILL_STORE_FILE_BYTES + 1) as u64)
            .read_to_end(&mut body)
            .map_err(|_| rejected("skill_install_archive_malformed"))?;
        if body.len() != size {
            return Err(rejected("skill_install_archive_size_mismatch"));
        }
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        output
            .start_file(relative, options)
            .map_err(|_| rejected("skill_install_archive_malformed"))?;
        output
            .write_all(&body)
            .map_err(|_| rejected("skill_install_archive_malformed"))?;
    }
    let flattened = output
        .finish()
        .map_err(|_| rejected("skill_install_archive_malformed"))?
        .into_inner();
    if flattened.len() > MAX_SKILL_STORE_ARCHIVE_BYTES {
        return Err(rejected("skill_install_archive_too_large"));
    }
    Ok(Flatten::Flattened(flattened))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    const DEFINITION: &[u8] = b"---\nname: english-tv-coach\ndescription: demo\n---\n";

    pub(in crate::chadex_core::adapters) fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            if let Some(dir) = name.strip_suffix('/') {
                writer.add_directory(dir, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(body).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    fn entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
        (0..archive.len())
            .map(|index| {
                let mut entry = archive.by_index(index).unwrap();
                let mut body = Vec::new();
                entry.read_to_end(&mut body).unwrap();
                (entry.name().to_string(), body)
            })
            .collect()
    }

    fn flattened(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        match flatten_single_folder_archive(bytes).unwrap() {
            Flatten::Flattened(out) => entries(&out),
            Flatten::Unchanged => panic!("expected the archive to be flattened"),
        }
    }

    #[test]
    fn single_wrapper_folder_is_removed() {
        let zip = zip_with(&[
            ("english-tv-coach/SKILL.md", DEFINITION),
            ("english-tv-coach/agents/openai.yaml", b"interface: {}\n"),
            ("english-tv-coach/assets/icon.svg", b"<svg/>"),
        ]);
        assert_eq!(
            flattened(&zip),
            vec![
                ("SKILL.md".to_string(), DEFINITION.to_vec()),
                ("agents/openai.yaml".to_string(), b"interface: {}\n".to_vec()),
                ("assets/icon.svg".to_string(), b"<svg/>".to_vec()),
            ]
        );
    }

    #[test]
    fn finder_artifacts_are_ignored_and_dropped() {
        let zip = zip_with(&[
            ("english-tv-coach/", b""),
            ("english-tv-coach/SKILL.md", DEFINITION),
            ("english-tv-coach/.DS_Store", b"finder"),
            ("english-tv-coach/assets/", b""),
            ("english-tv-coach/assets/icon.svg", b"<svg/>"),
            ("__MACOSX/", b""),
            ("__MACOSX/english-tv-coach/._SKILL.md", b"fork"),
            (".DS_Store", b"finder"),
        ]);
        let names: Vec<String> = flattened(&zip).into_iter().map(|(name, _)| name).collect();
        assert_eq!(names, vec!["SKILL.md", "assets/icon.svg"]);
    }

    #[test]
    fn other_shapes_are_left_to_the_runtime() {
        for zip in [
            // Already flat.
            zip_with(&[("SKILL.md", DEFINITION), ("assets/icon.svg", b"<svg/>")]),
            // Root SKILL.md next to a folder.
            zip_with(&[("SKILL.md", DEFINITION), ("pkg/SKILL.md", DEFINITION)]),
            // Two top-level folders.
            zip_with(&[("a/SKILL.md", DEFINITION), ("b/SKILL.md", DEFINITION)]),
            // A root file beside the wrapper.
            zip_with(&[("pkg/SKILL.md", DEFINITION), ("README.md", b"x")]),
            // The folder has no SKILL.md at its top.
            zip_with(&[("pkg/nested/SKILL.md", DEFINITION)]),
            // Only Finder artifacts.
            zip_with(&[("__MACOSX/pkg/SKILL.md", DEFINITION)]),
            // Not a ZIP.
            b"not a zip".to_vec(),
        ] {
            assert_eq!(flatten_single_folder_archive(&zip).unwrap(), Flatten::Unchanged);
        }
    }

    #[test]
    fn path_escapes_inside_the_wrapper_are_refused() {
        for name in ["pkg/../evil.md", "pkg/./x.md", "pkg//x.md", "pkg/a\\b.md"] {
            let zip = zip_with(&[("pkg/SKILL.md", DEFINITION), (name, b"x")]);
            assert_eq!(
                flatten_single_folder_archive(&zip).unwrap_err().code,
                "skill_install_archive_path_invalid",
                "{name}"
            );
        }
    }

    #[test]
    fn symlink_entries_are_refused() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = SimpleFileOptions::default();
        writer.start_file("pkg/SKILL.md", options).unwrap();
        writer.write_all(DEFINITION).unwrap();
        writer.add_symlink("pkg/link", "/etc/passwd", options).unwrap();
        let zip = writer.finish().unwrap().into_inner();
        assert_eq!(
            flatten_single_folder_archive(&zip).unwrap_err().code,
            "skill_install_archive_special_entry"
        );
    }

    #[test]
    fn duplicate_names_are_refused() {
        let zip = zip_with(&[
            ("pkg/SKILL.md", DEFINITION),
            ("pkg/notes.md", b"a"),
            ("pkg/NOTES.md", b"b"),
        ]);
        assert_eq!(
            flatten_single_folder_archive(&zip).unwrap_err().code,
            "skill_install_duplicate_path"
        );
    }

    #[test]
    fn decompression_bombs_are_refused_before_inflating_past_the_bounds() {
        let large = vec![0u8; MAX_SKILL_STORE_FILE_BYTES];
        let zip = zip_with(&[
            ("pkg/SKILL.md", DEFINITION),
            ("pkg/a.bin", &large),
            ("pkg/b.bin", &large),
            ("pkg/c.bin", &large),
            ("pkg/d.bin", &large),
        ]);
        assert!(zip.len() < MAX_SKILL_STORE_ARCHIVE_BYTES);
        assert_eq!(
            flatten_single_folder_archive(&zip).unwrap_err().code,
            "skill_install_total_too_large"
        );

        let too_big = vec![0u8; MAX_SKILL_STORE_FILE_BYTES + 1];
        let zip = zip_with(&[("pkg/SKILL.md", DEFINITION), ("pkg/big.bin", &too_big)]);
        assert_eq!(
            flatten_single_folder_archive(&zip).unwrap_err().code,
            "skill_install_file_too_large"
        );

        let mut many: Vec<(String, Vec<u8>)> = vec![("pkg/SKILL.md".into(), DEFINITION.to_vec())];
        many.extend(
            (0..MAX_SKILL_STORE_FILE_COUNT).map(|index| (format!("pkg/f{index}.md"), vec![])),
        );
        let refs: Vec<(&str, &[u8])> = many
            .iter()
            .map(|(name, body)| (name.as_str(), body.as_slice()))
            .collect();
        assert_eq!(
            flatten_single_folder_archive(&zip_with(&refs)).unwrap_err().code,
            "skill_install_file_count_exceeded"
        );
    }
}
