use super::*;
use crate::MAX_ELEMENT_ID_BYTES;
use std::cell::{Cell, RefCell};

#[derive(Clone, Debug, Default)]
struct FakeNode {
    role: String,
    subrole: Option<String>,
    identifier: Option<String>,
    title: Option<String>,
    description: Option<String>,
    placeholder: Option<String>,
    value: Option<String>,
    own_protected: bool,
    enabled: Option<bool>,
    focused: Option<bool>,
    children: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Call {
    Fingerprint { node: usize, inherited: bool },
    /// The fingerprint read title/description/placeholder (node was not protected).
    TextRead(usize),
    Role(usize),
    Value(usize),
    Enabled(usize),
    Focused(usize),
    ChildCount(usize),
    Children(usize, usize),
    ChildAt(usize, usize),
}

struct FakeTree {
    nodes: Vec<FakeNode>,
    calls: RefCell<Vec<Call>>,
    deadline_checks_left: Cell<usize>,
    clock: Cell<Duration>,
    clock_step: Cell<Duration>,
}

impl FakeTree {
    fn new(root_role: &str) -> Self {
        let mut tree = Self {
            nodes: Vec::new(),
            calls: RefCell::new(Vec::new()),
            deadline_checks_left: Cell::new(usize::MAX),
            clock: Cell::new(Duration::ZERO),
            clock_step: Cell::new(Duration::ZERO),
        };
        tree.nodes.push(FakeNode {
            role: root_role.to_string(),
            ..FakeNode::default()
        });
        tree
    }

    fn add(&mut self, parent: usize, role: &str) -> usize {
        let index = self.nodes.len();
        self.nodes.push(FakeNode {
            role: role.to_string(),
            ..FakeNode::default()
        });
        self.nodes[parent].children.push(index);
        index
    }

    fn titled(&mut self, parent: usize, role: &str, title: &str) -> usize {
        let index = self.add(parent, role);
        self.nodes[index].title = Some(title.to_string());
        index
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.borrow().clone()
    }

    fn clear_calls(&self) {
        self.calls.borrow_mut().clear();
    }

    fn values_read(&self) -> Vec<usize> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::Value(node) => Some(node),
                _ => None,
            })
            .collect()
    }

    fn text_reads(&self) -> Vec<usize> {
        self.calls()
            .into_iter()
            .filter_map(|call| match call {
                Call::TextRead(node) => Some(node),
                _ => None,
            })
            .collect()
    }
}

impl AxSource for FakeTree {
    type Node = usize;

    fn platform(&self) -> &'static str {
        "macos"
    }

    fn fingerprint(&self, node: &usize, inherited_protected: bool) -> Result<ElementFingerprint, String> {
        self.calls.borrow_mut().push(Call::Fingerprint {
            node: *node,
            inherited: inherited_protected,
        });
        self.clock.set(self.clock.get() + self.clock_step.get());
        let data = &self.nodes[*node];
        let protected = inherited_protected || data.own_protected;
        if !protected {
            self.calls.borrow_mut().push(Call::TextRead(*node));
        }
        Ok(ElementFingerprint {
            role: data.role.clone(),
            subrole: data.subrole.clone(),
            identifier: data.identifier.clone(),
            title: (!protected).then(|| data.title.clone()).flatten(),
            description: (!protected).then(|| data.description.clone()).flatten(),
            placeholder: (!protected).then(|| data.placeholder.clone()).flatten(),
            protected,
            #[cfg(windows)]
            native_runtime_id: Vec::new(),
        })
    }

    fn role(&self, node: &usize) -> Result<Option<String>, String> {
        self.calls.borrow_mut().push(Call::Role(*node));
        Ok(Some(self.nodes[*node].role.clone()))
    }

    fn value(&self, node: &usize, max_bytes: usize) -> Result<Option<String>, String> {
        self.calls.borrow_mut().push(Call::Value(*node));
        Ok(self.nodes[*node]
            .value
            .as_deref()
            .map(|text| truncate_utf8(text, max_bytes).to_string()))
    }

    fn enabled(&self, node: &usize) -> Result<Option<bool>, String> {
        self.calls.borrow_mut().push(Call::Enabled(*node));
        Ok(self.nodes[*node].enabled)
    }

    fn focused(&self, node: &usize) -> Result<Option<bool>, String> {
        self.calls.borrow_mut().push(Call::Focused(*node));
        Ok(self.nodes[*node].focused)
    }

    fn child_count(&self, node: &usize) -> Result<usize, String> {
        self.calls.borrow_mut().push(Call::ChildCount(*node));
        Ok(self.nodes[*node].children.len())
    }

    fn children(&self, node: &usize, take: usize) -> Result<Vec<usize>, String> {
        self.calls.borrow_mut().push(Call::Children(*node, take));
        Ok(self.nodes[*node].children.iter().copied().take(take).collect())
    }

    fn child_at(&self, node: &usize, index: usize) -> Result<usize, String> {
        self.calls.borrow_mut().push(Call::ChildAt(*node, index));
        self.nodes[*node]
            .children
            .get(index)
            .copied()
            .ok_or_else(|| "stale_element: AX child path no longer resolves exactly".to_string())
    }

    fn check_deadline(&self) -> Result<(), String> {
        let left = self.deadline_checks_left.get();
        if left == 0 {
            return Err(
                "accessibility_failed: macOS Accessibility observation deadline exceeded"
                    .to_string(),
            );
        }
        if left != usize::MAX {
            self.deadline_checks_left.set(left - 1);
        }
        Ok(())
    }
}

impl AxClock for FakeTree {
    fn elapsed(&self) -> Duration {
        self.clock.get()
    }
}

/// window
/// ├─ 1 AXGroup "Toolbar"
/// │   ├─ 3 AXButton "Share" (enabled)
/// │   └─ 4 AXTextField placeholder "Search" value "hello world"
/// └─ 2 AXGroup "Login"
///     ├─ 5 AXTextField/AXSecureTextField "Password" value "hunter2"
///     │   └─ 7 AXStaticText "dots" value "hunter2-child"
///     └─ 6 AXGroup (AXProtectedContent) "Vault" value "vault-secret"
///         └─ 8 AXStaticText "inner" value "inner-secret"
fn sample_tree() -> FakeTree {
    let mut tree = FakeTree::new("AXWindow");
    tree.nodes[0].title = Some("Main".to_string());
    let toolbar = tree.titled(0, "AXGroup", "Toolbar");
    let login = tree.titled(0, "AXGroup", "Login");
    let share = tree.titled(toolbar, "AXButton", "Share");
    tree.nodes[share].enabled = Some(true);
    tree.nodes[share].focused = Some(false);
    let search = tree.add(toolbar, "AXTextField");
    tree.nodes[search].placeholder = Some("Search".to_string());
    tree.nodes[search].value = Some("hello world".to_string());
    tree.nodes[search].enabled = Some(true);
    tree.nodes[search].focused = Some(true);
    let password = tree.titled(login, "AXTextField", "Password");
    tree.nodes[password].subrole = Some("AXSecureTextField".to_string());
    tree.nodes[password].value = Some("hunter2".to_string());
    let vault = tree.titled(login, "AXGroup", "Vault");
    tree.nodes[vault].own_protected = true;
    tree.nodes[vault].value = Some("vault-secret".to_string());
    let dots = tree.titled(password, "AXStaticText", "dots");
    tree.nodes[dots].value = Some("hunter2-child".to_string());
    let inner = tree.titled(vault, "AXStaticText", "inner");
    tree.nodes[inner].value = Some("inner-secret".to_string());
    tree
}

