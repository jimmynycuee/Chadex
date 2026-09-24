import test from "node:test";
import assert from "node:assert/strict";
import {
  communicationTimeLabel,
  parseAgentIds,
  deliveryAgentLabel,
  createAgentRow,
  renderAgentRows,
  createConversationRow,
  renderConversationRows,
  createConversationMessageCard,
  renderConversationMessages,
  createInboxDeliveryCard,
  renderInboxDeliveryCards,
} from "../dist/runtime_communication.js";

function createMockElement(tag = "div") {
  const listeners = new Map();
  let directText = "";
  const el = {
    tagName: tag.toUpperCase(),
    className: "",
    title: "",
    dataset: {},
    children: [],
    childNodes: [],
    attributes: {},
    get textContent() {
      if (this.childNodes.length === 0) return directText;
      return this.childNodes.map((c) => (typeof c === "string" ? c : c.textContent || "")).join("");
    },
    set textContent(val) {
      directText = String(val);
      this.childNodes = [];
      this.children = [];
    },
    get firstChild() {
      return this.childNodes[0] || null;
    },
    get childElementCount() {
      return this.children.length;
    },
    classList: {
      classes: new Set(),
      add(cls) {
        cls.split(/\s+/).filter(Boolean).forEach((c) => this.classes.add(c));
      },
      remove(cls) {
        this.classes.delete(cls);
      },
      contains(cls) {
        return this.classes.has(cls);
      },
      toggle(cls, force) {
        if (force === undefined) {
          if (this.classes.has(cls)) {
            this.classes.delete(cls);
            return false;
          }
          this.classes.add(cls);
          return true;
        }
        if (force) {
          this.classes.add(cls);
          return true;
        }
        this.classes.delete(cls);
        return false;
      },
    },
    appendChild(child) {
      if (child.className) {
        child.classList.add(child.className);
      }
      this.children.push(child);
      this.childNodes.push(child);
      return child;
    },
    removeChild(child) {
      const idx = this.childNodes.indexOf(child);
      if (idx >= 0) this.childNodes.splice(idx, 1);
      const cidx = this.children.indexOf(child);
      if (cidx >= 0) this.children.splice(cidx, 1);
      return child;
    },
    setAttribute(key, value) {
      this.attributes[key] = String(value);
      if (key === "class") this.classList.add(String(value));
    },
    getAttribute(key) {
      return this.attributes[key] ?? null;
    },
    addEventListener(event, handler) {
      if (!listeners.has(event)) listeners.set(event, []);
      listeners.get(event).push(handler);
    },
    click() {
      const list = listeners.get("click") || [];
      for (const handler of list) handler({ type: "click" });
    },
    querySelector(selector) {
      const results = this.querySelectorAll(selector);
      return results[0] || null;
    },
    querySelectorAll(selector) {
      const found = [];
      const match = (elem) => {
        if (selector.startsWith(".")) {
          const cls = selector.slice(1);
          if (elem.classList?.contains(cls) || (elem.className && elem.className.includes(cls))) {
            found.push(elem);
          }
        } else if (selector.toLowerCase() === elem.tagName.toLowerCase()) {
          found.push(elem);
        }
        for (const c of elem.children) match(c);
      };
      for (const child of this.children) match(child);
      return found;
    },
  };
  return el;
}

function withMockDom(fn) {
  const originalDoc = globalThis.document;
  globalThis.document = {
    createElement(tag) {
      return createMockElement(tag);
    },
  };
  try {
    return fn();
  } finally {
    globalThis.document = originalDoc;
  }
}

test("communicationTimeLabel formats milliseconds or shows fallback", () => {
  assert.equal(communicationTimeLabel(null), "time unavailable");
  assert.equal(communicationTimeLabel(undefined), "time unavailable");
  assert.equal(communicationTimeLabel("abc"), "time unavailable");
  assert.equal(communicationTimeLabel(null, "zh-CN"), "时间不可用");

  const formatted = communicationTimeLabel(1700000000000, "en");
  assert.ok(formatted.length > 0 && formatted !== "time unavailable");
});

test("parseAgentIds extracts distinct trimmed IDs", () => {
  assert.deepEqual(parseAgentIds("agent-1, agent-2  agent-3, agent-1"), [
    "agent-1",
    "agent-2",
    "agent-3",
  ]);
  assert.deepEqual(parseAgentIds("   "), []);
  assert.deepEqual(parseAgentIds("single"), ["single"]);
});

