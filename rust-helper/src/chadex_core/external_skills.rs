//! Read-only discovery of Skill folders that other agents already maintain
//! (the shared `~/.agents/skills`, Claude Code and Codex). The package rules
//! mirror the Runner's configured `[skills] roots` scanner so that a
//! recommended root is one the Runner will actually accept: roots and package
//! directories must not be links, `SKILL.md` must be a regular bounded UTF-8
//! file with explicit `name`/`description` frontmatter. Only metadata is
//! returned; Skill bodies never leave this module.

use chadex_runtime_core::sensitive_paths::is_secret_path;
use chadex_runtime_core::skill_metadata::{parse_skill_metadata, MAX_SKILL_DEFINITION_BYTES};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const EXTERNAL_SKILL_DISCOVERY_FORMAT: &str = "chadex.external_skill_sources.v1";
/// Same bound as the Runner's `MAX_CONFIGURED_SKILL_ROOT_SCAN_ENTRIES`.
pub(crate) const MAX_ROOT_SCAN_ENTRIES: usize = 1024;
/// Same bound as the Runner's `MAX_CONFIGURED_SKILL_PACKAGES`.
pub(crate) const MAX_PACKAGES_PER_ROOT: usize = 256;
const MAX_PACKAGE_NAME_BYTES: usize = 160;
const SKILL_DEFINITION_FILE: &str = "SKILL.md";
const SKILL_SCRIPTS_DIR: &str = "scripts";

#[derive(Debug, Clone)]
pub(crate) struct DiscoveryEnv {
    pub(crate) home: PathBuf,
    pub(crate) claude_config_dir: Option<PathBuf>,
    pub(crate) codex_home: Option<PathBuf>,
}

impl DiscoveryEnv {
    pub(crate) fn from_process() -> Option<Self> {
        let home = chadex_runtime_runner_config::paths::home_dir()?;
        Some(Self {
            home,
            claude_config_dir: absolute_env_path("CLAUDE_CONFIG_DIR"),
            codex_home: absolute_env_path("CODEX_HOME"),
        })
    }

    fn candidates(&self) -> [(&'static str, PathBuf); 3] {
        [
            ("agents", self.home.join(".agents").join("skills")),
            (
                "claude",
                self.claude_config_dir
                    .clone()
                    .unwrap_or_else(|| self.home.join(".claude"))
                    .join("skills"),
            ),
            (
                "codex",
                self.codex_home
                    .clone()
                    .unwrap_or_else(|| self.home.join(".codex"))
                    .join("skills"),
            ),
        ]
    }
}

fn absolute_env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ExternalSkillDiscovery {
    pub(crate) format: &'static str,
    pub(crate) sources: Vec<ExternalSkillSource>,
    /// Canonical roots worth connecting, in discovery order: sources with real
    /// valid packages plus the roots that link-only sources point into.
    pub(crate) recommended_roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ExternalSkillSource {
    pub(crate) kind: &'static str,
    pub(crate) path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) canonical_path: Option<String>,
    /// `available`, `not_found`, `not_directory`, `unavailable`,
    /// `scan_limit_exceeded` or `duplicate_source`.
    pub(crate) status: &'static str,
    /// The configured path itself is a link; the Runner only accepts
    /// `canonical_path`.
    pub(crate) root_is_link: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) same_as: Option<&'static str>,
    pub(crate) valid_count: usize,
    pub(crate) symlink_count: usize,
    pub(crate) invalid_count: usize,
    pub(crate) script_count: usize,
    pub(crate) truncated: bool,
    /// Canonical roots that this source's link packages resolve into.
    pub(crate) provided_by: Vec<String>,
    pub(crate) packages: Vec<ExternalSkillPackage>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ExternalSkillPackage {
    pub(crate) package: String,
    /// `valid`, `symlink` or `invalid`.
    pub(crate) state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) description: Option<String>,
    pub(crate) has_scripts: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) link_target_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) invalid_reason: Option<&'static str>,
    pub(crate) name_conflict: bool,
    #[serde(skip)]
    canonical_package: Option<PathBuf>,
}

