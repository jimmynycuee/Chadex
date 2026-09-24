import {
  translate,
  type RuntimeLanguage,
} from "./runtime_i18n.js";
import { formatUpdatedTime } from "./runtime_activity.js";
import {
  runtimeCollaborationMessageSides,
  runtimeCollaborationMessageCanMutate,
} from "./runtime_collaboration_state.js";
import { appendRichMessage } from "./runtime_rich_text.js";
import { runtimeIcon, createMessageAction } from "./runtime_icons.js";

function clearCollaborationNode(node: HTMLElement): void {
  while (node.firstChild) node.removeChild(node.firstChild);
}

export function collaborationPhaseLabel(phase: string, language?: RuntimeLanguage): string {
  switch (phase) {
    case "live": return translate("Live", language);
    case "reconnecting": return translate("Reconnecting", language);
    case "paused": return translate("Paused", language);
    default: return translate("Idle", language);
  }
}

export function syncCollaborationComposerLayout(
  body: HTMLTextAreaElement | null = typeof document !== "undefined" ? document.getElementById("runtime-message-body") as HTMLTextAreaElement | null : null,
  composer: HTMLElement | null = typeof document !== "undefined" ? document.getElementById("runtime-collaboration-form") : null,
  send: HTMLButtonElement | null = typeof document !== "undefined" ? document.getElementById("runtime-message-send") as HTMLButtonElement | null : null,
): void {
  const hasContent = !!body?.value.trim();
  composer?.classList.toggle("has-content", hasContent);
  send?.classList.toggle("is-ready", hasContent);
  if (!body) return;
  body.style.height = "0px";
  const nextHeight = Math.min(Math.max(body.scrollHeight, 44), 180);
  body.style.height = nextHeight + "px";
  body.style.overflowY = body.scrollHeight > 180 ? "auto" : "hidden";
}

export function formatComposerOptionSummary(
  kind: string,
  priority: string,
  requiresAck: boolean,
  language?: RuntimeLanguage,
): { label: string; hasSelection: boolean } {
  const signals: string[] = [];
  if (kind && kind !== "note") signals.push(translate(kind, language));
  if (priority && priority !== "normal") signals.push(translate(priority, language));
  if (requiresAck) signals.push(language === "zh-CN" ? "需确认" : "ACK");
  return {
    label: signals.length ? signals.join(" · ") : translate("Options", language),
    hasSelection: signals.length > 0,
  };
}

export function runtimeSearchMatches(query: string, values: unknown[]): boolean {
  const text = values.filter((value) => typeof value === "string").join(" ").toLocaleLowerCase();
  return query.trim().toLocaleLowerCase().split(/\s+/).every((term) => text.includes(term));
}

export function filterCollaborationCards(
  cards: HTMLElement[],
  separators: HTMLElement[],
  messages: any[],
  query: string,
): { matches: number; total: number } {
  let matches = 0;
  for (const card of cards) {
    const message = messages.find((entry: any) => entry && entry.message_id === card.dataset.messageId);
    const visible = runtimeSearchMatches(query, [message?.message, message?.resolution, message?.message_id, message?.author_session_id]);
    card.hidden = !visible;
    if (visible) matches++;
  }
  for (const node of separators) {
    node.hidden = !!query.trim();
  }
  return { matches, total: cards.length };
}

export function renderLatestAgentMessage(
  container: HTMLElement,
  messages: any[],
  locallyAuthoredMessageIds: Set<string>,
  language?: RuntimeLanguage,
): void {
  const updatedLabel = (timestamp: any): string => formatUpdatedTime(timestamp, language);
  const sides = runtimeCollaborationMessageSides(messages, locallyAuthoredMessageIds);
  const latest = [...messages].reverse().find((message: any) => sides.get(String(message.message_id)) === "incoming" && !message.superseded_by_message_id && message.closure_kind !== "withdrawn");
  clearCollaborationNode(container);
  if (latest) {
    appendRichMessage(container, latest.message);
    const time = document.createElement("p"); time.className = "muted small"; time.textContent = updatedLabel(latest.created_at);
    container.appendChild(time);
  } else {
    container.textContent = language === "zh-CN"
      ? "当前保留范围内暂无 Agent 留言。ACK 不包含回复正文；下方可查看模型报告的进度。"
      : "No Agent message in the retained window. ACK contains no reply text; model-reported progress appears below.";
  }
}

export interface RenderCollaborationCardsOptions {
  locallyAuthoredIds: Set<string>;
  previouslyRenderedMessageIds: Set<string>;
  canMutate: boolean;
  language?: RuntimeLanguage;
  onReply: (messageId: string) => void;
  onEdit: (message: any) => void;
  onWithdraw: (messageId: string) => void;
}

