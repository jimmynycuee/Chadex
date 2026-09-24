import test from "node:test";
import assert from "node:assert/strict";
import {
  collaborationPhaseLabel,
  formatComposerOptionSummary,
  runtimeSearchMatches,
  filterCollaborationCards,
  syncCollaborationComposerLayout,
  renderLatestAgentMessage,
  renderCollaborationMessageCards,
} from "../dist/runtime_collaboration.js";

function createMockElement(tag = "div") {
  return {
    tagName: tag.toUpperCase(),
    className: "",
    textContent: "",
    dataset: {},
    children: [],
    childNodes: [],
    attributes: {},
    style: {},
    scrollHeight: 44,
    value: "",
    hidden: false,
    classList: {
      classes: new Set(),
      add(...tokens) { for (const t of tokens) this.classes.add(t); },
      remove(...tokens) { for (const t of tokens) this.classes.delete(t); },
      toggle(cls, force) {
        if (force === undefined) {
          if (this.classes.has(cls)) { this.classes.delete(cls); return false; }
          this.classes.add(cls); return true;
        }
        if (force) { this.classes.add(cls); return true; }
        this.classes.delete(cls); return false;
      },
      has(cls) { return this.classes.has(cls); },
    },
    appendChild(child) {
      this.children.push(child);
      this.childNodes.push(child);
      return child;
    },
    removeChild(child) {
      const idx = this.children.indexOf(child);
      if (idx >= 0) this.children.splice(idx, 1);
      const cIdx = this.childNodes.indexOf(child);
      if (cIdx >= 0) this.childNodes.splice(cIdx, 1);
      return child;
    },
    get firstChild() {
      return this.childNodes[0] || null;
    },
    setAttribute(key, value) { this.attributes[key] = value; },
    getAttribute(key) { return this.attributes[key] ?? null; },
    addEventListener() {},
  };
}

function withMockDom(fn) {
  const originalDocument = globalThis.document;
  globalThis.document = {
    createElement(tag) { return createMockElement(tag); },
    createElementNS(_ns, tag) { return createMockElement(tag); },
    createTextNode(text) { return { nodeType: 3, textContent: text }; },
  };
  try {
    return fn();
  } finally {
    globalThis.document = originalDocument;
  }
}

test("collaborationPhaseLabel translates known and fallback phases", () => {
  assert.equal(collaborationPhaseLabel("live"), "Live");
  assert.equal(collaborationPhaseLabel("reconnecting"), "Reconnecting");
  assert.equal(collaborationPhaseLabel("paused"), "Paused");
  assert.equal(collaborationPhaseLabel("idle"), "Idle");
  assert.equal(collaborationPhaseLabel("unknown"), "Idle");

  assert.equal(collaborationPhaseLabel("live", "zh-CN"), "实时");
  assert.equal(collaborationPhaseLabel("reconnecting", "zh-CN"), "正在重连");
  assert.equal(collaborationPhaseLabel("paused", "zh-CN"), "已暂停");
  assert.equal(collaborationPhaseLabel("idle", "zh-CN"), "空闲");
});

test("formatComposerOptionSummary summarizes kind, priority, and ack requirements", () => {
  const defaultSummary = formatComposerOptionSummary("note", "normal", false);
  assert.equal(defaultSummary.label, "Options");
  assert.equal(defaultSummary.hasSelection, false);

  const guidanceSummary = formatComposerOptionSummary("guidance", "high", true);
  assert.equal(guidanceSummary.label, "guidance · high · ACK");
  assert.equal(guidanceSummary.hasSelection, true);

  const zhSummary = formatComposerOptionSummary("guidance", "high", true, "zh-CN");
  assert.equal(zhSummary.label, "指导 · 高 · 需确认");
  assert.equal(zhSummary.hasSelection, true);

  const questionSummary = formatComposerOptionSummary("question", "normal", false);
  assert.equal(questionSummary.label, "question");
  assert.equal(questionSummary.hasSelection, true);

  const priorityOnly = formatComposerOptionSummary("note", "urgent", false);
  assert.equal(priorityOnly.label, "urgent");
  assert.equal(priorityOnly.hasSelection, true);
});

test("runtimeSearchMatches matches queries across multiple values", () => {
  assert.equal(runtimeSearchMatches("", ["any", "values"]), true);
  assert.equal(runtimeSearchMatches("  ", ["any"]), true);
  assert.equal(runtimeSearchMatches("test", ["A quick test message"]), true);
  assert.equal(runtimeSearchMatches("TEST", ["a quick test message"]), true);
  assert.equal(runtimeSearchMatches("failed build", ["Build output", null, "failed with error 1"]), true);
  assert.equal(runtimeSearchMatches("missing word", ["Build output", "failed"]), false);
  assert.equal(runtimeSearchMatches("term", [123, undefined, null]), false);
});

