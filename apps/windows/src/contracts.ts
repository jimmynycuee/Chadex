// Raw helper snake_case DTOs; frontend never parses helper stdout.
export interface ProjectInspection {
  path: string; allowed_root: string; is_git_repository: boolean; readable: boolean; writable: boolean;
}
export interface RuntimeActivityEntry {
  sequence: number; timestamp_ms: number; source: string; level: string; event_kind: string; message: string;
}
export interface McpPerformanceTrace {
  sequence: number; started_at_ms: number; finished_at_ms?: number | null;
  tool_names: string[]; tool_failed?: boolean | null; status_code?: number | null;
  total_us?: number | null; completion?: string | null;
}
export interface RuntimeReadiness {
  runtime_configured: boolean; runtime_ready: boolean; needs_attention: boolean;
  summary: string; next_action: string | null; summary_kind: string;
  server: string; runner: string; exposure: string; project: string;
}
export interface TaskProgress {
  task_id: string; project: string; goal: string; status: string; current_step: number;
  total_steps: number; completed_steps: number; plan: string[]; cancel_requested: boolean;
  started_at_ms: number; finished_at_ms?: number | null;
  validation: { status: string; checks_passed?: number; checks_failed?: number };
}
export interface MascotJob {
  job_id: string; status: string; started_at_ms: number | null; finished_at_ms: number | null; exit_code: number | null;
}
export interface BackendSnapshot {
  phase: 'unconfigured' | 'preparing' | 'waiting_for_chatgpt_verification' | 'verified' | 'stopped' | 'error';
  graphify?: { available: boolean; path: string | null; source: string } | null;
  selected_project: ProjectInspection | null;
  tunnel_ready: boolean; chat_gpt_connected: boolean; chat_gpt_verified_for_selected_project: boolean;
  last_verified_at_ms: number | null;
  current_operation: { id: string; kind: string; phase: string; started_at_ms: number; cancellable: boolean } | null;
  task_progress: TaskProgress | null;
  mascot_jobs?: MascotJob[] | null;
  error: { code: string; message: string; recovery?: string | null } | null;
  activity_sequence: number; state_revision: number;
  runtime_status?: RuntimeReadiness | null;
  tunnel_status?: { configured: boolean; state: string } | null;
}
export interface Preferences {
  restore_project: boolean; launch_at_login: boolean; notifications: boolean;
  ferret_visible: boolean; ferret_motion: boolean; theme: 'system' | 'dark' | 'light';
  recent_projects: string[]; last_project: string | null; tunnel_id: string;
}
export interface DesktopState {
  helper: { state: 'running' | 'stopping' | 'stopped' | 'failed'; pid: number | null; error: string | null };
  runtime: BackendSnapshot | null; activity: RuntimeActivityEntry[];
  preferences: Preferences; paths: { helper: string; runtime: string; data: string };
  credential_stored: boolean; version: string;
  startup_error?: string | null; credential_error?: string | null;
  runtime_version?: string | null;
  traces?: McpPerformanceTrace[] | null;
}

export type RuntimeMethod = 'inspectProject' | 'activateProject' | 'switchLocalProject'
  | 'configureLocalSetup' | 'resumeService' | 'connectChatGPT' | 'startTunnel' | 'stopTunnel'
  | 'disconnectAI' | 'stopLocalService' | 'updateProxySettings' | 'queryActivities'
  | 'getStatus' | 'refreshRuntime' | 'observeChatGPTActivity';

export const defaultPreferences: Preferences = {
  restore_project: true, launch_at_login: false, notifications: false,
  ferret_visible: true, ferret_motion: true, theme: 'system', recent_projects: [], last_project: null, tunnel_id: '',
};