export function renderCollaborationMessageCards(
  node: HTMLElement,
  messages: any[],
  options: RenderCollaborationCardsOptions,
): void {
  const tr = (text: string): string => translate(text, options.language);
  const updatedLabel = (timestamp: any): string => formatUpdatedTime(timestamp, options.language);

  const byId = new Map<string, any>();
  const children = new Map<string, any[]>();
  for (const message of messages) {
    const id = String(message?.message_id || ""); if (id) byId.set(id, message);
  }
  for (const message of messages) {
    const parent = typeof message?.reply_to === "string" ? message.reply_to : "";
    if (parent && byId.has(parent)) {
      const list = children.get(parent) || []; list.push(message); children.set(parent, list);
    }
  }
  const messageSides = runtimeCollaborationMessageSides(messages, options.locallyAuthoredIds);
  const visited = new Set<string>();
  let previousRenderedSide = "";
  let previousRenderedDay = "";
  const appendMessage = (message: any, depth: number, parentUnavailable: boolean): void => {
    const id = String(message?.message_id || ""); if (!id || visited.has(id)) return; visited.add(id);
    const card = document.createElement("article");
    card.dataset.messageId = id;
    card.className = "message-card " + String(message?.kind || "note") + (String(message?.status || "") === "resolved" ? " resolved" : "") + (parentUnavailable ? " retained-reply" : "");
    const messageSide = messageSides.get(id) || "neutral";
    card.classList.add(messageSide === "incoming" ? "agent-authored" : messageSide === "outgoing" ? "human-authored" : "provenance-unknown");
    const createdAt = typeof message?.created_at === "number" ? message.created_at : 0;
    const createdDate = createdAt ? new Date(createdAt * 1000) : null;
    const dayKey = createdDate ? [createdDate.getFullYear(), createdDate.getMonth(), createdDate.getDate()].join("-") : "";
    if (dayKey && dayKey !== previousRenderedDay) {
      const separator = document.createElement("div");
      separator.className = "message-date-separator";
      const label = document.createElement("span");
      label.textContent = createdDate?.toLocaleDateString(options.language === "zh-CN" ? "zh-CN" : "en", { month: "short", day: "numeric", year: "numeric" }) || "";
      separator.appendChild(label);
      node.appendChild(separator);
      previousRenderedDay = dayKey;
      previousRenderedSide = "";
    }
    card.classList.add(messageSide === "incoming" ? "message-incoming" : messageSide === "outgoing" ? "message-outgoing" : "message-neutral");
    if (!options.previouslyRenderedMessageIds.has(id)) card.classList.add("message-entering");
    if (previousRenderedSide === messageSide) card.classList.add("message-group-continuation");
    previousRenderedSide = messageSide;
    if (depth > 0) card.classList.add("message-thread");
    const content = document.createElement("div"); content.className = "message-content";
    const author = document.createElement("div"); author.className = "message-author";
    const authorName = document.createElement("span"); authorName.className = "message-author-name";
    authorName.textContent = messageSide === "incoming" ? tr("Agent") : messageSide === "outgoing" ? tr("You") : tr("Retained message");
    if (message?.author_session_id) authorName.title = String(message.author_session_id);
    else if (messageSide === "neutral") authorName.title = tr("Author provenance unavailable");
    author.appendChild(authorName); content.appendChild(author);
    if (message?.reply_to) {
      const replyContext = document.createElement("div"); replyContext.className = "message-reply-context";
      replyContext.appendChild(runtimeIcon("reply"));
      const replyText = document.createElement("span");
      const parent = byId.get(String(message.reply_to));
      const preview = parent?.message ? String(parent.message).replace(/\s+/g, " ").trim().slice(0, 120) : tr("Original message unavailable");
      replyText.textContent = tr("Replying to") + " · " + preview;
      replyContext.appendChild(replyText); content.appendChild(replyContext);
    }
    const footer = document.createElement("div"); footer.className = "message-footer";
    const head = document.createElement("div"); head.className = "message-head";
    const kindValue = String(message?.kind || "note");
    const priorityValue = String(message?.priority || "normal");
    const statusValue = String(message?.status || "open");
    const messageSignals: string[] = [];
    if (kindValue !== "note") messageSignals.push(tr(kindValue));
    if (priorityValue !== "normal") messageSignals.push(tr(priorityValue));
    if (statusValue && statusValue !== "open" && statusValue !== "resolved") messageSignals.push(tr(statusValue));
    if (messageSignals.length) {
      const kind = document.createElement("span"); kind.className = "message-kind"; kind.textContent = messageSignals.join(" · "); head.appendChild(kind);
    }
    const time = document.createElement("span"); time.className = "muted small"; time.textContent = updatedLabel(message?.created_at);
    head.appendChild(time); footer.appendChild(head);
    const meta = document.createElement("div"); meta.className = "message-meta";
    const metaParts = [id]; if (message?.author_session_id) metaParts.push((options.language === "zh-CN" ? "作者 " : "author ") + String(message.author_session_id));
    if (parentUnavailable) metaParts.push(options.language === "zh-CN" ? "保留的回复 · 上级消息不可用" : "retained reply · parent unavailable");
    else if (message?.reply_to) metaParts.push((options.language === "zh-CN" ? "回复 " : "reply to ") + String(message.reply_to));
    if (message?.superseded_by_message_id) {
      const replacementId = String(message.superseded_by_message_id);
      metaParts.push(byId.has(replacementId)
        ? "superseded by " + replacementId
        : "superseded by " + replacementId + " · replacement unavailable / retained link only");
    }
    if (message?.supersedes_message_id) {
      const originalId = String(message.supersedes_message_id);
      metaParts.push(byId.has(originalId)
        ? "replaces " + originalId
        : "replaces " + originalId + " · retained link only");
    }
    meta.textContent = metaParts.join(" · "); footer.appendChild(meta); footer.title = meta.textContent;
    const bubble = document.createElement("div"); bubble.className = "message-bubble";
    appendRichMessage(bubble, message?.message);
    content.appendChild(bubble);
    if (message?.requires_ack) {
      const ack = document.createElement("div"); ack.className = "message-ack";
      const acknowledged = typeof message?.first_ack_observed_at === "number";
      ack.classList.toggle("observed", acknowledged);
      ack.textContent = acknowledged
        ? (options.language === "zh-CN" ? "已观察到 ACK（不代表回复或完成）" : "ACK observed (not a reply or completion)") + " · " + updatedLabel(message.first_ack_observed_at)
        : tr("Acknowledgement required");
      ack.title = acknowledged
        ? "ACK required · First ACK observed " + updatedLabel(message.first_ack_observed_at)
        : "ACK required";
      footer.appendChild(ack);
    }
    if (message?.resolved_at || message?.resolution || message?.resolved_by_message_id || message?.closure_kind) {
      const resolution = document.createElement("div"); resolution.className = "message-resolution";
      const parts: string[] = [];
      if (message?.closure_kind === "withdrawn") parts.push("withdrawn" + (message.resolved_at ? " " + updatedLabel(message.resolved_at) : ""));
      else if (message?.closure_kind === "superseded") parts.push("superseded" + (message.resolved_at ? " " + updatedLabel(message.resolved_at) : ""));
      else if (message.resolved_at) parts.push("resolved " + updatedLabel(message.resolved_at));
      if (message.resolution) parts.push(String(message.resolution));
      if (message.resolved_by_message_id) parts.push("by " + String(message.resolved_by_message_id));
      const resolutionLabel = message?.closure_kind === "withdrawn"
        ? tr("Withdrawn")
        : message?.closure_kind === "superseded"
          ? tr("Replaced")
          : tr("Resolved");
      resolution.textContent = resolutionLabel + (message.resolved_at ? " · " + updatedLabel(message.resolved_at) : "");
      resolution.title = parts.join(" · "); footer.appendChild(resolution);
      if (message.resolution) {
        const explanation = document.createElement("section"); explanation.className = "message-resolution-body";
        const label = document.createElement("strong"); label.textContent = options.language === "zh-CN" ? "处理说明" : "Resolution";
        explanation.appendChild(label);
        appendRichMessage(explanation, message.resolution);
        content.appendChild(explanation);
      }
    }
    const actions = document.createElement("div"); actions.className = "message-actions";
    actions.appendChild(createMessageAction(tr("Reply"), "reply", () => options.onReply(id)));
    if (runtimeCollaborationMessageCanMutate(message) && options.canMutate) {
      const editLabel = options.language === "zh-CN" ? "替换这条保留消息，同时保留其历史记录。" : "Replace this retained message while preserving its history.";
      const deleteLabel = options.language === "zh-CN" ? "撤回这条保留消息；历史记录仍会保留。" : "Withdraw this retained message; history is preserved.";
      actions.appendChild(createMessageAction(editLabel, "edit", () => options.onEdit(message)));
      actions.appendChild(createMessageAction(deleteLabel, "trash", () => options.onWithdraw(id), true));
    }
    footer.appendChild(actions);
    content.appendChild(footer);
    card.appendChild(content);
    node.appendChild(card);
    for (const child of children.get(id) || []) appendMessage(child, depth + 1, false);
  };
  for (const message of messages) {
    const parent = typeof message?.reply_to === "string" ? message.reply_to : "";
    if (!parent || !byId.has(parent)) appendMessage(message, 0, !!parent);
  }
  for (const message of messages) appendMessage(message, 0, false);
}
