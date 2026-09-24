//! Small string-matching helper shared by Runner error classifiers.

/// Return `true` if `haystack` contains any of `needles` as a substring.
///
/// Shared by Runner transport/error classifiers; it has no process-resolution
/// semantics and remains Runner-owned.
pub(crate) fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}
