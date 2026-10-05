//! Path spelling helpers for Skill root discovery and validation.
//!
//! On Windows `Path::canonicalize` returns extended-length (`\\?\C:\...`)
//! paths. The Runner, the UI and `runner.toml` all use the ordinary spelling,
//! so canonical results are converted back when that is lossless, and Windows
//! comparisons ignore case. Everything here is a no-op or byte-identical on
//! Unix.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// Longest simplified path (UTF-16 units) that is still handed out without the
/// `\\?\` prefix. `MAX_PATH` is 260, but directory APIs reserve room for a
/// 8.3 name, so stay below 248; longer paths keep their verbatim spelling.
const MAX_SIMPLIFIED_UTF16_LEN: usize = 248;

/// `Path::canonicalize` with the Windows verbatim prefix removed when safe.
pub(super) fn canonicalize(path: &Path) -> io::Result<PathBuf> {
    path.canonicalize().map(simplify_verbatim)
}

pub(super) fn simplify_verbatim(path: PathBuf) -> PathBuf {
    // `\\?\` is an ordinary (if odd) file name on Unix; never touch it there.
    if !cfg!(windows) {
        return path;
    }
    match path.to_str().and_then(simplify_verbatim_text) {
        Some(simplified) => PathBuf::from(simplified),
        None => path,
    }
}

/// `\\?\C:\a` -> `C:\a` and `\\?\UNC\server\share\a` -> `\\server\share\a`.
/// Returns `None` (keep the verbatim spelling) when the input is not one of
/// those two forms or when dropping the prefix could change what the path
/// means: Win32 normalisation would rewrite `.`/`..`/`/`, trailing dots or
/// spaces and device names, and the result would exceed the legacy length
/// limit.
fn simplify_verbatim_text(text: &str) -> Option<String> {
    let unc_prefix = text
        .get(..8)
        .filter(|head| head.eq_ignore_ascii_case(r"\\?\UNC\"));
    let simplified = if unc_prefix.is_some() {
        let rest = &text[8..];
        let mut parts = rest.splitn(3, '\\');
        let server = parts.next()?;
        let share = parts.next()?;
        if !component_is_plain(server) || !component_is_plain(share) {
            return None;
        }
        if !tail_is_plain(parts.next().unwrap_or("")) {
            return None;
        }
        format!(r"\\{rest}")
    } else {
        let rest = text.strip_prefix(r"\\?\")?;
        let bytes = rest.as_bytes();
        if bytes.len() < 3
            || !bytes[0].is_ascii_alphabetic()
            || bytes[1] != b':'
            || bytes[2] != b'\\'
        {
            return None;
        }
        if !tail_is_plain(&rest[3..]) {
            return None;
        }
        rest.to_string()
    };
    (simplified.encode_utf16().count() < MAX_SIMPLIFIED_UTF16_LEN).then_some(simplified)
}

/// Everything after the drive/share root: empty, or `\`-separated plain
/// components with no empty segment.
fn tail_is_plain(tail: &str) -> bool {
    tail.is_empty() || tail.split('\\').all(component_is_plain)
}

fn component_is_plain(component: &str) -> bool {
    if component.is_empty()
        || component == "."
        || component == ".."
        || component.ends_with(' ')
        || component.ends_with('.')
        || component
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '|' | '?' | '*'))
    {
        return false;
    }
    !is_reserved_device_name(component)
}

fn is_reserved_device_name(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM", "LPT"].iter().any(|device| {
        stem.strip_prefix(device).is_some_and(|number| {
            matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
                || matches!(number, "\u{b9}" | "\u{b2}" | "\u{b3}")
        })
    })
}

// ---------------------------------------------------------------------------
// Comparisons: case-insensitive on Windows, byte-exact elsewhere.
// ---------------------------------------------------------------------------

fn components_match(a: std::path::Component<'_>, b: std::path::Component<'_>, fold: bool) -> bool {
    a == b
        || (fold
            && a.as_os_str().to_string_lossy().to_lowercase()
                == b.as_os_str().to_string_lossy().to_lowercase())
}

fn strip_prefix_with(path: &Path, base: &Path, fold: bool) -> Option<PathBuf> {
    let mut remaining = path.components();
    for expected in base.components() {
        if !components_match(remaining.next()?, expected, fold) {
            return None;
        }
    }
    Some(remaining.as_path().to_path_buf())
}

