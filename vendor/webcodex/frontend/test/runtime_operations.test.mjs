import test from "node:test";
import assert from "node:assert/strict";
import {
  operationKey,
  idempotencyKeyFor,
  formatCommunicationAvailability,
  formatAgentCardRevision,
  formatAgentWakeStatus,
  formatAgentEndpointStatus,
  formatConversationSeq,
  validateAgentCreateInputs,
  validateAgentUpdateInputs,
  validateConversationCreateInputs,
} from "../dist/runtime_operations.js";

test("operationKey generates unique prefixed keys", () => {
  const k1 = operationKey("test");
  const k2 = operationKey("test");
  assert.match(k1, /^test-/);
  assert.match(k2, /^test-/);
  assert.notEqual(k1, k2);
});

test("idempotencyKeyFor preserves existing key when fingerprint matches", () => {
  const initial = idempotencyKeyFor(null, "fp-1", "op");
  assert.equal(initial.fingerprint, "fp-1");
  assert.match(initial.key, /^op-/);

  const replayed = idempotencyKeyFor(initial, "fp-1", "op");
  assert.equal(replayed.key, initial.key);

  const changed = idempotencyKeyFor(initial, "fp-2", "op");
  assert.notEqual(changed.key, initial.key);
  assert.equal(changed.fingerprint, "fp-2");
});

test("formatCommunicationAvailability produces localized availability status", () => {
  assert.equal(formatCommunicationAvailability(null, null, "en"), "communication:read checking…");
  assert.equal(formatCommunicationAvailability(null, null, "zh-CN"), "正在检查 communication:read…");

  assert.equal(formatCommunicationAvailability(false, false, "en"), "communication:read unavailable");
  assert.equal(formatCommunicationAvailability(false, false, "zh-CN"), "communication:read 不可用");

  assert.equal(formatCommunicationAvailability(true, false, "en"), "communication:read · read only");
  assert.equal(formatCommunicationAvailability(true, false, "zh-CN"), "communication:read · 只读");

  assert.match(formatCommunicationAvailability(true, true, "en"), /30s refresh while visible/);
  assert.match(formatCommunicationAvailability(true, true, "zh-CN"), /当前视图每 30 秒刷新/);
});

test("formatAgentCardRevision produces formatted revision and controller info", () => {
  const agent = {
    profile_revision: 2,
    current_controller_generation: 5,
    updated_at_unix_ms: 1700000000000,
  };
  const en = formatAgentCardRevision(agent, "en");
  assert.match(en, /Profile revision 2 · controller generation 5/);

  const zh = formatAgentCardRevision(agent, "zh-CN");
  assert.match(zh, /配置版本 2 · 控制器代数 5/);
});

test("formatAgentWakeStatus formats wake counts and states", () => {
  const agent = {
    unresolved_wake_count: 2,
    latest_wake_state: "active",
  };
  const en = formatAgentWakeStatus(agent, "en");
  assert.match(en, /2 unresolved Wakes · latest active/);

  const zh = formatAgentWakeStatus(agent, "zh-CN");
  assert.match(zh, /2 个未解决唤醒/);
});

test("formatAgentEndpointStatus formats active endpoint or fallback message", () => {
  assert.match(formatAgentEndpointStatus(null, "en"), /This window is not acting as the Agent/);
  assert.match(formatAgentEndpointStatus(null, "zh-CN"), /此窗口尚未作为该 Agent/);

  const endpoint = {
    endpoint_id: "ep-123",
    lifecycle: "attached",
    controller_generation: 1,
    lease_expires_at_unix_ms: 1700000000000,
    wake_capable: true,
  };
  const en = formatAgentEndpointStatus(endpoint, "en");
  assert.match(en, /Browser Endpoint ep-123 · attached · generation 1/);
  assert.match(en, /runtime wake capable: true/);
});

test("formatConversationSeq formats sequence number and bounded page indicators", () => {
  const summary = { last_seq: 42, message_count: 10 };
  assert.equal(formatConversationSeq(summary, { after_seq: 0, truncated: false }, "en"), "seq 42 · 10 messages");
  assert.equal(formatConversationSeq(summary, { after_seq: 10, truncated: true }, "en"), "seq 42 · 10 messages · recent bounded page");
  assert.equal(formatConversationSeq(summary, { after_seq: 10, truncated: true }, "zh-CN"), "序号 42 · 10 条消息 · 最近有界页面");
});

test("validateAgentCreateInputs validates required fields and generates fingerprint", () => {
  const missing = validateAgentCreateInputs("", "Name", "Desc", "");
  assert.equal(missing.valid, false);
  assert.match(missing.error, /Handle and display name are required/);

  const valid = validateAgentCreateInputs("agent-1", "Agent One", "A helpful agent", "tag1, tag2");
  assert.equal(valid.valid, true);
  assert.equal(valid.data.handle, "agent-1");
  assert.equal(valid.data.displayName, "Agent One");
  assert.deepEqual(valid.data.labels, ["tag1", "tag2"]);
  assert.ok(valid.data.fingerprint.includes("agent-1"));
});

test("validateAgentUpdateInputs validates required fields", () => {
  const missing = validateAgentUpdateInputs("agent-1", "", "Desc", "");
  assert.equal(missing.valid, false);

  const valid = validateAgentUpdateInputs("agent-1", "New Name", "Desc", "alpha, beta");
  assert.equal(valid.valid, true);
  assert.equal(valid.data.displayName, "New Name");
  assert.deepEqual(valid.data.specialtyLabels, ["alpha", "beta"]);
});

test("validateConversationCreateInputs validates at least one agent id", () => {
  const empty = validateConversationCreateInputs("Conv", "", "");
  assert.equal(empty.valid, false);
  assert.match(empty.error, /At least one Agent id is required/);

  const withDefault = validateConversationCreateInputs("Conv", "", "agent-default");
  assert.equal(withDefault.valid, true);
  assert.deepEqual(withDefault.data.agentIds, ["agent-default"]);

  const explicit = validateConversationCreateInputs("Conv", "a1, a2");
  assert.equal(explicit.valid, true);
  assert.deepEqual(explicit.data.agentIds, ["a1", "a2"]);
});