pub(crate) fn discover_external_skill_sources(env: &DiscoveryEnv) -> ExternalSkillDiscovery {
    let mut sources = Vec::new();
    let mut seen_roots: Vec<(PathBuf, &'static str)> = Vec::new();
    for (kind, path) in env.candidates() {
        let mut source = scan_source(kind, &path);
        if let Some(canonical) = source.canonical_path.as_deref().map(PathBuf::from) {
            if let Some((_, first_kind)) = seen_roots.iter().find(|(root, _)| *root == canonical) {
                source.status = "duplicate_source";
                source.same_as = Some(first_kind);
                source.packages.clear();
                source.valid_count = 0;
                source.symlink_count = 0;
                source.invalid_count = 0;
                source.script_count = 0;
                source.provided_by.clear();
            } else {
                seen_roots.push((canonical, kind));
            }
        }
        sources.push(source);
    }
    mark_name_conflicts(&mut sources);
    let recommended_roots = recommended_roots(&sources, &env.home);
    ExternalSkillDiscovery {
        format: EXTERNAL_SKILL_DISCOVERY_FORMAT,
        sources,
        recommended_roots,
    }
}

fn scan_source(kind: &'static str, path: &Path) -> ExternalSkillSource {
    let mut source = ExternalSkillSource {
        kind,
        path: path.to_string_lossy().into_owned(),
        canonical_path: None,
        status: "available",
        root_is_link: false,
        same_as: None,
        valid_count: 0,
        symlink_count: 0,
        invalid_count: 0,
        script_count: 0,
        truncated: false,
        provided_by: Vec::new(),
        packages: Vec::new(),
    };
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            source.status = "not_found";
            return source;
        }
        Err(_) => {
            source.status = "unavailable";
            return source;
        }
    };
    source.root_is_link = metadata_is_link_like(&metadata);
    let root = match path.canonicalize() {
        Ok(root) if root.is_dir() => root,
        Ok(_) => {
            source.status = "not_directory";
            return source;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            source.status = "not_found";
            return source;
        }
        Err(_) => {
            source.status = "unavailable";
            return source;
        }
    };
    source.canonical_path = Some(root.to_string_lossy().into_owned());
    let entries = match bounded_root_entries(&root) {
        Ok(entries) => entries,
        Err(status) => {
            source.status = status;
            source.truncated = status == "scan_limit_exceeded";
            return source;
        }
    };
    let mut provided_by = BTreeSet::new();
    for entry in entries {
        if source.packages.len() >= MAX_PACKAGES_PER_ROOT {
            source.truncated = true;
            break;
        }
        let package = inspect_package(&root, &entry);
        match package.state {
            "valid" => {
                source.valid_count += 1;
                if package.has_scripts {
                    source.script_count += 1;
                }
            }
            "symlink" => {
                source.symlink_count += 1;
                if let Some(target_root) = &package.link_target_root {
                    provided_by.insert(target_root.clone());
                }
            }
            _ => source.invalid_count += 1,
        }
        source.packages.push(package);
    }
    source.provided_by = provided_by.into_iter().collect();
    source
}

fn bounded_root_entries(root: &Path) -> Result<Vec<(String, bool)>, &'static str> {
    let read_dir = fs::read_dir(root).map_err(|_| "unavailable")?;
    let mut candidates = Vec::new();
    for (scanned, entry) in read_dir.enumerate() {
        if scanned >= MAX_ROOT_SCAN_ENTRIES {
            return Err("scan_limit_exceeded");
        }
        let entry = entry.map_err(|_| "unavailable")?;
        let file_type = entry.file_type().map_err(|_| "unavailable")?;
        if !file_type.is_dir() && !file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        candidates.push((name, file_type.is_symlink()));
    }
    candidates.sort();
    Ok(candidates)
}

fn inspect_package(root: &Path, (package_name, is_link): &(String, bool)) -> ExternalSkillPackage {
    let mut package = ExternalSkillPackage {
        package: package_name.clone(),
        state: "invalid",
        name: None,
        description: None,
        has_scripts: false,
        link_target_root: None,
        invalid_reason: None,
        name_conflict: false,
        canonical_package: None,
    };
    if !valid_package_name(package_name) {
        package.invalid_reason = Some("invalid_skill_package");
        return package;
    }
    if is_secret_path(package_name) {
        package.invalid_reason = Some("sensitive_skill_definition");
        return package;
    }
    let entry_path = root.join(package_name);
    let package_root = match entry_path.canonicalize() {
        Ok(path) if path.is_dir() => path,
        _ => {
            if *is_link {
                package.state = "symlink";
            }
            package.invalid_reason = Some("invalid_skill_package");
            return package;
        }
    };
    if *is_link {
        // The Runner rejects link packages, so report where the real package
        // lives; connecting that root is what makes this Skill usable.
        package.state = "symlink";
        package.link_target_root = package_root
            .parent()
            .map(|parent| parent.to_string_lossy().into_owned());
    } else if !chadex_runtime_runner_config::paths::path_is_within(&package_root, root) {
        package.invalid_reason = Some("invalid_skill_package");
        return package;
    }
    match read_package_metadata(&package_root) {
        Ok((name, description, has_scripts)) => {
            if !*is_link {
                package.state = "valid";
            }
            package.name = Some(name);
            package.description = Some(description);
            package.has_scripts = has_scripts;
            package.canonical_package = Some(package_root);
        }
        Err(reason) => package.invalid_reason = Some(reason),
    }
    package
}