/// Component-wise `strip_prefix`.
pub(super) fn strip_prefix(path: &Path, base: &Path) -> Option<PathBuf> {
    strip_prefix_with(path, base, cfg!(windows))
}

/// Component-wise `starts_with` (`path == base` counts).
pub(super) fn starts_with(path: &Path, base: &Path) -> bool {
    strip_prefix(path, base).is_some()
}

/// Component-wise equality.
pub(super) fn same_path(a: &Path, b: &Path) -> bool {
    strip_prefix(a, b).is_some_and(|rest| rest.as_os_str().is_empty())
}

/// Spelling equality: like `a.as_os_str() == b.as_os_str()` on Unix; on
/// Windows the same text ignoring case.
pub(super) fn same_text(a: &Path, b: &Path) -> bool {
    same_text_with(a, b, cfg!(windows))
}

fn same_text_with(a: &Path, b: &Path, fold: bool) -> bool {
    a.as_os_str() == b.as_os_str()
        || (fold && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase())
}

// ---------------------------------------------------------------------------
// Windows system trees
// ---------------------------------------------------------------------------

/// `%SystemRoot%`, `%ProgramFiles%`, `%ProgramFiles(x86)%` and `%ProgramData%`
/// with their stock locations as fallbacks for unset or non-absolute values.
/// Drive roots are rejected separately (any path without a parent).
pub(super) fn windows_system_roots(get_env: impl Fn(&str) -> Option<OsString>) -> Vec<String> {
    let value = |names: &[&str], fallback: &str| {
        names
            .iter()
            .filter_map(|name| get_env(name))
            .map(|value| value.to_string_lossy().into_owned())
            .find(|value| looks_like_absolute_windows_path(value))
            .unwrap_or_else(|| fallback.to_string())
    };
    vec![
        value(&["SystemRoot", "windir"], r"C:\Windows"),
        value(&["ProgramFiles"], r"C:\Program Files"),
        value(&["ProgramFiles(x86)"], r"C:\Program Files (x86)"),
        value(&["ProgramData"], r"C:\ProgramData"),
    ]
}