const SECURE: usize = 5;
const DOTS: usize = 7;
const VAULT: usize = 6;
const INNER: usize = 8;

fn bounds(max_depth: usize, max_nodes: usize) -> TreeBounds {
    TreeBounds {
        max_depth,
        max_nodes,
    }
}

fn find_bounds(limit: usize) -> FindBounds {
    FindBounds {
        limit,
        max_depth: 32,
        max_visited: 4000,
        max_children_per_node: 512,
        soft_budget: Duration::from_secs(6),
    }
}

fn query() -> FindQuery {
    FindQuery::default()
}

fn nodes_of(result: &AccessibilityTreeResult) -> Vec<Value> {
    result.output["nodes"].as_array().unwrap().clone()
}

/// Replaces random element ids by their position so outputs can be compared.
fn normalize_nodes(nodes: &[Value]) -> Vec<Value> {
    let positions: std::collections::HashMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node["element_id"].as_str().unwrap().to_string(), index))
        .collect();
    nodes
        .iter()
        .enumerate()
        .map(|(index, node)| {
            let mut node = node.clone();
            node["element_id"] = json!(format!("n{index}"));
            node["parent_element_id"] = match node["parent_element_id"].as_str() {
                Some(parent) => json!(format!("n{}", positions[parent])),
                None => Value::Null,
            };
            node
        })
        .collect()
}