fn read_package_metadata(package_root: &Path) -> Result<(String, String, bool), &'static str> {
    let definition = package_root.join(SKILL_DEFINITION_FILE);
    let metadata = fs::symlink_metadata(&definition).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "missing_skill_definition"
        } else {
            "invalid_skill_package"
        }
    })?;
    if metadata_is_link_like(&metadata) || !metadata.is_file() {
        return Err("invalid_skill_package");
    }
    let bytes = read_bounded(&definition, MAX_SKILL_DEFINITION_BYTES)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| "invalid_utf8_skill_definition")?;
    let skill = parse_skill_metadata(text)?;
    let has_scripts = fs::symlink_metadata(package_root.join(SKILL_SCRIPTS_DIR))
        .map(|metadata| metadata.is_dir() && !metadata_is_link_like(&metadata))
        .unwrap_or(false);
    Ok((skill.name, skill.description, has_scripts))
}

fn read_bounded(path: &Path, max_bytes: usize) -> Result<Vec<u8>, &'static str> {
    let file = File::open(path).map_err(|_| "invalid_skill_package")?;
    let mut bytes = Vec::with_capacity(max_bytes.min(64 * 1024));
    file.take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| "invalid_skill_package")?;
    if bytes.len() > max_bytes {
        return Err("skill_definition_too_large");
    }
    Ok(bytes)
}

/// Same-name packages that resolve to different real directories would show
/// up as a catalog `name_conflict` once both roots are connected.
fn mark_name_conflicts(sources: &mut [ExternalSkillSource]) {
    let mut by_name = BTreeMap::<String, BTreeSet<PathBuf>>::new();
    for package in sources.iter().flat_map(|source| source.packages.iter()) {
        if let (Some(name), Some(canonical)) = (&package.name, &package.canonical_package) {
            by_name
                .entry(name.to_lowercase())
                .or_default()
                .insert(canonical.clone());
        }
    }
    for package in sources.iter_mut().flat_map(|source| source.packages.iter_mut()) {
        package.name_conflict = package
            .name
            .as_ref()
            .and_then(|name| by_name.get(&name.to_lowercase()))
            .is_some_and(|paths| paths.len() > 1);
    }
}

fn recommended_roots(sources: &[ExternalSkillSource], home: &Path) -> Vec<String> {
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    let mut roots = Vec::<String>::new();
    let mut push = |root: &str| {
        let path = Path::new(root);
        if path == home || path.parent().is_none() || roots.iter().any(|seen| seen == root) {
            return;
        }
        roots.push(root.to_string());
    };
    for source in sources {
        if source.status == "available" && source.valid_count > 0 {
            if let Some(root) = &source.canonical_path {
                push(root);
            }
        }
    }
    for source in sources {
        for package in &source.packages {
            if package.state == "symlink" && package.canonical_package.is_some() {
                if let Some(root) = &package.link_target_root {
                    push(root);
                }
            }
        }
    }
    roots
}

fn valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_PACKAGE_NAME_BYTES
        && !name.contains(['/', '\\'])
        && name != "."
        && name != ".."
        && !name.chars().any(char::is_control)
}

fn metadata_is_link_like(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Writing `[skills]` in runner.toml
// ---------------------------------------------------------------------------

pub(crate) const RUNNER_CONFIG_MAX_BYTES: u64 = 256 * 1024;
const RUNNER_CONFIG_BACKUP_SUFFIX: &str = ".chadex-skills.bak";

/// System trees that can never be connected as a Skill root. `/` only matches
/// exactly; every other entry also covers its descendants.
pub(crate) const SYSTEM_SKILL_ROOT_DENYLIST: &[&str] = &[
    "/",
    "/System",
    "/Library",
    "/Applications",
    "/Volumes",
    "/bin",
    "/sbin",
    "/usr",
    "/etc",
    "/var",
    "/private",
    "/dev",
    "/proc",
    "/sys",
    "/run",
    "/boot",
    "/opt",
];

/// Credential stores under the home directory that are never Skill roots.
const HOME_CREDENTIAL_DIRS: &[&str] = &[".ssh", ".gnupg", ".aws", ".kube", ".docker"];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ExternalSkillRootsState {
    pub(crate) format: &'static str,
    pub(crate) roots: Vec<String>,
    pub(crate) script_roots: Vec<String>,
    /// SHA-256 of the whole runner.toml; pass it back as `expected_revision`.
    pub(crate) revision: String,
}

pub(crate) const EXTERNAL_SKILL_ROOTS_FORMAT: &str = "chadex.external_skill_roots.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootRejection {
    pub(crate) code: &'static str,
    pub(crate) path: Option<String>,
}

