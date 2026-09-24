import { translate, localizedCountLabel, type RuntimeLanguage } from "./runtime_i18n.js";

export function communicationTimeLabel(value: any, language?: RuntimeLanguage): string {
  if (typeof value !== "number" || !Number.isFinite(value)) return translate("time unavailable", language);
  return new Date(value).toLocaleString(language === "zh-CN" ? "zh-CN" : "en");
}

export function parseAgentIds(value: string): string[] {
  const ids = value
    .split(/[\s,]+/)
    .map((item) => item.trim())
    .filter(Boolean);
  return Array.from(new Set(ids));
}

export function deliveryAgentLabel(agentId: string, agents: any[] = []): string {
  const agent = agents.find((a) => String(a?.agent_id || "") === agentId);
  return agent ? String(agent.display_name || agent.handle || agentId) : agentId;
}

function appendCommunicationChip(parent: HTMLElement, text: string, extraClass = ""): HTMLElement {
  const chip = document.createElement("span");
  chip.className = "chip" + (extraClass ? " " + extraClass : "");
  chip.textContent = text;
  parent.appendChild(chip);
  return chip;
}

export function createAgentRow(
  agent: any,
  selectedAgentId: string,
  options: {
    onSelect: (agentId: string) => void;
    language?: RuntimeLanguage;
  },
): HTMLElement | null {
  const agentId = String(agent?.agent_id || "");
  if (!agentId) return null;
  const language = options.language;
  const row = document.createElement("button");
  row.type = "button";
  row.className = "communication-row" + (agentId === selectedAgentId ? " selected" : "");
  if (agentId === selectedAgentId) row.setAttribute("aria-current", "true");

  const head = document.createElement("div");
  head.className = "communication-row-head";
  const title = document.createElement("span");
  title.className = "communication-row-title";
  title.textContent = String(agent?.display_name || agent?.handle || "Agent") + " · @" + String(agent?.handle || "agent");

  const unread = document.createElement("span");
  unread.className = "chip" + (Number(agent?.queued_delivery_count || 0) > 0 ? " tone-warn" : "");
  unread.textContent = localizedCountLabel(agent?.queued_delivery_count, "queued delivery", "queued deliveries", language);
  head.appendChild(title);
  head.appendChild(unread);
  row.appendChild(head);

  const meta = document.createElement("span");
  meta.className = "communication-row-meta";
  meta.textContent = agentId
    + (language === "zh-CN" ? " · 配置版本 r" : " · profile r") + String(agent?.profile_revision || 0)
    + (language === "zh-CN" ? " · 控制器 g" : " · controller g") + String(agent?.current_controller_generation || 0)
    + " · " + localizedCountLabel(agent?.active_endpoint_count, "active Endpoint", "active Endpoints", language)
    + " · " + localizedCountLabel(agent?.unresolved_wake_count, "unresolved Wake", "unresolved Wakes", language);
  row.appendChild(meta);

  row.addEventListener("click", () => options.onSelect(agentId));
  return row;
}

export function renderAgentRows(
  list: HTMLElement | null,
  agents: any[],
  selectedAgentId: string,
  options: {
    onSelect: (agentId: string) => void;
    language?: RuntimeLanguage;
  },
): void {
  if (!list) return;
  while (list.firstChild) list.removeChild(list.firstChild);
  for (const agent of agents) {
    const row = createAgentRow(agent, selectedAgentId, options);
    if (row) list.appendChild(row);
  }
}

export function createConversationRow(
  conversation: any,
  selectedConversationId: string,
  options: {
    onSelect: (conversationId: string) => void;
    language?: RuntimeLanguage;
  },
): HTMLElement | null {
  const conversationId = String(conversation?.conversation_id || "");
  if (!conversationId) return null;
  const language = options.language;
  const row = document.createElement("button");
  row.type = "button";
  row.className = "communication-row" + (conversationId === selectedConversationId ? " selected" : "");
  if (conversationId === selectedConversationId) row.setAttribute("aria-current", "true");

  const head = document.createElement("div");
  head.className = "communication-row-head";
  const title = document.createElement("span");
  title.className = "communication-row-title";
  title.textContent = String(conversation?.title || translate("Untitled Conversation", language));

  const count = document.createElement("span");
  count.className = "chip";
  count.textContent = localizedCountLabel(conversation?.message_count, "message", "messages", language);
  head.appendChild(title);
  head.appendChild(count);
  row.appendChild(head);

  const meta = document.createElement("span");
  meta.className = "communication-row-meta";
  meta.textContent = conversationId
    + " · " + localizedCountLabel(conversation?.participant_count, "participant", "participants", language)
    + (language === "zh-CN" ? " · 序号 " : " · seq ") + String(conversation?.last_seq || 0);
  row.appendChild(meta);

  row.addEventListener("click", () => options.onSelect(conversationId));
  return row;
}

export function renderConversationRows(
  list: HTMLElement | null,
  conversations: any[],
  selectedConversationId: string,
  options: {
    onSelect: (conversationId: string) => void;
    language?: RuntimeLanguage;
  },
): void {
  if (!list) return;
  while (list.firstChild) list.removeChild(list.firstChild);
  for (const conversation of conversations) {
    const row = createConversationRow(conversation, selectedConversationId, options);
    if (row) list.appendChild(row);
  }
}

