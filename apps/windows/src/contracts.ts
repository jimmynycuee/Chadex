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
  /** Absent in older saved preferences: undefined means ON (matches macOS nil = ON). */
  prepare_service_on_launch?: boolean;
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
  | 'getStatus' | 'refreshRuntime' | 'prewarmRuntime' | 'observeChatGPTActivity'
  | 'discoverExternalSkillSources' | 'getExternalSkillRoots' | 'setExternalSkillRoots'
  | 'getSkillCatalog' | 'getSkillInventory' | 'getSkillDefinition' | 'installSkill' | 'activateSkill' | 'deactivateSkill' | 'removeSkill';

// Skills (docs/BRIDGE_PROTOCOL.md "Skills" / "External Skill sources"); raw helper snake_case.
export type ExternalSkillStatus = 'available' | 'not_found' | 'not_directory' | 'unavailable' | 'scan_limit_exceeded' | 'duplicate_source';
export interface ExternalSkillPackage {
  package: string; state: 'valid' | 'symlink' | 'invalid'; name?: string | null; description?: string | null;
  has_scripts: boolean; link_target_root?: string | null; invalid_reason?: string | null; name_conflict: boolean;
}
export interface ExternalSkillSource {
  kind: 'agents' | 'claude' | 'codex' | string; path: string; canonical_path?: string | null; status: ExternalSkillStatus | string;
  root_is_link: boolean; same_as?: string | null; valid_count: number; symlink_count: number; invalid_count: number;
  script_count: number; truncated: boolean; provided_by: string[]; packages: ExternalSkillPackage[];
}
export interface ExternalSkillSourceDiscovery {
  format: 'chadex.external_skill_sources.v1' | string; sources: ExternalSkillSource[]; recommended_roots: string[];
}
export interface ExternalSkillRootsState {
  format: 'chadex.external_skill_roots.v1' | string; roots: string[]; script_roots: string[]; revision: string;
  generation?: number | null; runner_resynced?: boolean | null; resync_error?: string | null;
}
export interface SetExternalSkillRootsParams {
  roots: string[]; script_roots: string[]; expected_revision: string; verify_project_path?: string;
}
export interface SkillDescriptor {
  skill_id: string; name: string; description: string; definition_revision: string; package_revision?: string | null;
  source_scope: string; trust: string; name_conflict: boolean;
  /** Absent from older helpers: undefined means the helper did not report a policy. */
  scripts_allowed?: boolean | null;
}
export interface SkillCatalog {
  project: string; catalog_revision: string; total_count: number; returned_count: number;
  skills: SkillDescriptor[]; invalid_count: number; diagnostics: unknown[]; discovery_truncated: boolean;
}
export interface ManagedSkillEntry {
  skill_id: string; skill_key: string; state_revision: string; active_package_revision?: string | null;
  preferred_package_revision: string; definition_revision: string; name: string; description: string; total_versions: number;
}
export interface SkillDefinitionPreview { skill_id: string; definition_revision: string; package_revision?: string | null; text: string; has_more: boolean }
export interface SkillInventory { project: string; total_count: number; skills: ManagedSkillEntry[] }

export const defaultPreferences: Preferences = {
  restore_project: true, launch_at_login: false, notifications: false,
  ferret_visible: true, ferret_motion: true, theme: 'system', recent_projects: [], last_project: null, tunnel_id: '', prepare_service_on_launch: true,
};