impl RootRejection {
    fn new(code: &'static str, path: Option<&Path>) -> Self {
        Self {
            code,
            path: path.map(|path| path.to_string_lossy().into_owned()),
        }
    }
}

pub(crate) fn read_runner_config(path: &Path) -> Result<String, &'static str> {
    let metadata = fs::symlink_metadata(path).map_err(|_| "runner_config_unavailable")?;
    if metadata_is_link_like(&metadata) || !metadata.is_file() {
        return Err("runner_config_unavailable");
    }
    if metadata.len() > RUNNER_CONFIG_MAX_BYTES {
        return Err("runner_config_too_large");
    }
    let bytes = read_bounded(path, RUNNER_CONFIG_MAX_BYTES as usize)
        .map_err(|_| "runner_config_unavailable")?;
    String::from_utf8(bytes).map_err(|_| "runner_config_unavailable")
}

pub(crate) fn runner_config_revision(content: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

pub(crate) fn current_skill_roots(content: &str) -> Result<ExternalSkillRootsState, &'static str> {
    #[derive(serde::Deserialize, Default)]
    struct Skills {
        #[serde(default)]
        roots: Vec<PathBuf>,
        #[serde(default)]
        script_roots: Vec<PathBuf>,
    }
    #[derive(serde::Deserialize)]
    struct Config {
        #[serde(default)]
        skills: Skills,
    }
    let config: Config = toml::from_str(content).map_err(|_| "runner_config_invalid")?;
    let strings = |paths: Vec<PathBuf>| {
        paths
            .into_iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    Ok(ExternalSkillRootsState {
        format: EXTERNAL_SKILL_ROOTS_FORMAT,
        roots: strings(config.skills.roots),
        script_roots: strings(config.skills.script_roots),
        revision: runner_config_revision(content),
    })
}

/// Allow-list check for roots Chadex is about to write. Every root must be an
/// existing, canonical (link-free) directory outside system trees, the home
/// directory itself and its credential stores; `script_roots` must be a subset
/// of `roots`. Shape rules are shared with the Runner.
pub(crate) fn validate_requested_roots(
    roots: &[PathBuf],
    script_roots: &[PathBuf],
    home: &Path,
    system_denylist: &[&str],
) -> Result<(), RootRejection> {
    chadex_runtime_runner_config::skills::validate_configured_skill_roots(roots, script_roots)
        .map_err(|_| RootRejection::new("skill_root_invalid", None))?;
    let home = home.canonicalize().unwrap_or_else(|_| home.to_path_buf());
    for root in roots {
        let metadata = fs::symlink_metadata(root)
            .map_err(|_| RootRejection::new("skill_root_not_found", Some(root)))?;
        if metadata_is_link_like(&metadata) {
            return Err(RootRejection::new("skill_root_is_link", Some(root)));
        }
        if !metadata.is_dir() {
            return Err(RootRejection::new("skill_root_not_directory", Some(root)));
        }
        let canonical = root
            .canonicalize()
            .map_err(|_| RootRejection::new("skill_root_not_found", Some(root)))?;
        // Compare text, not components: `Path` equality ignores `.` and
        // trailing separators, but runner.toml should hold the exact path.
        if canonical.as_os_str() != root.as_os_str() {
            return Err(RootRejection::new("skill_root_not_canonical", Some(root)));
        }
        let denied_system = system_denylist.iter().any(|denied| {
            let denied = Path::new(denied);
            if denied.parent().is_none() {
                canonical == denied
            } else {
                canonical.starts_with(denied)
            }
        });
        if denied_system || home.starts_with(&canonical) {
            return Err(RootRejection::new("skill_root_sensitive", Some(root)));
        }
        let in_credential_store = canonical
            .strip_prefix(&home)
            .ok()
            .and_then(|relative| relative.components().next())
            .is_some_and(|first| {
                HOME_CREDENTIAL_DIRS
                    .iter()
                    .any(|dir| first.as_os_str().eq_ignore_ascii_case(dir))
            });
        if in_credential_store || is_secret_path(&canonical.to_string_lossy()) {
            return Err(RootRejection::new("skill_root_sensitive", Some(root)));
        }
    }
    Ok(())
}

/// Replace only `skills.roots` / `skills.script_roots`, preserving every other
/// key, comment and table in the document.
pub(crate) fn render_skill_roots(
    content: &str,
    roots: &[PathBuf],
    script_roots: &[PathBuf],
) -> Result<String, &'static str> {
    let mut document = content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|_| "runner_config_invalid")?;
    if document.get("skills").is_none() {
        document["skills"] = toml_edit::table();
    }
    let skills = document["skills"]
        .as_table_like_mut()
        .ok_or("runner_config_invalid")?;
    let array = |paths: &[PathBuf]| {
        let mut array = toml_edit::Array::new();
        for path in paths {
            array.push(path.to_string_lossy().as_ref());
        }
        toml_edit::value(array)
    };
    skills.insert("roots", array(roots));
    if script_roots.is_empty() {
        skills.remove("script_roots");
    } else {
        skills.insert("script_roots", array(script_roots));
    }
    Ok(document.to_string())
}