/// The pre-engine macOS `accessibility_tree` loop, kept verbatim over `FakeTree`
/// as the golden reference for `TreeMode::Legacy`.
fn legacy_reference(
    tree: &FakeTree,
    max_depth: usize,
    max_nodes: usize,
) -> (Vec<Value>, Vec<ElementRecord>, bool) {
    let mut queue = VecDeque::from([(
        0usize,
        None::<usize>,
        0usize,
        Vec::<usize>::new(),
        Vec::<ElementFingerprint>::new(),
        false,
    )]);
    let mut nodes: Vec<Value> = Vec::new();
    let mut records = Vec::new();
    let mut truncated = false;
    let mut positions = Vec::new();
    while let Some((element, parent, depth, path, mut lineage, inherited_protected)) =
        queue.pop_front()
    {
        if nodes.len() >= max_nodes {
            truncated = true;
            break;
        }
        let fingerprint = tree.fingerprint(&element, inherited_protected).unwrap();
        let role = fingerprint.role.clone();
        let subrole = fingerprint.subrole.clone();
        let title = fingerprint.title.clone();
        let description = fingerprint.description.clone();
        let placeholder = fingerprint.placeholder.clone();
        let protected = fingerprint.protected;
        lineage.push(fingerprint);
        let sensitive = role == "AXSecureTextField"
            || subrole
                .as_deref()
                .is_some_and(|value| value.contains("Secure"));
        let value = if sensitive || protected {
            None
        } else {
            tree.value(&element, MAX_TEXT_BYTES).unwrap()
        };
        let enabled = tree.enabled(&element).unwrap();
        let focused = tree.focused(&element).unwrap();
        let child_count = tree.child_count(&element).unwrap();
        let my_position = nodes.len() + queue.len() + 1; // unused; ids are positional below
        let _ = my_position;
        if depth < max_depth && child_count > 0 {
            let reserved = nodes.len() + queue.len() + 1;
            let remaining = max_nodes.saturating_sub(reserved);
            let take = child_count.min(remaining);
            if take < child_count {
                truncated = true;
            }
            for (index, child) in tree.children(&element, take).unwrap().into_iter().enumerate() {
                let mut child_path = path.clone();
                child_path.push(index);
                queue.push_back((
                    child,
                    Some(nodes.len()),
                    depth + 1,
                    child_path,
                    lineage.clone(),
                    protected,
                ));
            }
        } else if child_count > 0 {
            truncated = true;
        }
        records.push(ElementRecord {
            surface_id: "surface_x".to_string(),
            path,
            lineage: lineage.into_iter().map(Arc::new).collect(),
        });
        positions.push(nodes.len());
        nodes.push(json!({
            "element_id": format!("n{}", nodes.len()),
            "parent_element_id": parent.map(|parent| format!("n{parent}")),
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
    (nodes, records, truncated)
}

fn records_of(result: &AccessibilityTreeResult) -> Vec<ElementRecord> {
    result.elements.iter().map(|(_, record)| record.clone()).collect()
}

fn assert_stale(error: String) {
    assert!(error.starts_with("stale_element:"), "{error}");
}

// ---------------------------------------------------------------------------
// observe_tree
// ---------------------------------------------------------------------------

#[test]
fn legacy_observe_tree_matches_the_original_algorithm_field_by_field() {
    for (max_depth, max_nodes) in [(8, 256), (6, 128), (1, 3), (2, 5), (0, 1), (8, 4)] {
        let tree = sample_tree();
        let result = observe_tree(
            &tree,
            0,
            "surface_x",
            None,
            bounds(max_depth, max_nodes),
            TreeMode::Legacy,
        )
        .unwrap();
        let (expected_nodes, expected_records, expected_truncated) =
            legacy_reference(&sample_tree(), max_depth, max_nodes);
        assert_eq!(
            normalize_nodes(&nodes_of(&result)),
            expected_nodes,
            "bounds {max_depth}/{max_nodes}"
        );
        assert_eq!(records_of(&result), expected_records);
        assert_eq!(
            result.output["truncated"].as_bool().unwrap(),
            expected_truncated
        );
        assert_eq!(result.output["max_depth"], json!(max_depth));
        assert_eq!(result.output["max_nodes"], json!(max_nodes));
        assert_eq!(result.output["platform"], json!("macos"));
        assert_eq!(result.output["node_count"], json!(expected_nodes.len()));
        // Legacy output keeps its exact historical key set.
        let mut keys: Vec<&str> = result
            .output
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "max_depth",
                "max_nodes",
                "node_count",
                "nodes",
                "platform",
                "surface_id",
                "truncated"
            ]
        );
    }
}

#[test]
fn legacy_observe_tree_keeps_historical_secure_descendant_behavior() {
    let tree = sample_tree();
    observe_tree(&tree, 0, "surface_x", None, bounds(8, 256), TreeMode::Legacy).unwrap();
    // The legacy walk only propagates AXProtectedContent, so a child of a secure field
    // is fingerprinted normally; the secure field's own value is still never read.
    assert!(tree.calls().contains(&Call::Fingerprint {
        node: DOTS,
        inherited: false
    }));
    assert!(tree.calls().contains(&Call::Fingerprint {
        node: INNER,
        inherited: true
    }));
    let values = tree.values_read();
    assert!(!values.contains(&SECURE));
    assert!(!values.contains(&VAULT));
    assert!(!values.contains(&INNER));
}

#[test]
fn query_observe_tree_never_reads_value_or_text_for_secure_protected_or_their_descendants() {
    let tree = sample_tree();
    let result =
        observe_tree(&tree, 0, "surface_x", None, bounds(8, 256), TreeMode::Query).unwrap();
    let values = tree.values_read();
    for forbidden in [SECURE, DOTS, VAULT, INNER] {
        assert!(!values.contains(&forbidden), "value read for {forbidden}");
    }
    let text_reads = tree.text_reads();
    for forbidden in [VAULT, DOTS, INNER] {
        assert!(!text_reads.contains(&forbidden), "text read for {forbidden}");
    }
    // The secure field itself keeps its title (ordinary label), but no value.
    assert!(text_reads.contains(&SECURE));
    let nodes = nodes_of(&result);
    for node in &nodes {
        let title = node["title"].as_str().unwrap_or_default();
        if ["dots", "inner", "Vault"].contains(&title) {
            panic!("protected title leaked: {title}");
        }
        assert_ne!(node["value"].as_str(), Some("hunter2"));
        assert_ne!(node["value"].as_str(), Some("hunter2-child"));
        assert_ne!(node["value"].as_str(), Some("vault-secret"));
        assert_ne!(node["value"].as_str(), Some("inner-secret"));
    }
    // Descendants of a secure field are stored as protected so control paths reject them.
    let dots_record = &result.elements[nodes
        .iter()
        .position(|node| node["role"] == "AXStaticText" && node["value"].is_null() && node["title"].is_null())
        .unwrap()]
    .1;
    assert!(dots_record.contains_protected_content());
}

#[test]
fn query_observe_tree_without_prefix_reports_null_root() {
    let tree = sample_tree();
    let result =
        observe_tree(&tree, 0, "surface_x", None, bounds(8, 256), TreeMode::Query).unwrap();
    assert_eq!(result.output["root"], Value::Null);
    assert_eq!(nodes_of(&result)[0]["depth"], json!(0));
}

fn tree_record(tree: &FakeTree, path: &[usize]) -> ElementRecord {
    let mut node = 0usize;
    let mut lineage = vec![Arc::new(tree.fingerprint(&node, false).unwrap())];
    for &index in path {
        let parent_fp = lineage.last().unwrap().clone();
        node = tree.nodes[node].children[index];
        let inherited = parent_fp.protected
            || (is_secure_text_fingerprint(&parent_fp)
                && matches!(node, DOTS | INNER));
        lineage.push(Arc::new(tree.fingerprint(&node, inherited).unwrap()));
    }
    tree.clear_calls();
    ElementRecord {
        surface_id: "surface_x".to_string(),
        path: path.to_vec(),
        lineage,
    }
}

#[test]
fn subtree_prefix_concatenates_path_lineage_and_reports_relative_depth() {
    let tree = sample_tree();
    let prefix = tree_record(&tree, &[0]); // Toolbar
    let result = observe_tree(
        &tree,
        1,
        "surface_x",
        Some(&prefix),
        bounds(4, 16),
        TreeMode::Query,
    )
    .unwrap();
    let nodes = nodes_of(&result);
    assert_eq!(nodes.len(), 3);
    assert_eq!(nodes[0]["depth"], json!(0));
    assert_eq!(nodes[0]["parent_element_id"], Value::Null);
    assert_eq!(nodes[0]["title"], json!("Toolbar"));
    assert_eq!(nodes[1]["depth"], json!(1));
    assert_eq!(nodes[1]["parent_element_id"], nodes[0]["element_id"]);
    assert_eq!(
        result.output["root"],
        json!({"element_id": nodes[0]["element_id"], "absolute_depth": 1})
    );
    let records = records_of(&result);
    assert_eq!(records[0].path, vec![0]);
    assert_eq!(records[0].lineage.len(), 2);
    assert_eq!(records[1].path, vec![0, 0]);
    assert_eq!(records[2].path, vec![0, 1]);
    assert_eq!(records[1].lineage.len(), 3);
    assert_eq!(records[1].lineage[..2], prefix.lineage[..]);
    assert!(Arc::ptr_eq(&records[1].lineage[0], &prefix.lineage[0]));
    assert_eq!(records[1].lineage[2].title.as_deref(), Some("Share"));
    // The query root fingerprint comes from the verified record, it is not re-read.
    assert!(!tree
        .calls()
        .iter()
        .any(|call| matches!(call, Call::Fingerprint { node: 1, .. })));
}

#[test]
fn subtree_rejects_incomplete_protected_and_secure_roots() {
    let tree = sample_tree();
    let mut incomplete = tree_record(&tree, &[0]);
    incomplete.lineage.pop();
    let error = observe_tree(&tree, 1, "surface_x", Some(&incomplete), bounds(2, 8), TreeMode::Query)
        .err()
        .unwrap();
    assert_stale(error);

    let secure = tree_record(&tree, &[1, 0]);
    let error = observe_tree(&tree, SECURE, "surface_x", Some(&secure), bounds(2, 8), TreeMode::Query)
        .err()
        .unwrap();
    assert!(error.starts_with("permission_denied:"), "{error}");

    let protected = tree_record(&tree, &[1, 1]);
    let error = observe_tree(&tree, VAULT, "surface_x", Some(&protected), bounds(2, 8), TreeMode::Query)
        .err()
        .unwrap();
    assert!(error.starts_with("permission_denied:"), "{error}");

    let below_protected = tree_record(&tree, &[1, 1, 0]);
    let error = observe_tree(&tree, INNER, "surface_x", Some(&below_protected), bounds(2, 8), TreeMode::Query)
        .err()
        .unwrap();
    assert!(error.starts_with("permission_denied:"), "{error}");
    assert!(tree.values_read().is_empty());
}

/// A chain `depth` levels deep below the window root.
fn chain_tree(depth: usize) -> FakeTree {
    let mut tree = FakeTree::new("AXWindow");
    let mut parent = 0;
    for level in 0..depth {
        parent = tree.titled(parent, "AXGroup", &format!("level-{level}"));
    }
    tree
}

fn chain_record(tree: &FakeTree, depth: usize) -> ElementRecord {
    let mut lineage = Vec::new();
    for node in 0..=depth {
        lineage.push(Arc::new(tree.fingerprint(&node, false).unwrap()));
    }
    tree.clear_calls();
    ElementRecord {
        surface_id: "surface_x".to_string(),
        path: vec![0; depth],
        lineage,
    }
}

#[test]
fn subtree_absolute_depth_is_capped_at_sixty_four() {
    let tree = chain_tree(70);
    let prefix = chain_record(&tree, 62);
    let result = observe_tree(
        &tree,
        62,
        "surface_x",
        Some(&prefix),
        bounds(8, 256),
        TreeMode::Query,
    )
    .unwrap();
    let nodes = nodes_of(&result);
    // Root at absolute depth 62 -> children at 63 and 64; nothing is expanded past 64.
    assert_eq!(nodes.len(), 3);
    assert_eq!(result.output["truncated"], json!(true));
    assert_eq!(result.output["root"]["absolute_depth"], json!(62));
    assert!(records_of(&result)
        .iter()
        .all(|record| record.path.len() <= MAX_ACCESSIBILITY_ABSOLUTE_DEPTH));

    let at_cap = chain_record(&tree, 64);
    let result = observe_tree(
        &tree,
        64,
        "surface_x",
        Some(&at_cap),
        bounds(8, 256),
        TreeMode::Query,
    )
    .unwrap();
    assert_eq!(nodes_of(&result).len(), 1);
    assert_eq!(result.output["truncated"], json!(true));
}

#[test]
fn observe_tree_respects_node_and_depth_bounds_and_deadline() {
    let tree = sample_tree();
    let result =
        observe_tree(&tree, 0, "surface_x", None, bounds(1, 256), TreeMode::Query).unwrap();
    assert_eq!(nodes_of(&result).len(), 3);
    assert_eq!(result.output["truncated"], json!(true));

    let result =
        observe_tree(&tree, 0, "surface_x", None, bounds(8, 4), TreeMode::Query).unwrap();
    assert_eq!(nodes_of(&result).len(), 4);
    assert_eq!(result.output["truncated"], json!(true));

    let tree = sample_tree();
    tree.deadline_checks_left.set(2);
    let error = observe_tree(&tree, 0, "surface_x", None, bounds(8, 256), TreeMode::Query)
        .err()
        .unwrap();
    assert!(error.contains("deadline exceeded"), "{error}");
}

#[test]
fn bounds_match_the_design_constants() {
    assert_eq!(crate::MAX_ACCESSIBILITY_ABSOLUTE_DEPTH, 64);
    assert_eq!(crate::DEFAULT_FIND_DEPTH, 32);
    assert_eq!(crate::MAX_FIND_DEPTH, 48);
    assert_eq!(crate::MAX_FIND_VISITED, 4000);
    assert_eq!(crate::DEFAULT_FIND_ELEMENTS_LIMIT, 8);
    assert_eq!(crate::MAX_FIND_ELEMENTS_LIMIT, 32);
    #[cfg(target_os = "macos")]
    {
        assert_eq!(crate::MAX_FIND_CHILDREN_PER_NODE, 512);
        assert_eq!(crate::FIND_SOFT_BUDGET, Duration::from_secs(6));
    }
}

// ---------------------------------------------------------------------------
// find
// ---------------------------------------------------------------------------

fn run_find(tree: &FakeTree, query: &FindQuery, bounds: FindBounds) -> AccessibilityTreeResult {
    find(tree, tree, 0, "surface_x", None, query, bounds).unwrap()
}

fn element_titles(result: &AccessibilityTreeResult) -> Vec<String> {
    result.output["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|element| {
            element["title"]
                .as_str()
                .or_else(|| element["placeholder"].as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect()
}

#[test]
fn find_matches_role_subrole_label_state_and_value_conditions() {
    let tree = sample_tree();
    let result = run_find(
        &tree,
        &FindQuery {
            role: Some("AXButton".into()),
            ..query()
        },
        find_bounds(8),
    );
    assert_eq!(element_titles(&result), vec!["Share"]);
    assert_eq!(result.output["search_mode"], json!("deep"));
    assert_eq!(result.output["stop_reason"], json!("complete"));
    assert_eq!(result.output["truncated"], json!(false));
    assert_eq!(result.output["root_element_id"], Value::Null);
    assert_eq!(result.output["count"], json!(1));
    assert_eq!(result.output["scanned_nodes"], json!(9));

    let by_label = run_find(
        &tree,
        &FindQuery {
            label: Some("Sear".into()),
            ..query()
        },
        find_bounds(8),
    );
    assert_eq!(by_label.output["count"], json!(1));
    assert_eq!(by_label.output["elements"][0]["role"], json!("AXTextField"));
    assert_eq!(by_label.output["elements"][0]["focused"], json!(true));

    let by_subrole = run_find(
        &tree,
        &FindQuery {
            subrole: Some("AXSecureTextField".into()),
            ..query()
        },
        find_bounds(8),
    );
    assert_eq!(by_subrole.output["count"], json!(1));

    let by_state = run_find(
        &tree,
        &FindQuery {
            enabled: Some(true),
            focused: Some(true),
            ..query()
        },
        find_bounds(8),
    );
    assert_eq!(by_state.output["count"], json!(1));
    assert_eq!(by_state.output["elements"][0]["role"], json!("AXTextField"));

    let by_value = run_find(
        &tree,
        &FindQuery {
            value: Some("lo wo".into()),
            ..query()
        },
        find_bounds(8),
    );
    assert_eq!(by_value.output["count"], json!(1));
    // Values are searched but never returned.
    assert!(!by_value.output.to_string().contains("hello world"));
    assert!(by_value.output["elements"][0].get("value").is_none());

    let combined = run_find(
        &tree,
        &FindQuery {
            role: Some("AXTextField".into()),
            value: Some("lo wo".into()),
            enabled: Some(false),
            ..query()
        },
        find_bounds(8),
    );
    assert_eq!(combined.output["count"], json!(0));
    // Case sensitive.
    let case = run_find(
        &tree,
        &FindQuery {
            label: Some("share".into()),
            ..query()
        },
        find_bounds(8),
    );
    assert_eq!(case.output["count"], json!(0));
}

#[test]
fn find_never_reads_value_for_secure_protected_or_their_descendants() {
    let tree = sample_tree();
    for needle in ["hunter2", "hunter2-child", "vault-secret", "inner-secret", "secret", ""] {
        tree.clear_calls();
        let result = run_find(
            &tree,
            &FindQuery {
                value: Some(needle.into()),
                ..query()
            },
            find_bounds(32),
        );
        let values = tree.values_read();
        for forbidden in [SECURE, DOTS, VAULT, INNER] {
            assert!(!values.contains(&forbidden), "{needle}: value read for {forbidden}");
        }
        let rendered = result.output.to_string();
        for secret in ["hunter2", "vault-secret", "inner-secret"] {
            assert!(!rendered.contains(secret), "{needle}: leaked {secret}");
        }
        for (_, record) in &result.elements {
            let last = record.target_fingerprint().unwrap();
            assert!(!is_sensitive(last), "{needle}: sensitive node matched a value query");
        }
    }
    // Title/description/placeholder of protected descendants are never read either.
    let tree = sample_tree();
    run_find(&tree, &query(), find_bounds(32));
    let text_reads = tree.text_reads();
    for forbidden in [VAULT, DOTS, INNER] {
        assert!(!text_reads.contains(&forbidden), "text read for {forbidden}");
    }
    assert!(tree.calls().contains(&Call::Fingerprint {
        node: DOTS,
        inherited: true
    }));
    assert!(tree.calls().contains(&Call::Fingerprint {
        node: INNER,
        inherited: true
    }));
}

#[test]
fn find_sensitive_nodes_can_match_structurally_but_never_by_value() {
    let tree = sample_tree();
    let result = run_find(
        &tree,
        &FindQuery {
            role: Some("AXStaticText".into()),
            ..query()
        },
        find_bounds(8),
    );
    assert_eq!(result.output["count"], json!(2));
    for element in result.output["elements"].as_array().unwrap() {
        assert_eq!(element["title"], Value::Null);
        assert_eq!(element["description"], Value::Null);
        assert_eq!(element["placeholder"], Value::Null);
    }
    for (_, record) in &result.elements {
        assert!(record.contains_protected_content());
    }
    // Their records re-resolve to the same nodes without ever reading text or values.
    tree.clear_calls();
    let mut resolved: Vec<usize> = result
        .elements
        .iter()
        .map(|(_, record)| resolve(&tree, 0, record).unwrap())
        .collect();
    resolved.sort_unstable();
    assert_eq!(resolved, vec![DOTS, INNER]);
    assert!(tree.values_read().is_empty());
    assert!(!tree.text_reads().contains(&DOTS));
    assert!(!tree.text_reads().contains(&INNER));
}

#[test]
fn find_limit_stops_at_the_first_extra_match_and_reports_it() {
    let mut tree = FakeTree::new("AXWindow");
    for index in 0..5 {
        tree.titled(0, "AXLink", &format!("link-{index}"));
    }
    let wanted = FindQuery {
        role: Some("AXLink".into()),
        ..query()
    };
    let exact = run_find(&tree, &wanted, find_bounds(5));
    assert_eq!(exact.output["count"], json!(5));
    assert_eq!(exact.output["stop_reason"], json!("complete"));
    assert_eq!(exact.output["truncated"], json!(false));

    let limited = run_find(&tree, &wanted, find_bounds(2));
    assert_eq!(limited.output["count"], json!(2));
    assert_eq!(limited.output["stop_reason"], json!("limit"));
    assert_eq!(limited.output["truncated"], json!(true));
    // window + links 0..=2 (the third link is the first extra match): the walk stopped there.
    assert_eq!(limited.output["scanned_nodes"], json!(4));
}

#[test]
fn find_stop_reasons_cover_visit_time_and_depth_bounds() {
    // visit budget
    let mut wide = FakeTree::new("AXWindow");
    for index in 0..20 {
        wide.titled(0, "AXGroup", &format!("g{index}"));
    }
    let mut bounds = find_bounds(8);
    bounds.max_visited = 5;
    let result = run_find(&wide, &query_for_role("AXNothing"), bounds);
    assert_eq!(result.output["stop_reason"], json!("visit_budget"));
    assert_eq!(result.output["truncated"], json!(true));
    assert_eq!(result.output["scanned_nodes"], json!(5));

    // children-per-node cap is reported as a visit budget too
    let mut bounds = find_bounds(8);
    bounds.max_children_per_node = 4;
    let result = run_find(&wide, &query_for_role("AXNothing"), bounds);
    assert_eq!(result.output["stop_reason"], json!("visit_budget"));
    assert_eq!(result.output["scanned_nodes"], json!(5));

    // time budget (injected clock advances one second per node read)
    let slow = chain_tree(10);
    slow.clock_step.set(Duration::from_secs(1));
    let mut bounds = find_bounds(8);
    bounds.soft_budget = Duration::from_secs(3);
    let result = run_find(&slow, &query_for_role("AXNothing"), bounds);
    assert_eq!(result.output["stop_reason"], json!("time_budget"));
    assert_eq!(result.output["truncated"], json!(true));
    assert_eq!(result.output["scanned_nodes"], json!(3));

    // depth bound
    let deep = chain_tree(10);
    let mut bounds = find_bounds(8);
    bounds.max_depth = 3;
    let result = run_find(&deep, &query_for_role("AXNothing"), bounds);
    assert_eq!(result.output["stop_reason"], json!("depth_bound"));
    assert_eq!(result.output["scanned_nodes"], json!(4));

    // complete: the whole tree fits
    let result = run_find(&deep, &query_for_role("AXNothing"), find_bounds(8));
    assert_eq!(result.output["stop_reason"], json!("complete"));
    assert_eq!(result.output["truncated"], json!(false));
    assert_eq!(result.output["scanned_nodes"], json!(11));
}

fn query_for_role(role: &str) -> FindQuery {
    FindQuery {
        role: Some(role.into()),
        ..query()
    }
}

#[test]
fn find_absolute_depth_bound_applies_below_a_deep_root() {
    let tree = chain_tree(70);
    let prefix = chain_record(&tree, 62);
    let result = find(
        &tree,
        &tree,
        62,
        "surface_x",
        Some(&prefix),
        &query_for_role("AXNothing"),
        find_bounds(8),
    )
    .unwrap();
    assert_eq!(result.output["stop_reason"], json!("depth_bound"));
    // root (62) + 63 + 64
    assert_eq!(result.output["scanned_nodes"], json!(3));
}

#[test]
fn find_deadline_error_is_propagated() {
    let tree = sample_tree();
    tree.deadline_checks_left.set(3);
    let error = find(&tree, &tree, 0, "surface_x", None, &query(), find_bounds(8))
        .err()
        .unwrap();
    assert!(error.contains("deadline exceeded"), "{error}");
}

#[test]
fn find_records_resolve_back_to_the_same_node_and_go_stale_on_change() {
    let mut tree = sample_tree();
    let result = run_find(&tree, &query_for_role("AXButton"), find_bounds(8));
    let (_, record) = &result.elements[0];
    assert_eq!(record.path, vec![0, 0]);
    assert_eq!(record.lineage.len(), 3);
    assert_eq!(resolve(&tree, 0, record).unwrap(), 3);

    // Retitled target
    tree.nodes[3].title = Some("Share now".to_string());
    assert_stale(resolve(&tree, 0, record).unwrap_err());
    tree.nodes[3].title = Some("Share".to_string());
    assert_eq!(resolve(&tree, 0, record).unwrap(), 3);

    // Retitled ancestor
    tree.nodes[1].title = Some("Toolbar 2".to_string());
    let error = resolve(&tree, 0, record).unwrap_err();
    assert!(error.contains("ancestor") || error.contains("lineage"), "{error}");
    tree.nodes[1].title = Some("Toolbar".to_string());

    // Reordered children
    tree.nodes[1].children.reverse();
    assert_stale(resolve(&tree, 0, record).unwrap_err());
    tree.nodes[1].children.reverse();

    // Missing child
    tree.nodes[1].children.clear();
    assert_stale(resolve(&tree, 0, record).unwrap_err());

    // Window root identity changed
    let tree = sample_tree();
    let mut other_root = sample_tree();
    other_root.nodes[0].title = Some("Other".to_string());
    assert_stale(resolve(&other_root, 0, record).unwrap_err());
    assert_eq!(resolve(&tree, 0, record).unwrap(), 3);
}

#[test]
fn records_below_a_secure_field_resolve_for_both_legacy_and_hardened_walks() {
    let tree = sample_tree();
    for mode in [TreeMode::Legacy, TreeMode::Query] {
        let result = observe_tree(&tree, 0, "surface_x", None, bounds(8, 256), mode).unwrap();
        let records = records_of(&result);
        let dots_record = records
            .iter()
            .find(|record| record.path == vec![1, 0, 0])
            .unwrap();
        tree.clear_calls();
        assert_eq!(resolve(&tree, 0, dots_record).unwrap(), DOTS, "{mode:?}");
        assert_eq!(dots_record.lineage[3].protected, mode == TreeMode::Query);
        // Resolution never reads a value.
        assert!(tree.values_read().is_empty());
    }
    // Hardened records re-resolve without reading the descendant's text.
    tree.clear_calls();
    let result = observe_tree(&tree, 0, "surface_x", None, bounds(8, 256), TreeMode::Query).unwrap();
    let dots_record = records_of(&result)
        .into_iter()
        .find(|record| record.path == vec![1, 0, 0])
        .unwrap();
    tree.clear_calls();
    resolve(&tree, 0, &dots_record).unwrap();
    assert!(!tree.text_reads().contains(&DOTS));
}

#[test]
fn resolve_rejects_incomplete_and_overlong_records() {
    let tree = sample_tree();
    let mut record = tree_record(&tree, &[0]);
    record.lineage.pop();
    assert_stale(resolve(&tree, 0, &record).unwrap_err());
    let mut long = tree_record(&tree, &[0]);
    long.path = vec![0; MAX_ACCESSIBILITY_ABSOLUTE_DEPTH + 1];
    long.lineage = vec![long.lineage[0].clone(); MAX_ACCESSIBILITY_ABSOLUTE_DEPTH + 2];
    assert_stale(resolve(&tree, 0, &long).unwrap_err());
}

#[test]
fn find_with_root_reissues_the_root_id_excludes_the_root_and_uses_absolute_depth() {
    let tree = sample_tree();
    let prefix = tree_record(&tree, &[0]); // Toolbar
    let result = find(
        &tree,
        &tree,
        1,
        "surface_x",
        Some(&prefix),
        &FindQuery {
            role: Some("AXGroup".into()),
            ..query()
        },
        find_bounds(8),
    )
    .unwrap();
    // The Toolbar root is itself an AXGroup but is never a match.
    assert_eq!(result.output["count"], json!(0));
    assert_eq!(result.elements.len(), 1);
    let root_id = result.output["root_element_id"].as_str().unwrap();
    assert!(root_id.starts_with("element_"));
    assert!(root_id.len() <= MAX_ELEMENT_ID_BYTES);
    assert_eq!(result.elements[0].0, root_id);
    assert_eq!(result.elements[0].1, prefix);

    let result = find(
        &tree,
        &tree,
        1,
        "surface_x",
        Some(&prefix),
        &query_for_role("AXButton"),
        find_bounds(8),
    )
    .unwrap();
    assert_eq!(result.output["count"], json!(1));
    // Absolute depth: window(0) > Toolbar(1) > Share(2).
    assert_eq!(result.output["elements"][0]["depth"], json!(2));
    assert_eq!(result.elements.len(), 2);
    let record = &result.elements[1].1;
    assert_eq!(record.path, vec![0, 0]);
    assert_eq!(record.lineage.len(), 3);
    assert_eq!(resolve(&tree, 0, record).unwrap(), 3);
    // Unique ids.
    assert_ne!(result.elements[0].0, result.elements[1].0);
}

#[test]
fn find_rejects_protected_and_secure_roots_before_any_native_read() {
    let tree = sample_tree();
    for path in [vec![1usize, 0], vec![1, 1], vec![1, 1, 0], vec![1, 0, 0]] {
        let prefix = tree_record(&tree, &path);
        let node = {
            let mut node = 0;
            for &index in &path {
                node = tree.nodes[node].children[index];
            }
            node
        };
        let error = find(&tree, &tree, node, "surface_x", Some(&prefix), &query(), find_bounds(8))
            .err()
            .unwrap();
        assert!(error.starts_with("permission_denied:"), "{path:?}: {error}");
        assert!(tree.calls().is_empty(), "{path:?}: native calls {:?}", tree.calls());
    }
}

#[test]
fn find_lazily_reads_only_the_attributes_the_query_needs() {
    let tree = sample_tree();
    run_find(&tree, &query_for_role("AXButton"), find_bounds(8));
    let calls = tree.calls();
    // No value or state reads for non-matching nodes; the single match reads its state once.
    assert!(!calls.iter().any(|call| matches!(call, Call::Value(_))));
    assert_eq!(
        calls.iter().filter(|call| matches!(call, Call::Enabled(_))).count(),
        1
    );
    assert_eq!(
        calls.iter().filter(|call| matches!(call, Call::Focused(_))).count(),
        1
    );
}

#[test]
fn find_children_cap_limits_children_requested_from_the_source() {
    let mut tree = FakeTree::new("AXWindow");
    for index in 0..600 {
        tree.titled(0, "AXGroup", &format!("g{index}"));
    }
    let result = run_find(&tree, &query_for_role("AXNothing"), find_bounds(8));
    assert!(tree.calls().contains(&Call::Children(0, 512)));
    assert_eq!(result.output["scanned_nodes"], json!(513));
    assert_eq!(result.output["stop_reason"], json!("visit_budget"));
}

// ---------------------------------------------------------------------------
// web content probe
// ---------------------------------------------------------------------------

fn probe(tree: &FakeTree) -> WebProbe {
    probe_web_content(tree, 0).unwrap()
}

#[test]
fn probe_finds_a_populated_web_area_and_reads_only_role_and_child_counts() {
    let mut tree = FakeTree::new("AXWindow");
    let toolbar = tree.add(0, "AXToolbar");
    tree.titled(toolbar, "AXButton", "Back");
    let group = tree.add(0, "AXGroup");
    let area = tree.titled(group, "AXWebArea", "Page");
    tree.add(area, "AXGroup");
    assert_eq!(probe(&tree), WebProbe::Content);
    for call in tree.calls() {
        assert!(
            matches!(call, Call::Role(_) | Call::ChildCount(_) | Call::Children(..)),
            "{call:?}"
        );
    }
}

#[test]
fn probe_distinguishes_empty_absent_and_inconclusive() {
    // Empty web area: keep waiting.
    let mut tree = FakeTree::new("AXWindow");
    tree.add(0, "AXWebArea");
    assert_eq!(probe(&tree), WebProbe::Empty);

    // A fully explored tree without any web area is a definite absence.
    let mut tree = FakeTree::new("AXWindow");
    let group = tree.add(0, "AXGroup");
    tree.add(group, "AXButton");
    assert_eq!(probe(&tree), WebProbe::NoWebArea);

    // Too deep to be sure: inconclusive means "still empty".
    assert_eq!(probe(&chain_tree(40)), WebProbe::Empty);

    // Too wide to be sure.
    let mut wide = FakeTree::new("AXWindow");
    for _ in 0..300 {
        let group = wide.add(0, "AXGroup");
        wide.add(group, "AXButton");
    }
    assert_eq!(probe(&wide), WebProbe::Empty);

    // A populated area beyond an empty one still wins.
    let mut tree = FakeTree::new("AXWindow");
    tree.add(0, "AXWebArea");
    let second = tree.add(0, "AXWebArea");
    tree.add(second, "AXGroup");
    assert_eq!(probe(&tree), WebProbe::Content);
}

#[test]
fn probe_is_bounded_and_honors_the_deadline() {
    let mut wide = FakeTree::new("AXWindow");
    for _ in 0..1000 {
        wide.add(0, "AXGroup");
    }
    probe(&wide);
    let roles = wide
        .calls()
        .into_iter()
        .filter(|call| matches!(call, Call::Role(_)))
        .count();
    assert!(roles <= 200, "{roles}");

    let tree = chain_tree(5);
    tree.deadline_checks_left.set(1);
    let error = probe_web_content(&tree, 0).unwrap_err();
    assert!(error.contains("deadline exceeded"), "{error}");
}

// ---------------------------------------------------------------------------
// ancestors
// ---------------------------------------------------------------------------

fn fp(role: &str, title: Option<&str>, protected: bool) -> Arc<ElementFingerprint> {
    Arc::new(ElementFingerprint {
        role: role.to_string(),
        subrole: None,
        identifier: None,
        title: title.map(str::to_string),
        description: None,
        placeholder: None,
        protected,
        #[cfg(windows)]
        native_runtime_id: Vec::new(),
    })
}

#[test]
fn ancestors_summary_joins_far_to_near_with_titles() {
    let summary = ancestors_summary(&[
        fp("AXWindow", Some("Budget"), false),
        fp("AXGroup", None, false),
        fp("AXToolbar", Some("Main"), false),
    ]);
    assert_eq!(summary, "AXWindow “Budget” › AXGroup › AXToolbar “Main”");
    assert_eq!(ancestors_summary(&[]), "");
}

#[test]
fn ancestors_summary_shows_only_the_role_of_protected_ancestors_and_never_values() {
    let summary = ancestors_summary(&[
        fp("AXGroup", Some("Vault title"), true),
        fp("AXGroup", Some("Open"), false),
    ]);
    assert_eq!(summary, "AXGroup › AXGroup “Open”");
    assert!(!summary.contains("Vault"));
}

#[test]
fn ancestors_summary_truncates_titles_on_utf8_boundaries_and_sanitizes_control_chars() {
    let long_title = "好".repeat(30); // 90 bytes
    let summary = ancestors_summary(&[fp("AXGroup", Some(&long_title), false)]);
    let quoted = summary.split('“').nth(1).unwrap().trim_end_matches('”');
    assert!(quoted.len() <= ANCESTOR_TITLE_MAX_BYTES);
    assert!(quoted.chars().all(|c| c == '好'));
    assert_eq!(quoted.chars().count(), 13); // 13 * 3 = 39 <= 40 < 42

    let summary = ancestors_summary(&[fp("AXGroup", Some("a\nb\tc"), false)]);
    assert_eq!(summary, "AXGroup “a b c”");
}

#[test]
fn ancestors_summary_drops_the_farthest_segments_to_fit_256_bytes() {
    let ancestors: Vec<_> = (0..40)
        .map(|index| fp(&format!("AXRole{index:02}"), Some("title-title-title"), false))
        .collect();
    let summary = ancestors_summary(&ancestors);
    assert!(summary.len() <= ANCESTORS_MAX_BYTES, "{}", summary.len());
    assert!(summary.starts_with("… › "));
    assert!(summary.ends_with("AXRole39 “title-title-title”"));
    assert!(!summary.contains("AXRole00"));

    // A single enormous nearest segment is cut on a char boundary.
    let huge = fp(&"é".repeat(200), None, false);
    let summary = ancestors_summary(&[fp("AXGroup", None, false), huge]);
    assert!(summary.len() <= ANCESTORS_MAX_BYTES);
    assert!(summary.starts_with("… › "));
    assert!(std::str::from_utf8(summary.as_bytes()).is_ok());
}

// ---------------------------------------------------------------------------
// ComputerRuntime registry integration (no native calls)
// ---------------------------------------------------------------------------

mod runtime {
    use super::*;
    use crate::{
        ComputerConfig, ComputerRuntime, ElementFindRequest, SurfaceRecord, WebAccessibilityPolicy,
    };

    fn runtime() -> ComputerRuntime {
        ComputerRuntime::new(ComputerConfig {
            max_encoded_image_bytes: usize::MAX,
            web_accessibility: WebAccessibilityPolicy::Auto,
        })
    }

    fn surface(application: &str, title: &str) -> SurfaceRecord {
        SurfaceRecord {
            native_id: 1,
            pid: 1,
            identity_hash: [0; 32],
            application: application.to_string(),
            title: title.to_string(),
            width: 800,
            height: 600,
        }
    }

    fn commit(
        runtime: &ComputerRuntime,
        surface_id: &str,
        record: &SurfaceRecord,
        result: AccessibilityTreeResult,
    ) -> Value {
        runtime
            .finish_accessibility_observation(surface_id, record, result.output, result.elements)
            .unwrap()
    }

    fn element_id_for(output: &Value, title: &str) -> String {
        let nodes = output.get("nodes").or_else(|| output.get("elements")).unwrap();
        nodes
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["title"] == json!(title))
            .unwrap_or_else(|| panic!("no node titled {title}"))["element_id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn web_context_carries_policy_and_surface_sensitivity() {
        let off = ComputerRuntime::new(ComputerConfig {
            max_encoded_image_bytes: usize::MAX,
            web_accessibility: WebAccessibilityPolicy::Off,
        });
        assert_eq!(off.web_context(false).policy, WebAccessibilityPolicy::Off);
        assert!(off.web_context(true).sensitive_surface);
        let auto = runtime();
        assert_eq!(auto.web_context(false).policy, WebAccessibilityPolicy::Auto);
        assert!(!auto.web_context(false).sensitive_surface);
    }

    #[test]
    fn query_commits_replace_the_surface_generation_and_reissue_the_root() {
        let runtime = runtime();
        let record = surface("Browser", "Page");
        runtime.insert_surface_for_test("surface_a", record.clone());
        let tree = sample_tree();

        let first = commit(
            &runtime,
            "surface_a",
            &record,
            observe_tree(&tree, 0, "surface_a", None, bounds(8, 256), TreeMode::Query).unwrap(),
        );
        assert_eq!(first["observation_generation"], json!(1));
        let toolbar_id = element_id_for(&first, "Toolbar");

        // Subtree below Toolbar: ids for the whole surface are replaced.
        let (_, root) = runtime.prepare_query("surface_a", Some(&toolbar_id)).unwrap();
        let root = root.unwrap();
        let second = commit(
            &runtime,
            "surface_a",
            &record,
            observe_tree(&tree, 1, "surface_a", Some(&root), bounds(4, 16), TreeMode::Query)
                .unwrap(),
        );
        assert_eq!(second["observation_generation"], json!(2));
        let new_root_id = second["root"]["element_id"].as_str().unwrap().to_string();
        assert_ne!(new_root_id, toolbar_id);
        assert_eq!(second["nodes"][0]["element_id"], json!(new_root_id));
        // The previous generation's handle, including the root that was passed in, is stale.
        let error = runtime
            .prepare_query("surface_a", Some(&toolbar_id))
            .err()
            .unwrap();
        assert!(error.starts_with("stale_element:"), "{error}");
        assert!(runtime.prepare_query("surface_a", Some(&new_root_id)).is_ok());

        // Find with the re-issued root re-issues it again and bumps the generation.
        let found = commit(
            &runtime,
            "surface_a",
            &record,
            find(
                &tree,
                &tree,
                1,
                "surface_a",
                Some(&root),
                &query_for_role("AXButton"),
                find_bounds(8),
            )
            .unwrap(),
        );
        assert_eq!(found["observation_generation"], json!(3));
        let reissued = found["root_element_id"].as_str().unwrap().to_string();
        assert_ne!(reissued, new_root_id);
        assert!(runtime.prepare_query("surface_a", Some(&reissued)).is_ok());
        assert!(runtime
            .prepare_query("surface_a", Some(&new_root_id))
            .err()
            .unwrap()
            .starts_with("stale_element:"));
        // The match is registered and usable as a handle for later calls.
        let share_id = element_id_for(&found, "Share");
        assert!(runtime.prepare_query("surface_a", Some(&share_id)).is_ok());
    }

    #[test]
    fn query_root_from_another_surface_or_unknown_is_stale_element() {
        let runtime = runtime();
        let a = surface("Browser", "A");
        let b = surface("Browser", "B");
        runtime.insert_surface_for_test("surface_a", a.clone());
        runtime.insert_surface_for_test("surface_b", b.clone());
        let tree = sample_tree();
        let b_output = commit(
            &runtime,
            "surface_b",
            &b,
            observe_tree(&tree, 0, "surface_b", None, bounds(8, 256), TreeMode::Query).unwrap(),
        );
        let b_id = element_id_for(&b_output, "Toolbar");
        for root in [b_id.as_str(), "element_neverissued"] {
            let error = runtime.prepare_query("surface_a", Some(root)).err().unwrap();
            assert!(error.starts_with("stale_element:"), "{error}");
            let error = runtime
                .accessibility_subtree("surface_a", Some(root), 4, 16)
                .unwrap_err();
            assert!(error.starts_with("stale_element:"), "{error}");
        }
        let error = runtime
            .prepare_query("surface_missing", None)
            .err()
            .unwrap();
        assert!(error.starts_with("stale_surface:"), "{error}");
    }

    #[test]
    fn protected_secure_and_descendant_roots_are_refused_before_any_native_call() {
        let runtime = runtime();
        let record = surface("Browser", "Login");
        runtime.insert_surface_for_test("surface_a", record.clone());
        let tree = sample_tree();
        let output = commit(
            &runtime,
            "surface_a",
            &record,
            observe_tree(&tree, 0, "surface_a", None, bounds(8, 256), TreeMode::Query).unwrap(),
        );
        let nodes = output["nodes"].as_array().unwrap();
        let ids_by_role_title = |role: &str, title: Option<&str>| -> String {
            nodes
                .iter()
                .find(|node| {
                    node["role"] == json!(role)
                        && title.map_or(node["title"].is_null(), |title| node["title"] == json!(title))
                })
                .unwrap()["element_id"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let secure = ids_by_role_title("AXTextField", Some("Password"));
        // The Vault group and the static texts below protected/secure nodes carry no title.
        let protected = ids_by_role_title("AXGroup", None);
        let protected_child = nodes
            .iter()
            .filter(|node| node["role"] == json!("AXStaticText") && node["title"].is_null())
            .map(|node| node["element_id"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(protected_child.len(), 2);
        let mut refused = vec![secure, protected];
        refused.extend(protected_child);
        for root in refused {
            let error = runtime.prepare_query("surface_a", Some(&root)).err().unwrap();
            assert!(error.starts_with("permission_denied:"), "{error}");
            let error = runtime
                .accessibility_subtree("surface_a", Some(&root), 4, 16)
                .unwrap_err();
            assert!(error.starts_with("permission_denied:"), "{error}");
            let request = ElementFindRequest {
                root_element_id: Some(root),
                role: Some("AXButton".into()),
                limit: 8,
                max_depth: 32,
                ..ElementFindRequest::default()
            };
            let error = runtime.find_elements("surface_a", &request).unwrap_err();
            assert!(error.starts_with("permission_denied:"), "{error}");
        }
    }

    #[test]
    fn sensitive_surfaces_are_refused_before_any_native_call() {
        let runtime = runtime();
        runtime.insert_surface_for_test("surface_pw", surface("Passwords", "Passwords"));
        runtime.insert_surface_for_test("surface_auth", surface("Example", "Authorization Required"));
        let request = ElementFindRequest {
            role: Some("AXButton".into()),
            limit: 8,
            max_depth: 32,
            ..ElementFindRequest::default()
        };
        for surface_id in ["surface_pw", "surface_auth"] {
            let expected = "permission_denied: sensitive Computer surface cannot be controlled";
            assert_eq!(
                runtime
                    .accessibility_subtree(surface_id, None, 4, 16)
                    .unwrap_err(),
                expected
            );
            assert_eq!(
                runtime.find_elements(surface_id, &request).unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn query_inputs_are_validated_before_the_registry_is_consulted() {
        let runtime = runtime();
        runtime.insert_surface_for_test("surface_a", surface("Browser", "Page"));
        for (surface_id, root) in [
            ("", None),
            ("surface_a", Some("bogus")),
            ("surface_a", Some("element_")),
            ("surface_a", Some(&"element_x".repeat(30)[..])),
        ] {
            let error = runtime
                .accessibility_subtree(surface_id, root, 4, 16)
                .unwrap_err();
            assert!(error.starts_with("invalid_request:"), "{error}");
        }
        for (max_depth, max_nodes) in [(9, 16), (4, 0), (4, 257)] {
            let error = runtime
                .accessibility_subtree("surface_a", None, max_depth, max_nodes)
                .unwrap_err();
            assert!(error.starts_with("invalid_request:"), "{error}");
        }
    }

    #[test]
    fn find_requests_are_validated_and_debug_redacts_the_value_needle() {
        let valid = ElementFindRequest {
            value: Some("needle-text".into()),
            limit: 8,
            max_depth: 32,
            ..ElementFindRequest::default()
        };
        assert!(valid.validate().is_ok());
        assert!(!format!("{valid:?}").contains("needle-text"));
        assert!(format!("{valid:?}").contains("value_present: true"));

        let empty = ElementFindRequest {
            limit: 8,
            max_depth: 32,
            ..ElementFindRequest::default()
        };
        assert!(empty.validate().unwrap_err().contains("at least one"));
        for invalid in [
            ElementFindRequest {
                value: Some(String::new()),
                ..valid.clone()
            },
            ElementFindRequest {
                value: Some("x".repeat(257)),
                ..valid.clone()
            },
            ElementFindRequest {
                role: Some("a\0b".into()),
                ..valid.clone()
            },
            ElementFindRequest {
                limit: 0,
                ..valid.clone()
            },
            ElementFindRequest {
                limit: 33,
                ..valid.clone()
            },
            ElementFindRequest {
                max_depth: 0,
                ..valid.clone()
            },
            ElementFindRequest {
                max_depth: 49,
                ..valid.clone()
            },
        ] {
            assert!(
                invalid.validate().unwrap_err().starts_with("invalid_request:"),
                "{invalid:?}"
            );
        }
        // A state-only filter is a valid query.
        assert!(ElementFindRequest {
            enabled: Some(true),
            limit: 1,
            max_depth: 1,
            ..ElementFindRequest::default()
        }
        .validate()
        .is_ok());
    }
}