test("filterCollaborationCards searches retained fields without mutating messages", () => {
  const messages = [
    { message_id: "m1", message: "Task 1 complete", resolution: "Fixed Unicode 路径" },
    { message_id: "m2", message: "Fix build error", resolution: "Patched" },
    { message_id: "m3", message: "Review pending", author_session_id: "worker-1" },
  ];
  const original = JSON.stringify(messages);
  const cards = messages.map((m) => {
    const el = createMockElement("article");
    el.dataset.messageId = m.message_id;
    return el;
  });
  const separators = [createMockElement("div"), createMockElement("div")];

  const resultAll = filterCollaborationCards(cards, separators, messages, "");
  assert.deepEqual(resultAll, { matches: 3, total: 3 });
  assert.deepEqual(cards.map((c) => c.hidden), [false, false, false]);
  assert.deepEqual(separators.map((s) => s.hidden), [false, false]);

  const resultResolution = filterCollaborationCards(cards, separators, messages, "FIXED 路径");
  assert.deepEqual(resultResolution, { matches: 1, total: 3 });
  assert.deepEqual(cards.map((c) => c.hidden), [false, true, true]);
  assert.deepEqual(separators.map((s) => s.hidden), [true, true]);

  const resultFiltered = filterCollaborationCards(cards, separators, messages, "build");
  assert.deepEqual(resultFiltered, { matches: 1, total: 3 });
  assert.deepEqual(cards.map((c) => c.hidden), [true, false, true]);

  const resultAuthor = filterCollaborationCards(cards, separators, messages, "worker-1");
  assert.deepEqual(resultAuthor, { matches: 1, total: 3 });
  assert.deepEqual(cards.map((c) => c.hidden), [true, true, false]);
  assert.equal(JSON.stringify(messages), original);
});

test("syncCollaborationComposerLayout adjusts classes and heights", () => {
  const body = createMockElement("textarea");
  const composer = createMockElement("form");
  const send = createMockElement("button");

  body.value = "";
  body.scrollHeight = 30;
  syncCollaborationComposerLayout(body, composer, send);
  assert.equal(composer.classList.has("has-content"), false);
  assert.equal(send.classList.has("is-ready"), false);
  assert.equal(body.style.height, "44px");
  assert.equal(body.style.overflowY, "hidden");

  body.value = "Some meaningful message";
  body.scrollHeight = 100;
  syncCollaborationComposerLayout(body, composer, send);
  assert.equal(composer.classList.has("has-content"), true);
  assert.equal(send.classList.has("is-ready"), true);
  assert.equal(body.style.height, "100px");
  assert.equal(body.style.overflowY, "hidden");

  body.scrollHeight = 250;
  syncCollaborationComposerLayout(body, composer, send);
  assert.equal(body.style.height, "180px");
  assert.equal(body.style.overflowY, "auto");
});

test("renderLatestAgentMessage renders latest incoming message or empty notice", () => {
  withMockDom(() => {
    const container = createMockElement("div");
    const locallyAuthored = new Set(["human-1"]);
    const messages = [
      { message_id: "human-1", message: "Hello", created_at: 100 },
      { message_id: "agent-1", author_session_id: "agent:runner:1", message: "Understood", created_at: 105 },
      { message_id: "agent-superseded", author_session_id: "agent:runner:1", message: "Old reply", created_at: 102, superseded_by_message_id: "agent-1" },
    ];

    renderLatestAgentMessage(container, messages, locallyAuthored);
    assert.equal(container.children.length > 0, true);

    const emptyContainer = createMockElement("div");
    renderLatestAgentMessage(emptyContainer, [], locallyAuthored, "en");
    assert.match(emptyContainer.textContent, /No Agent message in the retained window/);

    const emptyContainerZh = createMockElement("div");
    renderLatestAgentMessage(emptyContainerZh, [], locallyAuthored, "zh-CN");
    assert.match(emptyContainerZh.textContent, /当前保留范围内暂无 Agent 留言/);
  });
});

test("renderCollaborationMessageCards creates threaded DOM cards with actions", () => {
  withMockDom(() => {
    const board = createMockElement("div");
    const locallyAuthored = new Set(["msg-human-1"]);
    const previousRendered = new Set(["msg-agent-1"]);
    let replied = null;
    let edited = null;
    let withdrawn = null;

    const messages = [
      {
        message_id: "msg-agent-1",
        author_session_id: "agent:runner:1",
        kind: "note",
        message: "First agent message",
        created_at: 100,
      },
      {
        message_id: "msg-human-1",
        reply_to: "msg-agent-1",
        kind: "guidance",
        priority: "high",
        status: "open",
        requires_ack: true,
        message: "Please focus on auth module",
        created_at: 110,
      },
    ];

    renderCollaborationMessageCards(board, messages, {
      locallyAuthoredIds: locallyAuthored,
      previouslyRenderedMessageIds: previousRendered,
      canMutate: true,
      language: "en",
      onReply: (id) => { replied = id; },
      onEdit: (msg) => { edited = msg; },
      onWithdraw: (id) => { withdrawn = id; },
    });

    assert.equal(board.children.length >= 2, true);
    const card1 = board.children.find((c) => c.dataset?.messageId === "msg-agent-1");
    const card2 = board.children.find((c) => c.dataset?.messageId === "msg-human-1");
    assert.ok(card1);
    assert.ok(card2);

    assert.equal(card1.classList.has("agent-authored"), true);
    assert.equal(card1.classList.has("message-incoming"), true);
    assert.equal(card1.classList.has("message-entering"), false);

    assert.equal(card2.classList.has("human-authored"), true);
    assert.equal(card2.classList.has("message-outgoing"), true);
    assert.equal(card2.classList.has("message-entering"), true);
    assert.equal(card2.classList.has("message-thread"), true);
  });
});