pub(crate) fn runner_config_backup_path(config: &Path) -> PathBuf {
    let mut name = config
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(RUNNER_CONFIG_BACKUP_SUFFIX);
    config.with_file_name(name)
}

/// Write `candidate` only if runner.toml still holds `expected`; the previous
/// content is saved next to it first so a failed reload can be undone even
/// across a helper restart.
pub(crate) fn persist_if_unchanged(
    path: &Path,
    expected: &str,
    candidate: &str,
) -> Result<(), &'static str> {
    if read_runner_config(path)? != expected {
        return Err("runner_config_concurrent_change");
    }
    write_atomic(&runner_config_backup_path(path), expected.as_bytes())
        .map_err(|_| "runner_config_write_failed")?;
    if read_runner_config(path)? != expected {
        return Err("runner_config_concurrent_change");
    }
    write_atomic(path, candidate.as_bytes()).map_err(|_| "runner_config_write_failed")
}

/// Put `original` back if runner.toml still holds what Chadex wrote. Returns
/// `false` when someone else changed the file meanwhile (left untouched).
pub(crate) fn restore_if_unchanged(
    path: &Path,
    written: &str,
    original: &str,
) -> Result<bool, &'static str> {
    if read_runner_config(path)? != written {
        return Ok(false);
    }
    write_atomic(path, original.as_bytes()).map_err(|_| "runner_config_write_failed")?;
    Ok(true)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let parent = path.parent().ok_or(std::io::ErrorKind::InvalidInput)?;
    let name = path
        .file_name()
        .ok_or(std::io::ErrorKind::InvalidInput)?
        .to_string_lossy();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    // Keep the target name as the prefix so runner.toml temp files stay covered
    // by the secret-path rules even if a crash leaves one behind.
    let temp = parent.join(format!("{name}.{}.{nonce}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, package: &str, name: &str, scripts: bool) {
        let dir = root.join(package);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\n\nBody text.\n"),
        )
        .unwrap();
        if scripts {
            fs::create_dir_all(dir.join("scripts")).unwrap();
            fs::write(dir.join("scripts").join("run.sh"), "echo hi\n").unwrap();
        }
    }

    fn env_for(home: &Path) -> DiscoveryEnv {
        DiscoveryEnv {
            home: home.to_path_buf(),
            claude_config_dir: None,
            codex_home: None,
        }
    }

    fn source<'a>(discovery: &'a ExternalSkillDiscovery, kind: &str) -> &'a ExternalSkillSource {
        discovery
            .sources
            .iter()
            .find(|source| source.kind == kind)
            .unwrap()
    }

    #[test]
    fn missing_sources_are_reported_without_error() {
        let home = tempfile::tempdir().unwrap();
        let discovery = discover_external_skill_sources(&env_for(home.path()));
        assert_eq!(discovery.format, EXTERNAL_SKILL_DISCOVERY_FORMAT);
        assert_eq!(discovery.sources.len(), 3);
        assert!(discovery
            .sources
            .iter()
            .all(|source| source.status == "not_found"));
        assert!(discovery.recommended_roots.is_empty());
    }

    #[test]
    fn counts_valid_invalid_and_script_packages() {
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join(".agents/skills");
        write_skill(&agents, "alpha", "alpha", true);
        write_skill(&agents, "beta", "beta", false);
        fs::create_dir_all(agents.join("empty")).unwrap();
        fs::create_dir_all(agents.join("bad")).unwrap();
        fs::write(agents.join("bad/SKILL.md"), "no frontmatter\n").unwrap();
        fs::write(agents.join("README.md"), "not a package\n").unwrap();

        let discovery = discover_external_skill_sources(&env_for(home.path()));
        let agents_source = source(&discovery, "agents");
        assert_eq!(agents_source.status, "available");
        assert_eq!(agents_source.valid_count, 2);
        assert_eq!(agents_source.invalid_count, 2);
        assert_eq!(agents_source.script_count, 1);
        assert_eq!(agents_source.symlink_count, 0);
        let reasons: BTreeMap<_, _> = agents_source
            .packages
            .iter()
            .map(|package| (package.package.as_str(), package.invalid_reason))
            .collect();
        assert_eq!(reasons["empty"], Some("missing_skill_definition"));
        assert_eq!(reasons["bad"], Some("skill_frontmatter_missing"));
        assert_eq!(reasons["alpha"], None);
        let canonical = agents.canonicalize().unwrap();
        assert_eq!(
            discovery.recommended_roots,
            vec![canonical.to_string_lossy().into_owned()]
        );
    }

    #[test]
    fn metadata_only_no_skill_body_in_output() {
        let home = tempfile::tempdir().unwrap();
        write_skill(&home.path().join(".agents/skills"), "alpha", "alpha", false);
        let discovery = discover_external_skill_sources(&env_for(home.path()));
        let json = serde_json::to_string(&discovery).unwrap();
        assert!(json.contains("\"alpha skill\""));
        assert!(!json.contains("Body text."));
        assert!(!json.contains("canonical_package"));
    }

    #[test]
    fn oversized_definition_is_invalid() {
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join(".agents/skills");
        fs::create_dir_all(agents.join("huge")).unwrap();
        let mut text = String::from("---\nname: huge\ndescription: huge\n---\n");
        text.push_str(&"x".repeat(MAX_SKILL_DEFINITION_BYTES));
        fs::write(agents.join("huge/SKILL.md"), text).unwrap();
        let discovery = discover_external_skill_sources(&env_for(home.path()));
        let package = &source(&discovery, "agents").packages[0];
        assert_eq!(package.state, "invalid");
        assert_eq!(package.invalid_reason, Some("skill_definition_too_large"));
    }

    #[test]
    fn root_scan_limit_marks_source_truncated() {
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join(".agents/skills");
        fs::create_dir_all(&agents).unwrap();
        for index in 0..=MAX_ROOT_SCAN_ENTRIES {
            fs::write(agents.join(format!("f{index}")), "").unwrap();
        }
        let discovery = discover_external_skill_sources(&env_for(home.path()));
        let agents_source = source(&discovery, "agents");
        assert_eq!(agents_source.status, "scan_limit_exceeded");
        assert!(agents_source.truncated);
        assert!(agents_source.packages.is_empty());
    }

    #[test]
    fn package_limit_marks_source_truncated() {
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join(".agents/skills");
        for index in 0..=MAX_PACKAGES_PER_ROOT {
            fs::create_dir_all(agents.join(format!("p{index:04}"))).unwrap();
        }
        let discovery = discover_external_skill_sources(&env_for(home.path()));
        let agents_source = source(&discovery, "agents");
        assert!(agents_source.truncated);
        assert_eq!(agents_source.packages.len(), MAX_PACKAGES_PER_ROOT);
    }

    #[test]
    fn env_overrides_select_claude_and_codex_roots() {
        let home = tempfile::tempdir().unwrap();
        let claude = home.path().join("custom-claude");
        let codex = home.path().join("custom-codex");
        write_skill(&claude.join("skills"), "c", "c", false);
        write_skill(&codex.join("skills"), "x", "x", false);
        let env = DiscoveryEnv {
            home: home.path().to_path_buf(),
            claude_config_dir: Some(claude.clone()),
            codex_home: Some(codex.clone()),
        };
        let discovery = discover_external_skill_sources(&env);
        assert_eq!(source(&discovery, "claude").valid_count, 1);
        assert_eq!(source(&discovery, "codex").valid_count, 1);
        assert_eq!(
            source(&discovery, "claude").path,
            claude.join("skills").to_string_lossy()
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_packages_resolve_to_real_root_and_are_not_conflicts() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join(".agents/skills");
        let claude = home.path().join(".claude/skills");
        write_skill(&agents, "alpha", "alpha", true);
        fs::create_dir_all(&claude).unwrap();
        symlink(agents.join("alpha"), claude.join("alpha")).unwrap();
        symlink(home.path().join("nowhere"), claude.join("dangling")).unwrap();

        let discovery = discover_external_skill_sources(&env_for(home.path()));
        let claude_source = source(&discovery, "claude");
        let agents_canonical = agents.canonicalize().unwrap().to_string_lossy().into_owned();
        assert_eq!(claude_source.valid_count, 0);
        assert_eq!(claude_source.symlink_count, 2);
        assert_eq!(claude_source.script_count, 0);
        assert_eq!(claude_source.provided_by, vec![agents_canonical.clone()]);
        let linked = claude_source
            .packages
            .iter()
            .find(|package| package.package == "alpha")
            .unwrap();
        assert_eq!(linked.state, "symlink");
        assert_eq!(linked.name.as_deref(), Some("alpha"));
        assert!(linked.has_scripts);
        assert!(!linked.name_conflict);
        let dangling = claude_source
            .packages
            .iter()
            .find(|package| package.package == "dangling")
            .unwrap();
        assert_eq!(dangling.state, "symlink");
        assert_eq!(dangling.invalid_reason, Some("invalid_skill_package"));
        assert_eq!(discovery.recommended_roots, vec![agents_canonical]);
    }

    #[cfg(unix)]
    #[test]
    fn link_only_source_recommends_its_target_root() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let store = home.path().join("dev/skill-store");
        let claude = home.path().join(".claude/skills");
        write_skill(&store, "gamma", "gamma", false);
        fs::create_dir_all(&claude).unwrap();
        symlink(store.join("gamma"), claude.join("gamma")).unwrap();
        let discovery = discover_external_skill_sources(&env_for(home.path()));
        assert_eq!(
            discovery.recommended_roots,
            vec![store.canonicalize().unwrap().to_string_lossy().into_owned()]
        );
    }

    #[cfg(unix)]
    #[test]
    fn linked_root_reports_canonical_path_and_duplicates() {
        use std::os::unix::fs::symlink;
        let home = tempfile::tempdir().unwrap();
        let agents = home.path().join(".agents/skills");
        write_skill(&agents, "alpha", "alpha", false);
        fs::create_dir_all(home.path().join(".codex")).unwrap();
        symlink(&agents, home.path().join(".codex/skills")).unwrap();
        let discovery = discover_external_skill_sources(&env_for(home.path()));
        let codex = source(&discovery, "codex");
        assert!(codex.root_is_link);
        assert_eq!(codex.status, "duplicate_source");
        assert_eq!(codex.same_as, Some("agents"));
        assert!(codex.packages.is_empty());
        assert_eq!(discovery.recommended_roots.len(), 1);
    }

    #[test]
    fn same_name_in_different_packages_is_a_conflict() {
        let home = tempfile::tempdir().unwrap();
        write_skill(&home.path().join(".agents/skills"), "one", "Review", false);
        write_skill(&home.path().join(".codex/skills"), "two", "review", false);
        let discovery = discover_external_skill_sources(&env_for(home.path()));
        assert!(source(&discovery, "agents").packages[0].name_conflict);
        assert!(source(&discovery, "codex").packages[0].name_conflict);
        assert_eq!(discovery.recommended_roots.len(), 2);
    }

    #[test]
    fn home_and_filesystem_root_are_never_recommended() {
        let home = tempfile::tempdir().unwrap();
        let canonical_home = home.path().canonicalize().unwrap();
        let sources = vec![ExternalSkillSource {
            kind: "agents",
            path: String::new(),
            canonical_path: Some(canonical_home.to_string_lossy().into_owned()),
            status: "available",
            root_is_link: false,
            same_as: None,
            valid_count: 1,
            symlink_count: 0,
            invalid_count: 0,
            script_count: 0,
            truncated: false,
            provided_by: Vec::new(),
            packages: vec![ExternalSkillPackage {
                package: "x".into(),
                state: "symlink",
                name: Some("x".into()),
                description: None,
                has_scripts: false,
                link_target_root: Some("/".into()),
                invalid_reason: None,
                name_conflict: false,
                canonical_package: Some(PathBuf::from("/x")),
            }],
        }];
        assert!(recommended_roots(&sources, home.path()).is_empty());
    }

    // --- runner.toml writes -------------------------------------------------

    const SAMPLE_CONFIG: &str = "# operator comment\nserver_url = \"http://127.0.0.1:1\"\nclient_id = \"c\"\n\n[policy]\nallowed_roots = [\"/p\"]\n";

    #[test]
    fn render_preserves_other_settings_and_comments() {
        let rendered = render_skill_roots(
            SAMPLE_CONFIG,
            &[PathBuf::from("/s/a"), PathBuf::from("/s/b")],
            &[PathBuf::from("/s/a")],
        )
        .unwrap();
        assert!(rendered.starts_with(SAMPLE_CONFIG));
        let state = current_skill_roots(&rendered).unwrap();
        assert_eq!(state.roots, vec!["/s/a", "/s/b"]);
        assert_eq!(state.script_roots, vec!["/s/a"]);
        let cleared = render_skill_roots(&rendered, &[PathBuf::from("/s/a")], &[]).unwrap();
        let state = current_skill_roots(&cleared).unwrap();
        assert_eq!(state.roots, vec!["/s/a"]);
        assert!(state.script_roots.is_empty());
        assert!(!cleared.contains("script_roots"));
        assert!(cleared.contains("# operator comment"));
        assert!(cleared.contains("allowed_roots = [\"/p\"]"));
    }

    #[test]
    fn render_keeps_other_skills_keys() {
        let content = "[skills]\nroots = [\"/old\"]\nfuture_key = 1\n";
        let rendered = render_skill_roots(content, &[PathBuf::from("/new")], &[]).unwrap();
        assert!(rendered.contains("future_key = 1"));
        assert_eq!(current_skill_roots(&rendered).unwrap().roots, vec!["/new"]);
    }

    #[test]
    fn persist_is_compare_and_swap_with_backup_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runner.toml");
        fs::write(&path, SAMPLE_CONFIG).unwrap();
        let candidate = render_skill_roots(SAMPLE_CONFIG, &[PathBuf::from("/s")], &[]).unwrap();

        assert_eq!(
            persist_if_unchanged(&path, "stale", &candidate),
            Err("runner_config_concurrent_change")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), SAMPLE_CONFIG);

        persist_if_unchanged(&path, SAMPLE_CONFIG, &candidate).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), candidate);
        assert_eq!(
            fs::read_to_string(runner_config_backup_path(&path)).unwrap(),
            SAMPLE_CONFIG
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let leftovers = fs::read_dir(dir.path())
            .unwrap()
            .filter(|entry| entry.as_ref().unwrap().file_name().to_string_lossy().ends_with(".tmp"))
            .count();
        assert_eq!(leftovers, 0);

        assert!(restore_if_unchanged(&path, &candidate, SAMPLE_CONFIG).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), SAMPLE_CONFIG);
        // A concurrent operator edit is never clobbered by rollback.
        fs::write(&path, "server_url = \"x\"\n").unwrap();
        assert!(!restore_if_unchanged(&path, &candidate, SAMPLE_CONFIG).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "server_url = \"x\"\n");
    }

    #[test]
    fn revision_tracks_whole_file() {
        let state = current_skill_roots(SAMPLE_CONFIG).unwrap();
        assert!(state.roots.is_empty());
        assert_eq!(state.revision, runner_config_revision(SAMPLE_CONFIG));
        assert_ne!(state.revision, runner_config_revision("x = 1\n"));
    }

    #[cfg(unix)]
    #[test]
    fn rejects_links_relative_missing_and_sensitive_roots() {
        use std::os::unix::fs::symlink;
        let base = tempfile::tempdir().unwrap();
        let home = base.path().canonicalize().unwrap().join("home");
        let skills = home.join(".agents/skills");
        fs::create_dir_all(&skills).unwrap();
        fs::create_dir_all(home.join(".ssh/skills")).unwrap();
        symlink(&skills, home.join("linked")).unwrap();
        let deny: &[&str] = &["/", "/System", "/etc"];
        let check = |roots: &[PathBuf], scripts: &[PathBuf]| {
            validate_requested_roots(roots, scripts, &home, deny).map_err(|error| error.code)
        };

        assert_eq!(check(&[skills.clone()], &[skills.clone()]), Ok(()));
        assert_eq!(check(&[skills.clone()], &[home.clone()]), Err("skill_root_invalid"));
        assert_eq!(check(&[PathBuf::from("rel")], &[]), Err("skill_root_invalid"));
        assert_eq!(check(&[home.join("missing")], &[]), Err("skill_root_not_found"));
        assert_eq!(check(&[home.join("linked")], &[]), Err("skill_root_is_link"));
        assert_eq!(
            check(&[home.join(".agents/./skills")], &[]),
            Err("skill_root_not_canonical")
        );
        assert_eq!(check(&[home.clone()], &[]), Err("skill_root_sensitive"));
        assert_eq!(check(&[home.parent().unwrap().to_path_buf()], &[]), Err("skill_root_sensitive"));
        assert_eq!(check(&[home.join(".ssh/skills")], &[]), Err("skill_root_sensitive"));
        assert_eq!(check(&[PathBuf::from("/")], &[]), Err("skill_root_sensitive"));
        // `/etc` is itself a link on macOS; either way it is refused.
        assert!(check(&[PathBuf::from("/etc")], &[]).is_err());
    }

    #[test]
    fn production_denylist_covers_system_trees() {
        let tmp = tempfile::tempdir().unwrap();
        // macOS temp dirs live under /private/var, which must be refused.
        let canonical = tmp.path().canonicalize().unwrap();
        if canonical.starts_with("/private") || canonical.starts_with("/var") {
            assert_eq!(
                validate_requested_roots(
                    &[canonical.clone()],
                    &[],
                    Path::new("/nonexistent-home"),
                    SYSTEM_SKILL_ROOT_DENYLIST
                )
                .map_err(|error| error.code),
                Err("skill_root_sensitive")
            );
        }
    }
}
