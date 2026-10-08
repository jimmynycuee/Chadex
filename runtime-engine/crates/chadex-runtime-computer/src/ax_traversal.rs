//! Platform-neutral Accessibility traversal engine.
//!
//! The engine owns every decision that matters for safety and bounds: which
//! attributes may be read for which node, how deep and how long a walk may go,
//! how element handles (`ElementRecord`) are derived, and how they are resolved
//! back to a live node. The platform only supplies an [`AxSource`] that reads one
//! attribute at a time, so the whole engine can be exercised with a fake tree.
//!
//! Sensitive-content rules enforced here (never delegated to the source):
//! - `AXValue` is requested only for nodes that are neither protected nor secure.
//! - `Query` mode (subtree/find) additionally treats the descendants of a secure
//!   text field as protected: their title/description/placeholder/value are never
//!   read. `Legacy` mode keeps the historical behavior of the original tree.

use crate::{
    allocate_selector, ensure_correlated_fingerprint, ensure_queryable_root,
    is_secure_text_fingerprint, AccessibilityTreeResult, ElementFingerprint, ElementRecord,
    MAX_ACCESSIBILITY_ABSOLUTE_DEPTH, MAX_TEXT_BYTES,
};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::Arc;
use crate::web_accessibility::WebProbe;
use std::time::Duration;

/// Longest `AXValue` prefix a deep search compares against. Tree output keeps the
/// historical `MAX_TEXT_BYTES` bound; searching reads further because web text
/// frequently lives in long `AXStaticText` values.
pub(crate) const MAX_FIND_VALUE_BYTES: usize = 4096;
const PROBE_MAX_DEPTH: usize = 12;
const PROBE_MAX_VISITED: usize = 200;
const ANCESTORS_MAX_BYTES: usize = 256;
const ANCESTOR_TITLE_MAX_BYTES: usize = 40;
const ANCESTORS_TRUNCATION_PREFIX: &str = "… › ";
const ANCESTORS_SEPARATOR: &str = " › ";

/// One-attribute-at-a-time view of a native Accessibility tree.
pub(crate) trait AxSource {
    type Node: Clone;

    fn platform(&self) -> &'static str;
    /// Reads role/protected/subrole/identifier and, only when the node is not
    /// protected, title/description/placeholder. `inherited_protected` forces the
    /// protected treatment (no title/description/placeholder reads).
    fn fingerprint(
        &self,
        node: &Self::Node,
        inherited_protected: bool,
    ) -> Result<ElementFingerprint, String>;
    /// The `AXRole` alone; the cheap read used by web-content probing.
    fn role(&self, node: &Self::Node) -> Result<Option<String>, String>;
    /// Only ever called for nodes that are neither protected nor secure.
    fn value(&self, node: &Self::Node, max_bytes: usize) -> Result<Option<String>, String>;
    fn enabled(&self, node: &Self::Node) -> Result<Option<bool>, String>;
    fn focused(&self, node: &Self::Node) -> Result<Option<bool>, String>;
    fn child_count(&self, node: &Self::Node) -> Result<usize, String>;
    fn children(&self, node: &Self::Node, take: usize) -> Result<Vec<Self::Node>, String>;
    fn child_at(&self, node: &Self::Node, index: usize) -> Result<Self::Node, String>;
    fn check_deadline(&self) -> Result<(), String>;
}

/// Monotonic time since the start of the whole observation (including any wait
/// spent enabling web accessibility), so the soft budget never stretches.
pub(crate) trait AxClock {
    fn elapsed(&self) -> Duration;
}

fn is_sensitive(fingerprint: &ElementFingerprint) -> bool {
    fingerprint.protected || is_secure_text_fingerprint(fingerprint)
}

// ---------------------------------------------------------------------------
// resolve
// ---------------------------------------------------------------------------

