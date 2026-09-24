import test from "node:test";
import assert from "node:assert/strict";
import {
  formatUpdatedTime,
  formatSessionDateTime,
  formatLivenessPresentation,
  activityKindLabel,
  activityFacts,
  activityDescription,
  appendActivityPreview,
  createTimelineEvent,
  renderTimelineEvents,
} from "../dist/runtime_activity.js";

function createMockElement(tagName) {
  const children = [];
  const attributes = new Map();
  const eventListeners = new Map();
  const classListSet = new Set();

  const element = {
    tagName: tagName.toUpperCase(),
    style: {},
    dataset: {},
    scrollTop: 0,
    scrollHeight: 100,
    childNodes: children,
    children,
    parentElement: null,
    get className() {
      return Array.from(classListSet).join(" ");
    },
    set className(val) {
      classListSet.clear();
      String(val)
        .split(/\s+/)
        .filter(Boolean)
        .forEach((c) => classListSet.add(c));
    },
    textContent: "",

    classList: {
      add(...tokens) {
        tokens.forEach((t) => classListSet.add(t));
        element.className = Array.from(classListSet).join(" ");
      },
      remove(...tokens) {
        tokens.forEach((t) => classListSet.delete(t));
        element.className = Array.from(classListSet).join(" ");
      },
      contains(token) {
        return classListSet.has(token);
      },
      toggle(token, force) {
        if (typeof force === "boolean") {
          if (force) this.add(token);
          else this.remove(token);
          return force;
        }
        if (classListSet.has(token)) {
          this.remove(token);
          return false;
        }
        this.add(token);
        return true;
      },
    },

    get firstChild() {
      return children[0] || null;
    },

    appendChild(child) {
      if (!child) return child;
      child.parentElement = element;
      children.push(child);
      return child;
    },

    removeChild(child) {
      const idx = children.indexOf(child);
      if (idx !== -1) {
        children.splice(idx, 1);
        child.parentElement = null;
      }
      return child;
    },

    setAttribute(key, val) {
      attributes.set(key, String(val));
    },
    getAttribute(key) {
      return attributes.get(key) || null;
    },
    hasAttribute(key) {
      return attributes.has(key);
    },
    removeAttribute(key) {
      attributes.delete(key);
    },

    addEventListener(event, handler) {
      if (!eventListeners.has(event)) eventListeners.set(event, []);
      eventListeners.get(event).push(handler);
    },

    querySelector(selector) {
      const normalized = selector.trim();
      const matchClass = normalized.startsWith(".") ? normalized.slice(1) : null;
      for (const child of children) {
        if (matchClass && child.classList.contains(matchClass)) return child;
        if (child.tagName.toLowerCase() === normalized) return child;
        const found = child.querySelector?.(selector);
        if (found) return found;
      }
      return null;
    },
  };

  return element;
}

function withMockDom(run) {
  const previousDocument = globalThis.document;
  try {
    globalThis.document = {
      createElement(tag) {
        return createMockElement(tag);
      },
    };
    run();
  } finally {
    globalThis.document = previousDocument;
  }
}

test("formatUpdatedTime and formatSessionDateTime format timestamps or fall back", () => {
  assert.equal(formatUpdatedTime(null, "en"), "time unavailable");
  assert.equal(formatUpdatedTime(null, "zh-CN"), "时间不可用");
  assert.equal(formatSessionDateTime(undefined, "en"), "time unavailable");

  const formattedTime = formatUpdatedTime(1700000000, "en");
  assert.ok(typeof formattedTime === "string" && formattedTime.length > 0);

  const formattedDateTime = formatSessionDateTime(1700000000, "zh-CN");
  assert.ok(typeof formattedDateTime === "string" && formattedDateTime.length > 0);
});

