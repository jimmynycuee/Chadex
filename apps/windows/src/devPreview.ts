// Design preview data for `npm run dev` in a plain browser (`?preview=waiting`,
// `verified`, `unconfigured`, `error`). Loaded only when import.meta.env.DEV, so
// it never ships in the desktop build.
import type { DesktopApi } from './api';
import { defaultPreferences, type BackendSnapshot, type DesktopState, type RuntimeActivityEntry } from './contracts';

const project = { path: 'C:\\Users\\jimmy\\Documents\\Chadex', allowed_root: 'C:\\Users\\jimmy\\Documents\\Chadex', is_git_repository: true, readable: true, writable: true };

function runtime(scenario: string): BackendSnapshot | null {
  const base: BackendSnapshot = {
    phase: 'waiting_for_chatgpt_verification', selected_project: project, tunnel_ready: true, chat_gpt_connected: false,
    chat_gpt_verified_for_selected_project: false, last_verified_at_ms: null, current_operation: null, task_progress: null,
    mascot_jobs: [], error: null, activity_sequence: 9, state_revision: 1,
    runtime_status: { runtime_configured: true, runtime_ready: true, needs_attention: false, summary: '本地服務已啟動', next_action: null,
      summary_kind: 'ready', server: 'running', runner: 'running', exposure: 'local', project: 'selected' },
    tunnel_status: { configured: true, state: 'ready' },
  };
  if (scenario === 'verified') return { ...base, phase: 'verified', chat_gpt_connected: true, chat_gpt_verified_for_selected_project: true, last_verified_at_ms: Date.now() - 60_000 };
  if (scenario === 'unconfigured') return { ...base, phase: 'unconfigured', selected_project: null, tunnel_ready: false, tunnel_status: { configured: false, state: 'idle' } };
  if (scenario === 'error') return { ...base, phase: 'error', tunnel_ready: false,
    error: { code: 'tunnel_credentials_rejected', message: 'OpenAI 拒絕了 Tunnel 憑證', recovery: '到「連線」重新輸入 Tunnel ID 與 API key 後再試一次。' } };
  return base;
}

function activity(scenario: string): RuntimeActivityEntry[] {
  if (scenario === 'unconfigured') return [];
  const now = Date.now();
  const entry = (sequence: number, secondsAgo: number, source: string, event_kind: string, message: string, level = 'info') =>
    ({ sequence, timestamp_ms: now - secondsAgo * 1000, source, level, event_kind, message });
  return [
    entry(1, 86_400 + 4_000, 'chadex', 'operation_started', '正在連線到 ChatGPT'),
    entry(2, 86_400 + 3_660, 'tunnel', 'tunnel_retry', '正在重試 Tunnel 連線（第 2 次）', 'warning'),
    entry(3, 86_400 + 3_600, 'tunnel', 'tunnel_disconnected', '遠端主機關閉了 Tunnel 連線', 'error'),
    entry(4, 80, 'chadex', 'operation_started', '正在恢復本機執行環境'),
    entry(5, 74, 'service', 'process_started', 'Chadex 已啟動本機程序（PID 16174）'),
    entry(6, 72, 'runner', 'process_started', 'Chadex 已啟動本機程序（PID 16179）'),
    entry(7, 70, 'chadex', 'local_runtime_ready', '本機服務已就緒'),
    entry(8, 42, 'chadex', 'operation_started', '正在為 ChatGPT 開啟專案'),
    entry(9, 40, 'chadex', 'project_activated', '已切換到「Chadex」'),
  ];
}

export function previewApi(scenario: string): DesktopApi {
  const state: DesktopState = {
    helper: { state: 'running', pid: 4242, error: null }, runtime: runtime(scenario), activity: activity(scenario),
    preferences: { ...defaultPreferences, recent_projects: scenario === 'unconfigured' ? [] : [project.path], last_project: project.path, tunnel_id: 'tunnel_chadex_dev' },
    paths: { helper: 'C:\\Program Files\\Chadex\\helper\\chadex-helper.exe', runtime: 'C:\\Program Files\\Chadex\\chadex-runtime', data: 'C:\\Users\\jimmy\\AppData\\Local\\Chadex' },
    credential_stored: scenario !== 'unconfigured', version: '0.5.0', runtime_version: '0.5.0', traces: [],
  };
  const resolved = <T>(value: T) => () => Promise.resolve(value);
  return {
    desktopState: () => Promise.resolve({ ...state, runtime: state.runtime && { ...state.runtime } }),
    chooseProject: resolved(null), openProject: resolved(undefined), runtimeAction: resolved({}), prewarmRuntime: resolved({}),
    inspectProject: () => Promise.resolve(project), savePreferences: (prefs) => Promise.resolve(prefs),
    discoverExternalSkillSources: resolved({ format: 'chadex.external_skill_sources.v1', sources: [], recommended_roots: [] }),
    getExternalSkillRoots: resolved({ format: 'chadex.external_skill_roots.v1', roots: [], script_roots: [], revision: 'r1' }),
    setExternalSkillRoots: (params) => Promise.resolve({ format: 'chadex.external_skill_roots.v1', roots: params.roots, script_roots: params.script_roots, revision: 'r2' }),
    getSkillCatalog: resolved({ project: project.path, catalog_revision: 'c', total_count: 0, returned_count: 0, skills: [], invalid_count: 0, diagnostics: [], discovery_truncated: false }),
    getSkillInventory: resolved({ project: project.path, total_count: 0, skills: [] }),
    getSkillDefinition: resolved({ skill_id: '', definition_revision: '', text: '', has_more: false }),
    installSkill: resolved({}), activateSkill: resolved({}), deactivateSkill: resolved({}), removeSkill: resolved({}),
    chooseSkillFolder: resolved(null), chooseSkillArchive: resolved(null),
    getGlobalInstructions: resolved(''), saveGlobalInstructions: (content) => Promise.resolve(content),
    storeCredential: resolved(undefined), forgetCredential: resolved(undefined), restartHelper: resolved(undefined), quitApp: resolved(undefined),
  };
}
