import test from "node:test";
import assert from "node:assert/strict";
import { app, flush, toolResult } from "./app_test_support.mjs";

const project = "agent:special:demo";
const session_id = `wc_sess_${"1".repeat(32)}`;
const snapshot_id = `wc_changes_snapshot_${"2".repeat(32)}`;
const input = { project, session_id };

function file(index, overrides = {}) {
  return {
    path: `src/file_${index}.rs`,
    kind: "modified",
    additions: index + 1,
    deletions: index,
    binary: false,
    ...overrides,
  };
}

const snapshot = {
  version: 3,
  project,
  session_id,
  snapshot_id,
  files_changed: 7,
  additions: 35,
  deletions: 21,
  files_total: 7,
  files_returned: 7,
  files_truncated: false,
  files: [
    file(0),
    file(1, { path: "src/new.rs", kind: "added", additions: 8, deletions: 0 }),
    file(2, { path: "src/removed.rs", kind: "deleted", additions: 0, deletions: 9 }),
    file(3, { path: "src/new_name.rs", previous_path: "src/old_name.rs", kind: "renamed", additions: 2, deletions: 1 }),
    file(4, { path: "assets/blob.bin", binary: true, additions: null, deletions: null }),
    file(5),
    file(6),
  ],
};

function firstFileNodes(view) {
  const root = view.nodes.files.children[0];
  const button = root.children[0];
  const wrap = root.children[1];
  return { root, button, wrap, state: wrap.children[0], pre: wrap.children[1] };
}

for (const first of ["input", "result"]) {
  test(`Final Changes ${first}-first bootstrap renders metadata without lazy reads`, async () => {
    const view = app("mcp_changes_app.html");
    if (first === "input") view.toolInput(input);
    else view.toolResult({ changes: snapshot });
    assert.equal(view.calls("changes_file_diff").length, 0);
    await view.initialize();
    if (first === "input") view.toolResult({ changes: snapshot });
    else view.toolInput(input);
    await flush();

    assert.equal(view.calls("changes_file_diff").length, 0);
    assert.equal(view.nodes.badge.textContent, "Frozen");
    assert.equal(view.nodes.summary.textContent, "Changed 7 files");
    assert.equal(view.nodes.files.children.length, 5);
    assert.equal(view.nodes.more.hidden, false);
    assert.equal(view.nodes.more.textContent, "Show 2 more files");
    assert.equal(view.timers.size, 0);
  });
}

test("user expansion performs one exact lazy read and caches the frozen diff", async () => {
  const view = app("mcp_changes_app.html");
  view.toolInput(input);
  view.toolResult({ changes: snapshot });
  await view.initialize();

  let nodes = firstFileNodes(view);
  nodes.button.onclick();
  await flush();
  assert.equal(view.calls("changes_file_diff").length, 1);
  assert.deepEqual(
    { ...view.calls("changes_file_diff")[0].params.arguments },
    { project, session_id, snapshot_id, path: "src/file_0.rs" },
  );
  await view.reply(
    view.calls("changes_file_diff")[0],
    toolResult({
      changes_file_diff: {
        version: 1,
        project,
        session_id,
        snapshot_id,
        path: "src/file_0.rs",
        previous_path: null,
        kind: "modified",
        binary: false,
        diff: "diff --git a/src/file_0.rs b/src/file_0.rs\n@@ -1 +1 @@\n-old\n+new\n",
        bytes_total: 78,
        bytes_returned: 78,
        lines_total: 5,
        lines_returned: 5,
        truncated: false,
      },
    }),
  );
  nodes = firstFileNodes(view);
  assert.equal(nodes.state.textContent, "Frozen diff");
  assert.equal(nodes.pre.children.some(line => line.textContent === "+new" && line.className.includes("added")), true);
  assert.equal(nodes.pre.children.some(line => line.textContent === "-old" && line.className.includes("deleted")), true);

  nodes.button.onclick();
  nodes.button.onclick();
  await flush();
  assert.equal(view.calls("changes_file_diff").length, 1, "re-expansion must use the card-local frozen diff cache");
});

