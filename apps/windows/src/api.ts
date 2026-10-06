import { invoke, isTauri } from '@tauri-apps/api/core';
import type {
  DesktopState, ExternalSkillRootsState, ExternalSkillSourceDiscovery, ManagedSkillEntry, Preferences, ProjectInspection,
  RuntimeMethod, SetExternalSkillRootsParams, SkillCatalog, SkillInventory,
} from './contracts';

export interface DesktopApi {
  desktopState(): Promise<DesktopState>;
  chooseProject(): Promise<string | null>;
  openProject(path: string): Promise<void>;
  runtimeAction(method: RuntimeMethod, params?: Record<string, unknown>): Promise<unknown>;
  /** Fire-and-forget launch warm-up; the helper decides whether anything needs resuming. */
  prewarmRuntime(): Promise<unknown>;
  inspectProject(path: string): Promise<ProjectInspection>;
  savePreferences(preferences: Preferences): Promise<Preferences>;
  discoverExternalSkillSources(): Promise<ExternalSkillSourceDiscovery>;
  getExternalSkillRoots(): Promise<ExternalSkillRootsState>;
  setExternalSkillRoots(params: SetExternalSkillRootsParams): Promise<ExternalSkillRootsState>;
  getSkillCatalog(path: string): Promise<SkillCatalog>;
  getSkillInventory(path: string): Promise<SkillInventory>;
  installSkill(path: string, skillKey: string, artifactPath: string): Promise<unknown>;
  activateSkill(path: string, entry: ManagedSkillEntry): Promise<unknown>;
  deactivateSkill(path: string, entry: ManagedSkillEntry): Promise<unknown>;
  /** Native folder picker; resolves the exact canonical path the helper requires, or null on cancel. */
  chooseSkillFolder(): Promise<string | null>;
  /**
   * Native ZIP picker; resolves the chosen ZIP's absolute path (it may be anywhere), or null on cancel.
   * `installSkill` copies it into the project's `.chadex/skill-imports/`, installs from that copy and
   * deletes the copy afterwards. `project` is kept for the IPC contract only.
   */
  chooseSkillArchive(project: string): Promise<string | null>;
  getGlobalInstructions(): Promise<string>;
  saveGlobalInstructions(content: string): Promise<string>;
  storeCredential(credential: string): Promise<void>;
  forgetCredential(): Promise<void>;
  restartHelper(): Promise<void>;
  quitApp(): Promise<void>;
}

const SKILL_ARCHIVE_ERRORS: Record<string, string> = {
  skill_archive_not_zip: '請選擇 .zip 檔案。',
  skill_archive_not_regular_file: '所選項目不是一般檔案（不支援捷徑、連結或資料夾），請直接選擇 ZIP 檔。',
  skill_archive_too_large: '這個 ZIP 超過 Skill 套件 8 MB 的上限。',
  skill_archive_unreadable: '無法讀取所選的 ZIP，請確認檔案仍存在且你有權限開啟。',
  skill_archive_project_unavailable: '找不到目前專案的資料夾，請重新整理專案後再試。',
  skill_archive_unsafe_directory: '專案內的 .chadex 資料夾是連結或不是資料夾，無法安全地複製 ZIP 進去。請移除或修正 .chadex\\skill-imports 後再試。',
  skill_archive_copy_failed: '無法把 ZIP 複製到專案內，請確認專案資料夾可寫入且仍有可用空間後再試。',
};
/** Replaces a `skill_archive_*` backend code with a readable message; anything else is rethrown as is. */
export function skillArchiveError(error: unknown): unknown {
  const text = typeof error === 'string' ? error : error instanceof Error ? error.message
    : error && typeof error === 'object' && 'message' in error ? String(error.message) : '';
  const code = Object.keys(SKILL_ARCHIVE_ERRORS).find((key) => text.includes(key));
  return code ? new Error(SKILL_ARCHIVE_ERRORS[code]) : error;
}
function rethrowSkillArchiveError(error: unknown): never { throw skillArchiveError(error); }

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error('此頁面需要 Chadex 桌面環境。請由 Windows 桌面應用程式開啟。');
  return invoke<T>(command, args);
}
export const desktopApi: DesktopApi = {
  desktopState: () => call('desktop_state'),
  chooseProject: () => call('choose_project'),
  openProject: (path) => call('open_project', { path }),
  runtimeAction: (method, params = {}) => call('runtime_action', { method, params }),
  prewarmRuntime: () => call('runtime_action', { method: 'prewarmRuntime', params: {} }),
  inspectProject: (path) => call('runtime_action', { method: 'inspectProject', params: { path } }),
  discoverExternalSkillSources: () => call('runtime_action', { method: 'discoverExternalSkillSources', params: {} }),
  getExternalSkillRoots: () => call('runtime_action', { method: 'getExternalSkillRoots', params: {} }),
  setExternalSkillRoots: (params) => call('runtime_action', { method: 'setExternalSkillRoots', params }),
  getSkillCatalog: (path) => call('runtime_action', { method: 'getSkillCatalog', params: { path } }),
  getSkillInventory: (path) => call('runtime_action', { method: 'getSkillInventory', params: { path } }),
  installSkill: (path, skillKey, artifactPath) => call<unknown>('runtime_action', { method: 'installSkill', params: { path, skill_key: skillKey, artifact_path: artifactPath } }).catch(rethrowSkillArchiveError),
  activateSkill: (path, entry) => call('runtime_action', { method: 'activateSkill',
    params: { path, skill_key: entry.skill_key, package_revision: entry.preferred_package_revision, state_revision: entry.state_revision } }),
  deactivateSkill: (path, entry) => call('runtime_action', { method: 'deactivateSkill', params: { path, skill_key: entry.skill_key, state_revision: entry.state_revision } }),
  chooseSkillFolder: () => call('choose_skill_folder'),
  chooseSkillArchive: (project) => call<string | null>('choose_skill_archive', { project }).catch(rethrowSkillArchiveError),
  savePreferences: (preferences) => call('save_preferences', { preferences }),
  getGlobalInstructions: () => call('get_global_instructions'),
  saveGlobalInstructions: (content) => call('save_global_instructions', { content }),
  storeCredential: (credential) => call('store_credential', { credential }),
  forgetCredential: () => call('forget_credential'),
  restartHelper: () => call('restart_helper'),
  quitApp: () => call('quit_app'),
};