test("deliveryAgentLabel resolves displayName or handle or falls back to id", () => {
  const agents = [
    { agent_id: "a1", display_name: "Architect Agent", handle: "architect" },
    { agent_id: "a2", handle: "coder" },
  ];
  assert.equal(deliveryAgentLabel("a1", agents), "Architect Agent");
  assert.equal(deliveryAgentLabel("a2", agents), "coder");
  assert.equal(deliveryAgentLabel("a3", agents), "a3");
  assert.equal(deliveryAgentLabel("a1", []), "a1");
});

test("createAgentRow builds interactive button and handles selection", () => {
  withMockDom(() => {
    let selected = "";
    const agent = {
      agent_id: "agent-qa",
      display_name: "QA Bot",
      handle: "qa",
      queued_delivery_count: 3,
      profile_revision: 2,
      current_controller_generation: 1,
      active_endpoint_count: 1,
      unresolved_wake_count: 0,
    };

    const row = createAgentRow(agent, "agent-qa", {
      onSelect: (id) => {
        selected = id;
      },
      language: "en",
    });

    assert.ok(row);
    assert.ok(row.className.includes("selected"));
    assert.equal(row.getAttribute("aria-current"), "true");
    assert.match(row.querySelector(".communication-row-title")?.textContent || "", /QA Bot · @qa/);
    assert.equal(row.querySelector(".chip")?.textContent, "3 queued deliveries");

    row.click();
    assert.equal(selected, "agent-qa");
  });
});

test("createConversationRow renders conversation metadata and triggers callback", () => {
  withMockDom(() => {
    let selected = "";
    const conversation = {
      conversation_id: "conv-101",
      title: "Design Review",
      message_count: 12,
      participant_count: 4,
      last_seq: 15,
    };

    const row = createConversationRow(conversation, "conv-101", {
      onSelect: (id) => {
        selected = id;
      },
      language: "zh-CN",
    });

    assert.ok(row);
    assert.ok(row.className.includes("selected"));
    assert.equal(row.querySelector(".communication-row-title")?.textContent, "Design Review");
    assert.equal(row.querySelector(".chip")?.textContent, "12 条消息");
    assert.match(row.querySelector(".communication-row-meta")?.textContent || "", /conv-101 · 4 位参与者 · 序号 15/);

    row.click();
    assert.equal(selected, "conv-101");
  });
});

test("createConversationMessageCard formats agent and human authored messages", () => {
  withMockDom(() => {
    const agents = [{ agent_id: "bot-1", display_name: "Helper Bot" }];

    const agentMsg = {
      seq: 1,
      message_id: "msg-1",
      created_at_unix_ms: 1700000000000,
      author: { participant_kind: "agent", agent_id: "bot-1" },
      body: "Hello world",
      deliveries: [{ recipient_agent_id: "bot-1", state: "delivered" }],
    };

    const card = createConversationMessageCard(agentMsg, agents, { language: "en" });
    assert.ok(card.className.includes("agent-authored"));
    assert.match(card.querySelector(".conversation-message-author")?.textContent || "", /Agent · Helper Bot/);
    assert.equal(card.querySelector(".conversation-message-body")?.textContent, "Hello world");
    assert.match(card.querySelector(".conversation-message-deliveries")?.textContent || "", /Agent Inbox: Helper Bot delivered/);

    const humanMsg = {
      seq: 2,
      message_id: "msg-2",
      created_at_unix_ms: 1700000000000,
      author: { participant_kind: "human", principal_kind: "admin" },
      body: "Human input",
      deliveries: [],
    };

    const humanCard = createConversationMessageCard(humanMsg, agents, { language: "zh-CN" });
    assert.equal(humanCard.className.includes("agent-authored"), false);
    assert.match(humanCard.querySelector(".conversation-message-author")?.textContent || "", /人工 · admin/);
  });
});

test("createInboxDeliveryCard renders delivery preview and handles consume action", () => {
  withMockDom(() => {
    let consumedId = "";
    const item = {
      delivery_id: "deliv-99",
      conversation_title: "Bug Triage",
      message: {
        seq: 5,
        author: { participant_kind: "agent", agent_id: "reporter" },
        body: "Stack trace attached",
      },
    };

    const card = createInboxDeliveryCard(item, [{ agent_id: "reporter", handle: "reporter-bot" }], {
      onConsume: (id) => {
        consumedId = id;
      },
      language: "en",
    });

    assert.ok(card.className.includes("inbox-delivery"));
    assert.match(card.querySelector(".communication-row-title")?.textContent || "", /Bug Triage · #5/);
    assert.equal(card.querySelector(".inbox-message-preview")?.textContent, "Stack trace attached");

    const consumeBtn = card.querySelector(".text-button");
    assert.ok(consumeBtn);
    consumeBtn.click();
    assert.equal(consumedId, "deliv-99");
  });
});