test("Show more expands only bounded initial metadata and does not read diffs", async () => {
  const view = app("mcp_changes_app.html");
  view.toolResult({ changes: snapshot });
  await view.initialize();
  view.toolInput(input);
  view.nodes.more.onclick();
  await flush();
  assert.equal(view.nodes.files.children.length, 7);
  assert.equal(view.nodes.more.hidden, true);
  assert.equal(view.calls("changes_file_diff").length, 0);
});

test("truncated lazy diff is labeled truthfully", async () => {
  const view = app("mcp_changes_app.html");
  view.toolResult({ changes: snapshot });
  await view.initialize();
  view.toolInput(input);
  const nodes = firstFileNodes(view);
  nodes.button.onclick();
  await flush();
  await view.reply(
    view.calls("changes_file_diff")[0],
    toolResult({
      changes_file_diff: {
        version: 1,
        project,
        session_id,
        snapshot_id,
        path: "src/file_0.rs",
        previous_path: null,
        kind: "modified",
        binary: false,
        diff: "@@ -1 +1 @@\n-old\n+new\n",
        bytes_total: 50000,
        bytes_returned: 24,
        lines_total: 2000,
        lines_returned: 3,
        truncated: true,
      },
    }),
  );
  assert.match(firstFileNodes(view).state.textContent, /truncated \(24\/50000 bytes\)/);
});

for (const first of ["input", "result"]) {
  test(`conflicting Changes ${first}-first identity fails closed without a lazy read`, async () => {
    const view = app("mcp_changes_app.html");
    if (first === "input") view.toolInput(input);
    else view.toolResult({ changes: snapshot });
    await view.initialize();
    const foreign = { project: "agent:special:other", session_id: `wc_sess_${"3".repeat(32)}` };
    if (first === "input") view.toolResult({ changes: { ...snapshot, ...foreign } });
    else view.toolInput(foreign);
    await flush();
    assert.equal(view.calls("changes_file_diff").length, 0);
    assert.equal(view.nodes.summary.textContent, "Changes unavailable");
    assert.match(view.nodes.footer.textContent, /Invalid|conflicting/);
    assert.equal(view.timers.size, 0);
  });
}

test("mismatched lazy response fails closed", async () => {
  const view = app("mcp_changes_app.html");
  view.toolInput(input);
  view.toolResult({ changes: snapshot });
  await view.initialize();
  firstFileNodes(view).button.onclick();
  await flush();
  await view.reply(
    view.calls("changes_file_diff")[0],
    toolResult({
      changes_file_diff: {
        version: 1,
        project,
        session_id,
        snapshot_id,
        path: "src/other.rs",
        previous_path: null,
        kind: "modified",
        binary: false,
        diff: "",
        bytes_total: 0,
        bytes_returned: 0,
        lines_total: 0,
        lines_returned: 0,
        truncated: false,
      },
    }),
  );
  assert.equal(view.nodes.summary.textContent, "Changes unavailable");
  assert.equal(view.timers.size, 0);
});

for (const method of ["ui/resource-teardown", "pagehide", "beforeunload"]) {
  test(`${method} ignores a late in-flight frozen diff response`, async () => {
    const view = app("mcp_changes_app.html");
    view.toolInput(input);
    view.toolResult({ changes: snapshot });
    await view.initialize();
    firstFileNodes(view).button.onclick();
    await flush();
    const request = view.calls("changes_file_diff")[0];
    await view.teardown(method);
    await view.reply(
      request,
      toolResult({
        changes_file_diff: {
          version: 1,
          project,
          session_id,
          snapshot_id,
          path: "src/file_0.rs",
          previous_path: null,
          kind: "modified",
          binary: false,
          diff: "+late\n",
          bytes_total: 6,
          bytes_returned: 6,
          lines_total: 1,
          lines_returned: 1,
          truncated: false,
        },
      }),
    );
    assert.equal(view.calls("changes_file_diff").length, 1);
    assert.equal(view.timers.size, 0);
  });
}
