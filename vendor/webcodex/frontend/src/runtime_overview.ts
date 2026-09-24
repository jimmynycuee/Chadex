import { translate, localizedCountLabel, type RuntimeLanguage } from "./runtime_i18n.js";
import { runtimeProjectIdentityText, runtimeDeviceIds } from "./runtime_console_state.js";

export function pendingAttentionCount(attention: any): number {
  return ["open_risks", "open_todos", "open_questions", "open_guidance"].reduce(
    (total, key) =>
      total + (typeof attention?.[key] === "number" ? Math.max(0, Math.floor(attention[key])) : 0),
    0
  );
}

export function runnerAttentionCount(runner: any): number {
  return pendingAttentionCount(runner?.sessions?.attention);
}

export function attentionLabel(attention: any, language?: RuntimeLanguage): string {
  const parts: string[] = [];
  for (const [key, singular] of [
    ["open_risks", "risk"],
    ["open_todos", "todo"],
    ["open_questions", "question"],
    ["open_guidance", "guidance"],
  ] as const) {
    const count = typeof attention?.[key] === "number" ? attention[key] : 0;
    if (count) parts.push(localizedCountLabel(count, singular, singular + "s", language));
  }
  return parts.length ? parts.join(" · ") : translate("No retained pending attention", language);
}

export function formatProjectIdentity(project: any, language?: RuntimeLanguage): string {
  if (language !== "zh-CN") return runtimeProjectIdentityText(project);
  if (!project || typeof project.id !== "string" || !project.id) {
    return translate("No project selected", language);
  }
  const runner =
    typeof project.client_id === "string" && project.client_id
      ? project.client_id
      : translate("unknown", language);
  const path = typeof project.path === "string" && project.path ? project.path : "不可用";
  return "运行器：" + runner + " · 项目：" + project.id + " · 工作空间：" + path;
}

export function extractProjectSelectorDevices(
  projects: any[],
  knownDevices: Iterable<string> = [],
  runnerRows: any[] = [],
  selectedDevice = "",
): string[] {
  const devices = new Set(knownDevices);
  for (const device of runtimeDeviceIds(projects)) devices.add(device);
  for (const runner of runnerRows) {
    const clientId = typeof runner?.client_id === "string" ? runner.client_id : "";
    if (clientId) devices.add(clientId);
  }
  if (selectedDevice) devices.add(selectedDevice);
  return Array.from(devices).sort((left, right) => left.localeCompare(right));
}

export function formatProjectLabel(project: any): string {
  const name = project && project.name ? String(project.name) : "";
  const id = project && project.id ? String(project.id) : "";
  const identity = name && name !== id ? name + " — " + id : id;
  const status = project && project.connected ? String(project.agent_status || "online") : "offline";
  return identity + " · " + status;
}

export function mergeEffectiveProjects(projects: any[], homeProjectRows: any[] = []): any[] {
  const aggregates = new Map<string, any>();
  for (const row of homeProjectRows) {
    if (row && typeof row.id === "string") aggregates.set(row.id, row);
  }
  return (Array.isArray(projects) ? projects : []).map((project) => {
    const aggregate = aggregates.get(String(project?.id || ""));
    return aggregate ? { ...project, sessions: aggregate.sessions } : project;
  });
}

export interface RuntimeOverviewMetricsView {
  identity: string;
  build: string;
  runners: string;
  alignment: string;
  projects: string;
  jobs: string;
  attention: string;
  sessions: string;
  recentStatus: string;
}

export function formatRuntimeOverviewMetrics(
  data: any,
  language?: RuntimeLanguage,
): RuntimeOverviewMetricsView | null {
  if (!data) return null;
  const buildGitCommit = data.build_git_commit;
  const buildText = buildGitCommit
    ? (language === "zh-CN" ? "构建 " : "build ") +
      buildGitCommit +
      (data.build_git_dirty ? (language === "zh-CN" ? " · 有未提交更改" : " · dirty") : "")
    : translate("build unavailable", language);

  const projectsText = data.projects_available
    ? localizedCountLabel(data.visible_projects, "visible Project", "visible Projects", language) +
      (data.projects_truncated ? (language === "zh-CN" ? " · 不完整" : " · partial") : "")
    : translate("project:read unavailable", language);

  const jobsText =
    localizedCountLabel(data.active_jobs, "active Job", "active Jobs", language) +
    (data.mixed_builds_present ? (language === "zh-CN" ? " · 存在混合构建" : " · mixed builds") : "");

  const sessionsText =
    localizedCountLabel(data.workflow_sessions?.active, "active Session", "active Sessions", language) +
    " · " +
    localizedCountLabel(data.workflow_sessions?.running, "running Session", "running Sessions", language) +
    (data.workflow_sessions?.truncated
      ? language === "zh-CN"
        ? " · 有界汇总"
        : " · bounded aggregate"
      : "");

  const recentMeta = data.recent_sessions || {};
  const recentStatusText =
    localizedCountLabel(recentMeta.returned, "Session", "Sessions", language) +
    (recentMeta.truncated
      ? (language === "zh-CN" ? " · 前 " : " · top ") + String(recentMeta.returned || 0)
      : "") +
    (recentMeta.scan_truncated
      ? language === "zh-CN"
        ? " · 扫描不完整"
        : " · partial scan"
      : "");

  return {
    identity: [data.service, data.version].filter(Boolean).join(" · "),
    build: buildText,
    runners: localizedCountLabel(data.runner_count, "Runner", "Runners", language),
    alignment:
      localizedCountLabel(data.runners_online, "online", "online", language) +
      " · " +
      localizedCountLabel(data.runners_stale, "stale", "stale", language) +
      " · " +
      localizedCountLabel(data.runners_unavailable, "unavailable", "unavailable", language),
    projects: projectsText,
    jobs: jobsText,
    attention: attentionLabel(data.workflow_sessions, language),
    sessions: sessionsText,
    recentStatus: recentStatusText,
  };
}
