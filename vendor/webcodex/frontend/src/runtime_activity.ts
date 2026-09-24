import { translate, type RuntimeLanguage } from "./runtime_i18n.js";
import { workflowSessionLivenessPresentation } from "./workflow_session_state.js";

export function formatUpdatedTime(timestamp: any, language?: RuntimeLanguage): string {
  if (typeof timestamp !== "number") return translate("time unavailable", language);
  return new Date(timestamp * 1000).toLocaleTimeString(language === "zh-CN" ? "zh-CN" : "en");
}

export function formatSessionDateTime(timestamp: any, language?: RuntimeLanguage): string {
  if (typeof timestamp !== "number") return translate("time unavailable", language);
  return new Date(timestamp * 1000).toLocaleString(language === "zh-CN" ? "zh-CN" : "en");
}

export function formatLivenessPresentation(session: any, language?: RuntimeLanguage): any {
  const presentation = workflowSessionLivenessPresentation(session);
  if (language !== "zh-CN") return presentation;
  let label = translate(String(presentation.label || "idle"), language);
  if (presentation.state === "idle" && String(presentation.label || "").startsWith("idle · ")) {
    label = translate("idle", language) + " · " + String(presentation.label).slice("idle · ".length);
  }
  return { ...presentation, label, tooltip: translate(String(presentation.tooltip || ""), language) };
}

export function activityKindLabel(activity: any, language?: RuntimeLanguage): string {
  const kind = String(activity && activity.kind || "Activity");
  if (activity && activity.job_handoff) {
    if (kind === "Tested") return language === "zh-CN" ? "测试" : "Test";
    if (kind === "Ran") return language === "zh-CN" ? "命令" : "Command";
  }
  if (kind === "Explored" && activity && typeof activity.group_count === "number") {
    return (language === "zh-CN" ? "探索 ×" : "Explored ×") + activity.group_count;
  }
  if (language !== "zh-CN") return kind;
  const labels: Record<string, string> = {
    Activity: "活动",
    Progress: "进度",
    Explored: "探索",
    Edited: "编辑",
    Tested: "测试",
    Ran: "运行",
    Reviewed: "审查",
  };
  return labels[kind] || kind;
}

export function durationLabel(durationMs: number): string {
  if (durationMs < 1000) return durationMs + " ms";
  return (durationMs / 1000).toFixed(durationMs < 10_000 ? 1 : 0) + " s";
}

export function activityFacts(activity: any, includeTiming: boolean, language?: RuntimeLanguage): string[] {
  const facts: string[] = [];
  if (activity && typeof activity.group_count === "number") {
    if (Array.isArray(activity.group_kinds) && activity.group_kinds.length) {
      facts.push(activity.group_kinds.map(String).join(" / "));
    }
    if (Array.isArray(activity.group_tools) && activity.group_tools.length) {
      facts.push(activity.group_tools.map(String).join(", "));
    }
  } else if (activity && activity.tool) {
    facts.push(String(activity.tool));
  }
  if (activity && activity.kind === "Progress") {
    facts.push(language === "zh-CN" ? "仅供参考" : "informational");
  } else if (activity && activity.job_handoff) {
    facts.push(language === "zh-CN" ? "已移交" : "handed off");
    if (activity.execution_state) {
      facts.push((language === "zh-CN" ? "执行 " : "execution ") + translate(String(activity.execution_state), language));
    }
  } else if (activity && activity.state) {
    facts.push(translate(String(activity.state), language));
  }
  if (includeTiming && activity && typeof activity.duration_ms === "number") {
    facts.push(durationLabel(activity.duration_ms));
  }
  if (activity && typeof activity.exit_code === "number") {
    // Label it explicitly as process exit code so that `state = failed` + `exit_code = 0`
    // is clearly understood as a process exit code and never confused with action success.
    facts.push((language === "zh-CN" ? "进程退出 " : "process exit ") + activity.exit_code);
  }
  if (activity && activity.job_id) {
    facts.push("job " + String(activity.job_id));
  }
  if (includeTiming && activity && typeof activity.started_at === "number") {
    facts.push(new Date(activity.started_at * 1000).toLocaleTimeString(language === "zh-CN" ? "zh-CN" : "en"));
  }
  return facts;
}

export function activityDescription(activity: any, language?: RuntimeLanguage): string {
  if (!activity) return "";
  const parts = [activityKindLabel(activity, language), ...activityFacts(activity, false, language)];
  if (activity.summary && !activity.job_handoff) parts.push(String(activity.summary));
  return parts.join(" · ");
}

export function appendActivityPreview(
  parent: HTMLElement,
  label: string,
  activity: any,
  language?: RuntimeLanguage,
): void {
  if (!activity) return;
  const row = document.createElement("div");
  row.className = "activity-preview muted small";
  const prefix = document.createElement("span");
  prefix.className = "activity-preview-label";
  prefix.textContent = label;
  const text = document.createElement("span");
  text.textContent = activityDescription(activity, language);
  row.appendChild(prefix);
  row.appendChild(text);
  parent.appendChild(row);
}

export function createTimelineEvent(activity: any, language?: RuntimeLanguage): HTMLElement {
  const item = document.createElement("li");
  item.className = "timeline-event";
  if (activity && activity.kind === "Progress") item.classList.add("reported-progress");
  if (activity && ["failed", "timed_out"].includes(String(activity.state || ""))) item.classList.add("failed");

  const head = document.createElement("div");
  head.className = "timeline-head";
  const kind = document.createElement("span");
  kind.className = "timeline-kind";
  kind.textContent = activityKindLabel(activity, language);
  const meta = document.createElement("span");
  meta.className = "muted small";
  meta.textContent = activityFacts(activity, true, language).join(" · ");
  head.appendChild(kind);
  head.appendChild(meta);
  item.appendChild(head);

  if (activity && activity.summary) {
    const body = document.createElement("div");
    body.className = "timeline-body small";
    body.textContent = String(activity.summary);
    item.appendChild(body);
  }
  if (activity && Array.isArray(activity.paths) && activity.paths.length) {
    const paths = document.createElement("div");
    paths.className = "muted small";
    paths.textContent = activity.paths.map(String).join(" · ");
    item.appendChild(paths);
  }
  return item;
}

export function renderTimelineEvents(
  container: HTMLElement | null,
  activities: any[],
  language?: RuntimeLanguage,
): void {
  if (!container) return;
  while (container.firstChild) container.removeChild(container.firstChild);
  for (const activity of activities) {
    const item = createTimelineEvent(activity, language);
    container.appendChild(item);
  }
}