fn looks_like_absolute_windows_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    (bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/'))
        || text.starts_with(r"\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simplified(text: &str) -> Option<String> {
        simplify_verbatim_text(text)
    }

    #[test]
    fn verbatim_drive_and_unc_prefixes_are_removed() {
        assert_eq!(simplified(r"\\?\C:\Users\me").as_deref(), Some(r"C:\Users\me"));
        assert_eq!(simplified(r"\\?\c:\").as_deref(), Some(r"c:\"));
        assert_eq!(
            simplified(r"\\?\UNC\server\share\skills").as_deref(),
            Some(r"\\server\share\skills")
        );
        assert_eq!(
            simplified(r"\\?\UNC\server\share").as_deref(),
            Some(r"\\server\share")
        );
        assert_eq!(
            simplified(r"\\?\C:\Users\空間\skills").as_deref(),
            Some(r"C:\Users\空間\skills")
        );
    }

    #[test]
    fn unsafe_or_foreign_verbatim_forms_are_kept() {
        for text in [
            r"C:\already\plain",
            r"\\server\share",
            r"/unix/path",
            r"\\?\C:",
            r"\\?\C:foo",
            r"\\?\1:\x",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\x",
            r"\\?\GLOBALROOT\Device\x",
            r"\\.\C:\x",
            r"\\?\UNC\server",
            r"\\?\UNC\\share\x",
        ] {
            assert_eq!(simplified(text), None, "{text}");
        }
    }

    #[test]
    fn verbatim_paths_win32_would_rewrite_are_kept() {
        for text in [
            r"\\?\C:\a\..\b",
            r"\\?\C:\a\.\b",
            r"\\?\C:\a\\b",
            r"\\?\C:\a\b\",
            r"\\?\C:\a/b",
            r"\\?\C:\trailing.\x",
            r"\\?\C:\trailing \x",
            r"\\?\C:\bad<name",
            r"\\?\C:\ads:stream",
            r"\\?\C:\a\NUL",
            r"\\?\C:\a\nul.txt",
            r"\\?\C:\a\com1",
            r"\\?\C:\a\LPT9.log",
            r"\\?\C:\a\CONIN$",
            r"\\?\UNC\server\share\a\..\b",
            r"\\?\UNC\ser:ver\share",
        ] {
            assert_eq!(simplified(text), None, "{text}");
        }
        // Names that merely resemble devices are fine.
        assert!(simplified(r"\\?\C:\a\com0").is_some());
        assert!(simplified(r"\\?\C:\a\console").is_some());
        assert!(simplified(r"\\?\C:\a\nullable.txt").is_some());
    }

    #[test]
    fn long_verbatim_paths_keep_their_prefix() {
        let ok = format!(r"\\?\C:\{}", "a".repeat(200));
        assert!(simplified(&ok).is_some());
        let long = format!(r"\\?\C:\{}\{}", "a".repeat(120), "b".repeat(130));
        assert_eq!(simplified(&long), None);
    }

    #[test]
    fn simplify_never_rewrites_unix_paths() {
        if cfg!(windows) {
            return;
        }
        let odd = PathBuf::from(r"\\?\C:\looks\like\verbatim");
        assert_eq!(simplify_verbatim(odd.clone()), odd);
    }

    #[test]
    fn canonicalize_matches_std_apart_from_the_verbatim_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let std_path = dir.path().canonicalize().unwrap();
        let ours = canonicalize(dir.path()).unwrap();
        if cfg!(windows) {
            assert!(!ours.to_string_lossy().starts_with(r"\\?\"));
            assert!(std_path.to_string_lossy().starts_with(r"\\?\"));
        } else {
            assert_eq!(ours, std_path);
        }
        assert!(canonicalize(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn folded_comparisons_ignore_case_only_when_asked() {
        let upper = Path::new("/Users/Me/Skills");
        let lower = Path::new("/users/me/skills");
        assert_eq!(strip_prefix_with(upper, lower, false), None);
        assert_eq!(
            strip_prefix_with(Path::new("/Users/Me/Skills/a"), Path::new("/users/me"), true),
            Some(PathBuf::from("Skills/a"))
        );
        assert_eq!(
            strip_prefix_with(Path::new("/Users/Me"), Path::new("/users/me/skills"), true),
            None
        );
        assert!(!same_text_with(upper, lower, false));
        assert!(same_text_with(upper, lower, true));
        assert!(!same_text_with(Path::new("/a/b"), Path::new("/a/b/"), true));
    }

    #[test]
    fn exact_comparisons_match_std_on_this_platform() {
        let a = Path::new("/tmp/skills");
        assert!(same_path(a, Path::new("/tmp/skills")));
        assert!(starts_with(Path::new("/tmp/skills/x"), a));
        assert!(!starts_with(Path::new("/tmp/skills-two"), a));
        assert!(!starts_with(Path::new("/tmp"), a));
        assert_eq!(
            strip_prefix(Path::new("/tmp/skills/x/y"), a),
            Some(PathBuf::from("x/y"))
        );
        if !cfg!(windows) {
            assert!(!same_path(a, Path::new("/TMP/skills")));
            assert!(!same_text(a, Path::new("/TMP/skills")));
        }
    }

    #[test]
    fn windows_system_roots_use_environment_then_fallbacks() {
        let env = |name: &str| -> Option<OsString> {
            match name {
                "SystemRoot" => Some(r"D:\WINNT".into()),
                "ProgramFiles" => Some("relative".into()),
                "ProgramFiles(x86)" => Some(OsString::new()),
                "ProgramData" => Some(r"E:\Data".into()),
                _ => None,
            }
        };
        assert_eq!(
            windows_system_roots(env),
            vec![
                r"D:\WINNT",
                r"C:\Program Files",
                r"C:\Program Files (x86)",
                r"E:\Data"
            ]
        );
        assert_eq!(
            windows_system_roots(|name| (name == "windir").then(|| r"F:\Win".into()))[0],
            r"F:\Win"
        );
        assert_eq!(
            windows_system_roots(|_| None),
            vec![
                r"C:\Windows",
                r"C:\Program Files",
                r"C:\Program Files (x86)",
                r"C:\ProgramData"
            ]
        );
    }
}