/// Re-walks `record.path` from `window_root`, verifying every fingerprint on the
/// way. Any drift is `stale_element`.
pub(crate) fn resolve<S: AxSource>(
    source: &S,
    window_root: S::Node,
    record: &ElementRecord,
) -> Result<S::Node, String> {
    if record.lineage.len() != record.path.len() + 1 {
        return Err("stale_element: AX element correlation lineage is incomplete".to_string());
    }
    if record.path.len() > MAX_ACCESSIBILITY_ABSOLUTE_DEPTH {
        return Err("stale_element: AX element path exceeds the bounded depth".to_string());
    }
    let mut current = window_root;
    let current_root_fingerprint = source.fingerprint(&current, false)?;
    ensure_correlated_fingerprint(&record.lineage[0], &current_root_fingerprint, true)?;
    for (depth, &index) in record.path.iter().enumerate() {
        let child_count = source.child_count(&current)?;
        if index >= child_count {
            return Err("stale_element: AX child path no longer exists".to_string());
        }
        current = source.child_at(&current, index)?;
        let parent = &record.lineage[depth];
        let expected = &record.lineage[depth + 1];
        // A child below a secure field is protected only when the record says so:
        // records minted by the hardened (`Query`) path carry that, records minted
        // by the legacy tree do not, and both must re-resolve to themselves.
        let inherited = parent.protected || (is_secure_text_fingerprint(parent) && expected.protected);
        let current_fingerprint = source.fingerprint(&current, inherited)?;
        ensure_correlated_fingerprint(expected, &current_fingerprint, false)?;
    }
    Ok(current)
}

// ---------------------------------------------------------------------------
// web content probe
// ---------------------------------------------------------------------------

/// Cheap readiness check used after enabling web accessibility: is there an
/// `AXWebArea` that already has children? Reads only role and child counts, never any
/// text, and stays within 12 levels / 200 nodes. An inconclusive search (budget cut)
/// counts as "still empty" so the caller keeps waiting instead of assuming absence.
pub(crate) fn probe_web_content<S: AxSource>(
    source: &S,
    window: S::Node,
) -> Result<WebProbe, String> {
    let mut queue = VecDeque::from([(window, 0usize)]);
    let mut visited = 0usize;
    let mut saw_empty_web_area = false;
    let mut inconclusive = false;
    while let Some((node, depth)) = queue.pop_front() {
        source.check_deadline()?;
        if visited >= PROBE_MAX_VISITED {
            inconclusive = true;
            break;
        }
        visited += 1;
        let is_web_area = source.role(&node)?.as_deref() == Some("AXWebArea");
        let child_count = source.child_count(&node)?;
        if is_web_area {
            if child_count > 0 {
                return Ok(WebProbe::Content);
            }
            saw_empty_web_area = true;
            continue;
        }
        if child_count == 0 {
            continue;
        }
        if depth >= PROBE_MAX_DEPTH {
            inconclusive = true;
            continue;
        }
        let remaining = PROBE_MAX_VISITED.saturating_sub(visited + queue.len());
        let take = child_count.min(remaining);
        if take < child_count {
            inconclusive = true;
        }
        if take > 0 {
            for child in source.children(&node, take)? {
                queue.push_back((child, depth + 1));
            }
        }
    }
    Ok(if saw_empty_web_area || inconclusive {
        WebProbe::Empty
    } else {
        WebProbe::NoWebArea
    })
}

// ---------------------------------------------------------------------------
// tree observation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TreeMode {
    /// Historical `computer_accessibility_tree` behavior.
    Legacy,
    /// `computer_accessibility_subtree` with a root: secure fields protect their
    /// descendants and the output carries a `root` descriptor.
    Query,
    /// `computer_accessibility_subtree` without a root: exactly the `Legacy` walk (same
    /// sensitive-content handling, same nodes and records), only reported through the
    /// subtree wire shape (`root: null`).
    LegacyRootless,
}