export function createConversationMessageCard(
  message: any,
  agents: any[],
  options: {
    language?: RuntimeLanguage;
  } = {},
): HTMLElement {
  const language = options.language;
  const author = message?.author || {};
  const agentAuthored = String(author.participant_kind || "") === "agent";
  const card = document.createElement("article");
  card.className = "conversation-message" + (agentAuthored ? " agent-authored" : "");

  const head = document.createElement("div");
  head.className = "conversation-message-head";
  const name = document.createElement("span");
  name.className = "conversation-message-author";
  name.textContent = agentAuthored
    ? "Agent · " + String(author.display_name || author.handle || (author.agent_id ? deliveryAgentLabel(String(author.agent_id), agents) : "") || author.agent_id || translate("unknown", language))
    : (language === "zh-CN" ? "人工 · " : "Human · ") + String(author.principal_kind || (language === "zh-CN" ? "凭证主体" : "credential principal"));

  const seq = document.createElement("span");
  seq.className = "muted small";
  seq.textContent = "#" + String(message?.seq || 0) + " · " + communicationTimeLabel(message?.created_at_unix_ms, language);
  head.appendChild(name);
  head.appendChild(seq);
  card.appendChild(head);

  const meta = document.createElement("div");
  meta.className = "conversation-message-meta";
  const metaParts = [String(message?.message_id || "")];
  if (author.agent_id) metaParts.push(String(author.agent_id));
  if (message?.reply_to) metaParts.push((language === "zh-CN" ? "回复 " : "reply to ") + String(message.reply_to));
  meta.textContent = metaParts.join(" · ");
  card.appendChild(meta);

  const body = document.createElement("div");
  body.className = "conversation-message-body";
  body.textContent = String(message?.body || "");
  card.appendChild(body);

  const deliveries = Array.isArray(message?.deliveries) ? message.deliveries : [];
  const delivery = document.createElement("div");
  delivery.className = "conversation-message-deliveries";
  delivery.textContent = deliveries.length
    ? (language === "zh-CN" ? "Agent 收件箱：" : "Agent Inbox: ") + deliveries.map((item: any) => deliveryAgentLabel(String(item?.recipient_agent_id || ""), agents) + " " + translate(String(item?.state || "unknown"), language)).join(" · ")
    : (language === "zh-CN" ? "没有 Agent 收件箱投递 · 仅保留记录 / 人工房间" : "No Agent Inbox delivery · transcript / Human room only");
  card.appendChild(delivery);

  return card;
}

export function renderConversationMessages(
  transcript: HTMLElement | null,
  messages: any[],
  agents: any[],
  options: {
    language?: RuntimeLanguage;
  } = {},
): void {
  if (!transcript) return;
  while (transcript.firstChild) transcript.removeChild(transcript.firstChild);
  for (const message of messages) {
    const card = createConversationMessageCard(message, agents, options);
    transcript.appendChild(card);
  }
  transcript.scrollTop = transcript.scrollHeight;
}

export function createInboxDeliveryCard(
  item: any,
  agents: any[],
  options: {
    onConsume: (deliveryId: string) => void;
    language?: RuntimeLanguage;
  },
): HTMLElement {
  const language = options.language;
  const row = document.createElement("article");
  row.className = "communication-row inbox-delivery";

  const head = document.createElement("div");
  head.className = "communication-row-head";
  const title = document.createElement("span");
  title.className = "communication-row-title";
  title.textContent = String(item?.conversation_title || translate("Untitled Conversation", language)) + " · #" + String(item?.message?.seq || 0);

  const consume = document.createElement("button");
  consume.type = "button";
  consume.className = "text-button";
  consume.textContent = translate("Consume", language);
  consume.addEventListener("click", () => options.onConsume(String(item?.delivery_id || "")));
  head.appendChild(title);
  head.appendChild(consume);
  row.appendChild(head);

  const meta = document.createElement("span");
  meta.className = "communication-row-meta";
  meta.textContent = String(item?.delivery_id || "")
    + (language === "zh-CN" ? " · 来自 " : " · from ")
    + (item?.message?.author?.participant_kind === "agent"
      ? deliveryAgentLabel(String(item.message.author.agent_id || ""), agents)
      : (language === "zh-CN" ? "人工" : "Human"));
  row.appendChild(meta);

  const body = document.createElement("div");
  body.className = "inbox-message-preview";
  body.textContent = String(item?.message?.body || "");
  row.appendChild(body);

  return row;
}

export function renderInboxDeliveryCards(
  container: HTMLElement | null,
  inbox: any[],
  agents: any[],
  options: {
    onConsume: (deliveryId: string) => void;
    language?: RuntimeLanguage;
  },
): void {
  if (!container) return;
  while (container.firstChild) container.removeChild(container.firstChild);
  for (const item of inbox) {
    const row = createInboxDeliveryCard(item, agents, options);
    container.appendChild(row);
  }
}