test("formatLivenessPresentation localizes liveness presentation in Chinese", () => {
  const sessionIdle = {
    running_call: false,
    running_jobs: 0,
    running_jobs_complete: true,
  };
  const enPres = formatLivenessPresentation(sessionIdle, "en");
  assert.equal(enPres.state, "idle");
  assert.equal(enPres.label, "idle");

  const zhPres = formatLivenessPresentation(sessionIdle, "zh-CN");
  assert.equal(zhPres.state, "idle");
  assert.equal(zhPres.label, "空闲");
});

test("activityKindLabel translates and formats kinds and job handoffs", () => {
  assert.equal(activityKindLabel({ kind: "Edited" }, "en"), "Edited");
  assert.equal(activityKindLabel({ kind: "Edited" }, "zh-CN"), "编辑");
  assert.equal(activityKindLabel({ kind: "Tested", job_handoff: true }, "zh-CN"), "测试");
  assert.equal(activityKindLabel({ kind: "Ran", job_handoff: true }, "zh-CN"), "命令");
  assert.equal(activityKindLabel({ kind: "Explored", group_count: 5 }, "en"), "Explored ×5");
  assert.equal(activityKindLabel({ kind: "Explored", group_count: 5 }, "zh-CN"), "探索 ×5");
});

test("activityFacts collects group kinds, timing, job handoff, and tool info", () => {
  const groupActivity = {
    group_count: 3,
    group_kinds: ["read", "write"],
    group_tools: ["file_view", "file_edit"],
  };
  const facts = activityFacts(groupActivity, false, "en");
  assert.deepEqual(facts, ["read / write", "file_view, file_edit"]);

  const handoffActivity = {
    tool: "shell",
    job_handoff: true,
    execution_state: "running",
    job_id: "job-99",
    started_at: 1700000000,
  };
  const handoffFacts = activityFacts(handoffActivity, true, "en");
  assert.ok(handoffFacts.includes("shell"));
  assert.ok(handoffFacts.includes("handed off"));
  assert.ok(handoffFacts.includes("execution running"));
  assert.ok(handoffFacts.includes("job job-99"));
  assert.equal(handoffFacts.length, 5);
});

test("activityDescription combines kind, facts, and summary", () => {
  const activity = {
    kind: "Edited",
    tool: "file_edit",
    summary: "Updated index.ts",
  };
  const desc = activityDescription(activity, "en");
  assert.equal(desc, "Edited · file_edit · Updated index.ts");
});

test("appendActivityPreview creates structured preview DOM without innerHTML", () => {
  withMockDom(() => {
    const parent = createMockElement("div");
    const activity = {
      kind: "Edited",
      tool: "file_edit",
      summary: "Updated index.ts",
    };
    appendActivityPreview(parent, "Recent: ", activity, "en");
    assert.equal(parent.children.length, 1);
    const row = parent.children[0];
    assert.ok(row.classList.contains("activity-preview"));
    assert.equal(row.querySelector(".activity-preview-label")?.textContent, "Recent: ");
  });
});

test("createTimelineEvent and renderTimelineEvents create valid DOM elements", () => {
  withMockDom(() => {
    const activity = {
      kind: "Progress",
      summary: "Compiling sources",
      paths: ["src/main.rs", "src/lib.rs"],
    };
    const eventEl = createTimelineEvent(activity, "en");
    assert.ok(eventEl.classList.contains("timeline-event"));
    assert.ok(eventEl.classList.contains("reported-progress"));
    assert.equal(eventEl.querySelector(".timeline-kind")?.textContent, "Progress");
    assert.equal(eventEl.querySelector(".timeline-body")?.textContent, "Compiling sources");

    const failedActivity = {
      kind: "Ran",
      state: "failed",
      summary: "Compilation failed",
    };
    const failedEl = createTimelineEvent(failedActivity, "en");
    assert.ok(failedEl.classList.contains("failed"));

    const container = createMockElement("ul");
    renderTimelineEvents(container, [activity, failedActivity], "en");
    assert.equal(container.children.length, 2);
  });
});