/// The walk semantics of a subtree request: only a rooted query is hardened, so the
/// model-facing `accessibility_tree` keeps its historical behavior when no root is given.
pub(crate) fn subtree_mode(root: Option<&ElementRecord>) -> TreeMode {
    if root.is_some() {
        TreeMode::Query
    } else {
        TreeMode::LegacyRootless
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TreeBounds {
    pub(crate) max_depth: usize,
    pub(crate) max_nodes: usize,
}

struct PendingNode<N> {
    node: N,
    parent_element_id: Option<String>,
    depth: usize,
    path: Vec<usize>,
    /// Ancestor fingerprints, excluding the node itself.
    lineage: Vec<Arc<ElementFingerprint>>,
    inherited_protected: bool,
    /// Fingerprint already verified by a resolve (the query root).
    known_fingerprint: Option<Arc<ElementFingerprint>>,
}

/// Breadth-first observation. `depth` and `max_nodes` are relative to `root`.
/// With `prefix`, `root` must be the live node that `prefix` resolved to.
pub(crate) fn observe_tree<S: AxSource>(
    source: &S,
    root: S::Node,
    surface_id: &str,
    prefix: Option<&ElementRecord>,
    bounds: TreeBounds,
    mode: TreeMode,
) -> Result<AccessibilityTreeResult, String> {
    let (root_path, root_lineage, root_known) = match prefix {
        Some(prefix) => {
            ensure_queryable_root(prefix)?;
            let mut lineage = prefix.lineage.clone();
            let known = lineage.pop();
            (prefix.path.clone(), lineage, known)
        }
        None => (Vec::new(), Vec::new(), None),
    };
    let root_absolute_depth = root_path.len();
    let TreeBounds {
        max_depth,
        max_nodes,
    } = bounds;
    let mut queue = VecDeque::from([PendingNode {
        node: root,
        parent_element_id: None,
        depth: 0,
        path: root_path,
        lineage: root_lineage,
        inherited_protected: false,
        known_fingerprint: root_known,
    }]);
    let mut nodes = Vec::with_capacity(max_nodes.min(64));
    let mut elements: Vec<(String, ElementRecord)> = Vec::with_capacity(max_nodes.min(64));
    let mut truncated = false;
    while let Some(PendingNode {
        node,
        parent_element_id,
        depth,
        path,
        mut lineage,
        inherited_protected,
        known_fingerprint,
    }) = queue.pop_front()
    {
        source.check_deadline()?;
        if nodes.len() >= max_nodes {
            truncated = true;
            break;
        }
        let element_id = allocate_selector("element_", |id| {
            elements.iter().any(|(existing, _)| existing == id)
        })?;
        let fingerprint = match known_fingerprint {
            Some(known) => known,
            None => Arc::new(source.fingerprint(&node, inherited_protected)?),
        };
        let secure = is_secure_text_fingerprint(&fingerprint);
        let protected = fingerprint.protected;
        let role = fingerprint.role.clone();
        let subrole = fingerprint.subrole.clone();
        let title = fingerprint.title.clone();
        let description = fingerprint.description.clone();
        let placeholder = fingerprint.placeholder.clone();
        lineage.push(fingerprint);
        let value = if secure || protected {
            None
        } else {
            source.value(&node, MAX_TEXT_BYTES)?
        };
        let enabled = source.enabled(&node)?;
        let focused = source.focused(&node)?;
        let child_count = source.child_count(&node)?;
        let absolute_depth = root_absolute_depth + depth;
        if depth < max_depth && absolute_depth < MAX_ACCESSIBILITY_ABSOLUTE_DEPTH && child_count > 0
        {
            let reserved = nodes.len() + queue.len() + 1;
            let remaining = max_nodes.saturating_sub(reserved);
            let take = child_count.min(remaining);
            if take < child_count {
                truncated = true;
            }
            let child_inherited = match mode {
                TreeMode::Legacy | TreeMode::LegacyRootless => protected,
                TreeMode::Query => protected || secure,
            };
            for (index, child) in source.children(&node, take)?.into_iter().enumerate() {
                let mut child_path = path.clone();
                child_path.push(index);
                queue.push_back(PendingNode {
                    node: child,
                    parent_element_id: Some(element_id.clone()),
                    depth: depth + 1,
                    path: child_path,
                    lineage: lineage.clone(),
                    inherited_protected: child_inherited,
                    known_fingerprint: None,
                });
            }
        } else if child_count > 0 {
            truncated = true;
        }
        elements.push((
            element_id.clone(),
            ElementRecord {
                surface_id: surface_id.to_string(),
                path,
                lineage,
            },
        ));
        nodes.push(json!({
            "element_id": element_id,
            "parent_element_id": parent_element_id,
            "depth": depth,
            "role": role,
            "subrole": subrole,
            "title": title,
            "description": description,
            "value": value,
            "placeholder": placeholder,
            "enabled": enabled,
            "focused": focused,
            "child_count": child_count,
        }));
    }
    if !queue.is_empty() {
        truncated = true;
    }
    source.check_deadline()?;
    let node_count = nodes.len();
    let root_descriptor = match (mode, nodes.first()) {
        (TreeMode::Query, Some(first)) if prefix.is_some() => Some(json!({
            "element_id": first["element_id"].clone(),
            "absolute_depth": root_absolute_depth,
        })),
        _ => None,
    };
    let mut output = json!({
        "platform": source.platform(),
        "surface_id": surface_id,
        "nodes": nodes,
        "node_count": node_count,
        "truncated": truncated,
        "max_depth": max_depth,
        "max_nodes": max_nodes,
    });
    if matches!(mode, TreeMode::Query | TreeMode::LegacyRootless) {
        output["root"] = root_descriptor.unwrap_or(Value::Null);
    }
    Ok(AccessibilityTreeResult { output, elements })
}

// ---------------------------------------------------------------------------
// deep find
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FindQuery {
    pub(crate) role: Option<String>,
    pub(crate) subrole: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) focused: Option<bool>,
    pub(crate) enabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FindBounds {
    pub(crate) limit: usize,
    /// Relative to the search root.
    pub(crate) max_depth: usize,
    pub(crate) max_visited: usize,
    pub(crate) max_children_per_node: usize,
    pub(crate) soft_budget: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FindStopReason {
    Complete,
    Limit,
    VisitBudget,
    TimeBudget,
    DepthBound,
}

impl FindStopReason {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Limit => "limit",
            Self::VisitBudget => "visit_budget",
            Self::TimeBudget => "time_budget",
            Self::DepthBound => "depth_bound",
        }
    }
}

struct Visited {
    parent: Option<u32>,
    child_index: u32,
    depth: usize,
    fingerprint: Arc<ElementFingerprint>,
    sensitive: bool,
}

struct QueuedNode<N> {
    node: N,
    parent: Option<u32>,
    child_index: u32,
    depth: usize,
}

/// Lazily evaluates the query for one node. Cheap conditions run first and every
/// attribute read is skipped once a condition already failed. `AXValue` is only
/// read for non-sensitive nodes; for sensitive nodes a `value` condition simply
/// never matches.
fn find_conditions_hold<S: AxSource>(
    source: &S,
    node: &S::Node,
    fingerprint: &ElementFingerprint,
    sensitive: bool,
    query: &FindQuery,
    enabled_cache: &mut Option<Option<bool>>,
    focused_cache: &mut Option<Option<bool>>,
) -> Result<bool, String> {
    if query
        .role
        .as_deref()
        .is_some_and(|expected| fingerprint.role != expected)
    {
        return Ok(false);
    }
    if query
        .subrole
        .as_deref()
        .is_some_and(|expected| fingerprint.subrole.as_deref() != Some(expected))
    {
        return Ok(false);
    }
    if query.label.as_deref().is_some_and(|expected| {
        ![
            fingerprint.title.as_deref(),
            fingerprint.description.as_deref(),
            fingerprint.placeholder.as_deref(),
        ]
        .into_iter()
        .flatten()
        .any(|text| text.contains(expected))
    }) {
        return Ok(false);
    }
    if let Some(expected) = query.enabled {
        let enabled = match *enabled_cache {
            Some(cached) => cached,
            None => {
                let read = source.enabled(node)?;
                *enabled_cache = Some(read);
                read
            }
        };
        if enabled != Some(expected) {
            return Ok(false);
        }
    }
    if let Some(expected) = query.focused {
        let focused = match *focused_cache {
            Some(cached) => cached,
            None => {
                let read = source.focused(node)?;
                *focused_cache = Some(read);
                read
            }
        };
        if focused != Some(expected) {
            return Ok(false);
        }
    }
    if let Some(expected) = query.value.as_deref() {
        if sensitive {
            return Ok(false);
        }
        let matched = source
            .value(node, MAX_FIND_VALUE_BYTES)?
            .is_some_and(|text| text.contains(expected));
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Breadth-first search below `root` (or from the window root when `prefix` is
/// `None`). A query root itself is never a match. The walk stops as soon as one
/// match beyond `bounds.limit` is found.
pub(crate) fn find<S: AxSource, C: AxClock>(
    source: &S,
    clock: &C,
    root: S::Node,
    surface_id: &str,
    prefix: Option<&ElementRecord>,
    query: &FindQuery,
    bounds: FindBounds,
) -> Result<AccessibilityTreeResult, String> {
    if let Some(prefix) = prefix {
        ensure_queryable_root(prefix)?;
    }
    let root_absolute_depth = prefix.map_or(0, |prefix| prefix.path.len());
    let mut arena: Vec<Visited> = Vec::new();
    let mut queue = VecDeque::from([QueuedNode {
        node: root,
        parent: None,
        child_index: 0,
        depth: 0,
    }]);
    let mut matches: Vec<(Value, ElementRecord)> = Vec::new();
    let mut extra_match = false;
    let mut visit_cap = false;
    let mut time_hit = false;
    let mut depth_bound = false;

    while let Some(QueuedNode {
        node,
        parent,
        child_index,
        depth,
    }) = queue.pop_front()
    {
        source.check_deadline()?;
        if arena.len() >= bounds.max_visited {
            visit_cap = true;
            break;
        }
        if clock.elapsed() >= bounds.soft_budget {
            time_hit = true;
            break;
        }
        let (fingerprint, sensitive) = match (parent, prefix) {
            (None, Some(prefix)) => (
                prefix
                    .lineage
                    .last()
                    .cloned()
                    .ok_or_else(|| "stale_element: AX element lineage is incomplete".to_string())?,
                false,
            ),
            _ => {
                let inherited = parent.is_some_and(|parent| arena[parent as usize].sensitive);
                let fingerprint = Arc::new(source.fingerprint(&node, inherited)?);
                let sensitive = is_sensitive(&fingerprint);
                (fingerprint, sensitive)
            }
        };
        let index = arena.len() as u32;
        arena.push(Visited {
            parent,
            child_index,
            depth,
            fingerprint: fingerprint.clone(),
            sensitive,
        });

        let is_candidate = prefix.is_none() || depth > 0;
        if is_candidate {
            let mut enabled_cache = None;
            let mut focused_cache = None;
            if find_conditions_hold(
                source,
                &node,
                &fingerprint,
                sensitive,
                query,
                &mut enabled_cache,
                &mut focused_cache,
            )? {
                if matches.len() >= bounds.limit {
                    extra_match = true;
                    break;
                }
                let enabled = match enabled_cache {
                    Some(cached) => cached,
                    None => source.enabled(&node)?,
                };
                let focused = match focused_cache {
                    Some(cached) => cached,
                    None => source.focused(&node)?,
                };
                matches.push(build_match(
                    surface_id,
                    prefix,
                    &arena,
                    index,
                    root_absolute_depth,
                    enabled,
                    focused,
                ));
            }
        }

        let absolute_depth = root_absolute_depth + depth;
        if depth < bounds.max_depth && absolute_depth < MAX_ACCESSIBILITY_ABSOLUTE_DEPTH {
            let child_count = source.child_count(&node)?;
            if child_count > 0 {
                let mut take = child_count.min(bounds.max_children_per_node);
                if take < child_count {
                    visit_cap = true;
                }
                let remaining = bounds.max_visited.saturating_sub(arena.len() + queue.len());
                if take > remaining {
                    take = remaining;
                    visit_cap = true;
                }
                if take > 0 {
                    for (child_index, child) in
                        source.children(&node, take)?.into_iter().enumerate()
                    {
                        queue.push_back(QueuedNode {
                            node: child,
                            parent: Some(index),
                            child_index: child_index as u32,
                            depth: depth + 1,
                        });
                    }
                }
            }
        } else if source.child_count(&node)? > 0 {
            depth_bound = true;
        }
    }
    source.check_deadline()?;

    let stop_reason = if extra_match {
        FindStopReason::Limit
    } else if time_hit {
        FindStopReason::TimeBudget
    } else if visit_cap {
        FindStopReason::VisitBudget
    } else if depth_bound {
        FindStopReason::DepthBound
    } else {
        FindStopReason::Complete
    };

    let mut elements: Vec<(String, ElementRecord)> = Vec::with_capacity(matches.len() + 1);
    let root_element_id = match prefix {
        Some(prefix) => {
            let id = allocate_selector("element_", |_| false)?;
            elements.push((id.clone(), prefix.clone()));
            Some(id)
        }
        None => None,
    };
    let mut output_elements = Vec::with_capacity(matches.len());
    for (mut descriptor, record) in matches {
        let id = allocate_selector("element_", |id| {
            elements.iter().any(|(existing, _)| existing == id)
        })?;
        descriptor["element_id"] = Value::String(id.clone());
        elements.push((id, record));
        output_elements.push(descriptor);
    }
    let count = output_elements.len();
    let output = json!({
        "platform": source.platform(),
        "surface_id": surface_id,
        "search_mode": "deep",
        "root_element_id": root_element_id,
        "elements": output_elements,
        "count": count,
        "scanned_nodes": arena.len(),
        "truncated": stop_reason != FindStopReason::Complete,
        "stop_reason": stop_reason.as_str(),
    });
    Ok(AccessibilityTreeResult { output, elements })
}

fn build_match(
    surface_id: &str,
    prefix: Option<&ElementRecord>,
    arena: &[Visited],
    index: u32,
    root_absolute_depth: usize,
    enabled: Option<bool>,
    focused: Option<bool>,
) -> (Value, ElementRecord) {
    // Walk back to the traversal root, collecting child indices and fingerprints.
    let mut relative_path = Vec::new();
    let mut relative_lineage: Vec<Arc<ElementFingerprint>> = Vec::new();
    let mut cursor = index;
    loop {
        let entry = &arena[cursor as usize];
        match entry.parent {
            Some(parent) => {
                relative_path.push(entry.child_index as usize);
                relative_lineage.push(entry.fingerprint.clone());
                cursor = parent;
            }
            None => {
                // With a prefix the root fingerprint is already the last prefix entry.
                if prefix.is_none() {
                    relative_lineage.push(entry.fingerprint.clone());
                }
                break;
            }
        }
    }
    relative_path.reverse();
    relative_lineage.reverse();
    let (mut path, mut lineage) = match prefix {
        Some(prefix) => (prefix.path.clone(), prefix.lineage.clone()),
        None => (Vec::new(), Vec::new()),
    };
    path.extend(relative_path);
    lineage.extend(relative_lineage);
    let target = &arena[index as usize];
    let fingerprint = &target.fingerprint;
    let absolute_depth = root_absolute_depth + target.depth;
    let ancestors = ancestors_summary(&lineage[..lineage.len().saturating_sub(1)]);
    let descriptor = json!({
        "element_id": Value::Null,
        "role": fingerprint.role,
        "subrole": fingerprint.subrole,
        "title": fingerprint.title,
        "description": fingerprint.description,
        "placeholder": fingerprint.placeholder,
        "enabled": enabled,
        "focused": focused,
        "depth": absolute_depth,
        "ancestors": ancestors,
    });
    (
        descriptor,
        ElementRecord {
            surface_id: surface_id.to_string(),
            path,
            lineage,
        },
    )
}

fn single_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    let mut end = text.len().min(max_bytes);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// `far › … › near` summary of `ancestors` (root first). Each segment is the role
/// plus, for non-protected ancestors, up to 40 bytes of title. The whole string is
/// bounded to 256 bytes by dropping the farthest segments and prefixing `… › `.
/// Values are never included.
pub(crate) fn ancestors_summary(ancestors: &[Arc<ElementFingerprint>]) -> String {
    let segments: Vec<String> = ancestors
        .iter()
        .map(|fingerprint| {
            let role = single_line(&fingerprint.role);
            match fingerprint.title.as_deref() {
                Some(title) if !fingerprint.protected && !title.is_empty() => {
                    let title = single_line(truncate_utf8(title, ANCESTOR_TITLE_MAX_BYTES));
                    format!("{role} “{title}”")
                }
                _ => role,
            }
        })
        .collect();
    let joined = segments.join(ANCESTORS_SEPARATOR);
    if joined.len() <= ANCESTORS_MAX_BYTES {
        return joined;
    }
    for start in 1..segments.len() {
        let candidate = format!(
            "{ANCESTORS_TRUNCATION_PREFIX}{}",
            segments[start..].join(ANCESTORS_SEPARATOR)
        );
        if candidate.len() <= ANCESTORS_MAX_BYTES {
            return candidate;
        }
    }
    let nearest = segments.last().map(String::as_str).unwrap_or_default();
    let budget = ANCESTORS_MAX_BYTES - ANCESTORS_TRUNCATION_PREFIX.len();
    format!(
        "{ANCESTORS_TRUNCATION_PREFIX}{}",
        truncate_utf8(nearest, budget)
    )
}

#[cfg(test)]
mod tests;
