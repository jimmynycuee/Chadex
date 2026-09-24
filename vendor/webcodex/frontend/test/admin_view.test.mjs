import test from "node:test";
import assert from "node:assert/strict";
import { capabilityLabels, renderAdminDashboard } from "../dist/admin_view.js";

class Node {
  constructor(tag = "div") {
    this.tag = tag;
    this.textContent = "";
    this.hidden = false;
    this.className = "";
    this.children = [];
    this.attributes = {};
  }
  get firstChild() { return this.children[0] || null; }
  appendChild(child) { this.children.push(child); return child; }
  append(...children) { this.children.push(...children); }
  removeChild(child) { this.children.splice(this.children.indexOf(child), 1); return child; }
  setAttribute(name, value) { this.attributes[name] = value; }
  addEventListener() {}
}

class FakeDocument {
  constructor() {
    this.nodes = new Map();
    for (const id of [
      "overview", "overview-error", "devices", "devices-error", "devices-empty",
      "projects", "projects-error", "projects-empty", "diagnostics", "activity",
      "activity-error", "activity-empty",
    ]) this.nodes.set(id, new Node());
  }
  getElementById(id) { return this.nodes.get(id) || null; }
  createElement(tag) { return new Node(tag); }
}

function rowText(node) {
  return node.children.map((cell) => cell.children[0]?.textContent || "");
}

function populated() {
  return {
    section_status: {
      overview: { status: "ok", error: null },
      devices: { status: "ok", error: null },
      projects: { status: "ok", error: null },
      activity: { status: "ok", error: null },
    },
    overview: { version: "1", build_commit: "abc", version_compatibility: "version_mismatch" },
    diagnostics: { server_transport: "ready" },
    devices: [
      { client_id: "a", capabilities: ["git", "shell"], compatibility: "compatible" },
      { client_id: "b", capabilities: { shell: true, patch: false, git: true }, compatibility: "version_mismatch" },
      { client_id: "c", capabilities: 42, compatibility: "unknown" },
    ],
    projects: [
      { id: "agent:a:p", client_id: "a", compatibility: "compatible", actions: { enable: false, disable: true, unregister: true } },
      { id: "agent:b:p", client_id: "b", compatibility: "version_mismatch", actions: { enable: true, disable: false, unregister: false } },
    ],
    activity: [{ kind: "run_shell", status: "ok" }],
  };
}

test("admin renderer handles populated agents and mixed compatibility", () => {
  const doc = new FakeDocument();
  assert.doesNotThrow(() => renderAdminDashboard(doc, populated()));
  const devices = doc.getElementById("devices");
  assert.equal(devices.children.length, 3);
  assert.equal(rowText(devices.children[0])[6], "git, shell");
  assert.equal(rowText(devices.children[1])[6], "git, shell");
  assert.equal(rowText(devices.children[2])[6], "—");
  assert.equal(rowText(devices.children[0])[9], "compatible");
  assert.equal(rowText(devices.children[1])[9], "version_mismatch");
  const projects = doc.getElementById("projects");
  assert.equal(rowText(projects.children[0])[9], "compatible");
  assert.equal(rowText(projects.children[1])[9], "version_mismatch");
});

test("section error preserves prior successful DOM and other sections render", () => {
  const doc = new FakeDocument();
  renderAdminDashboard(doc, populated());
  const priorDevices = doc.getElementById("devices").children.length;
  const update = populated();
  update.section_status.devices = { status: "error", error: "devices unavailable" };
  update.devices = [];
  update.projects.push({ id: "agent:c:p", client_id: "c", compatibility: "unknown" });
  renderAdminDashboard(doc, update);
  assert.equal(doc.getElementById("devices").children.length, priorDevices);
  assert.equal(doc.getElementById("devices-error").hidden, false);
  assert.equal(doc.getElementById("devices-error").textContent, "devices unavailable");
  assert.equal(doc.getElementById("projects").children.length, 3);
});

test("capability normalization is defensive", () => {
  assert.deepEqual(capabilityLabels(["shell", 1, "git"]), ["shell", "git"]);
  assert.deepEqual(capabilityLabels({ shell: true, patch: false, git: true }), ["git", "shell"]);
  assert.deepEqual(capabilityLabels(null), []);
});
