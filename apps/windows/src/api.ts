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
  /** Native ZIP picker; resolves a project-relative path (forward slashes), or null on cancel. */
  chooseSkillArchive(project: string): Promise<string | null>;
  getGlobalInstructions(): Promise<string>;
  saveGlobalInstructions(content: string): Promise<string>;
  storeCredential(credential: string): Promise<void>;
  forgetCredential(): Promise<void>;
  restartHelper(): Promise<void>;
  quitApp(): Promise<void>;
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isTauri()) throw new Error('此頁面需要 Chadex 桌面環境。請由 Windows 桌面應用程式開啟。');
  return invoke<T>(command, args);
}
export const desktopApi: DesktopApi = {
  desktopState: () => call('desktop_state'),
  chooseProject: () => call('choose_project'),
  openProject: (path) => call('open_project', { path }),
  runtimeAction: (method, params = {}) => call('runtime_action', { method, params }),
  inspectProject: (path) => call('runtime_action', { method: 'inspectProject', params: { path } }),
  discoverExternalSkillSources: () => call('runtime_action', { method: 'discoverExternalSkillSources', params: {} }),
  getExternalSkillRoots: () => call('runtime_action', { method: 'getExternalSkillRoots', params: {} }),
  setExternalSkillRoots: (params) => call('runtime_action', { method: 'setExternalSkillRoots', params }),
  getSkillCatalog: (path) => call('runtime_action', { method: 'getSkillCatalog', params: { path } }),
  getSkillInventory: (path) => call('runtime_action', { method: 'getSkillInventory', params: { path } }),
  installSkill: (path, skillKey, artifactPath) => call('runtime_action', { method: 'installSkill', params: { path, skill_key: skillKey, artifact_path: artifactPath } }),
  activateSkill: (path, entry) => call('runtime_action', { method: 'activateSkill',
    params: { path, skill_key: entry.skill_key, package_revision: entry.preferred_package_revision, state_revision: entry.state_revision } }),
  deactivateSkill: (path, entry) => call('runtime_action', { method: 'deactivateSkill', params: { path, skill_key: entry.skill_key, state_revision: entry.state_revision } }),
  chooseSkillFolder: () => call('choose_skill_folder'),
  chooseSkillArchive: (project) => call('choose_skill_archive', { project }),
  savePreferences: (preferences) => call('save_preferences', { preferences }),
  getGlobalInstructions: () => call('get_global_instructions'),
  saveGlobalInstructions: (content) => call('save_global_instructions', { content }),
  storeCredential: (credential) => call('store_credential', { credential }),
  forgetCredential: () => call('forget_credential'),
  restartHelper: () => call('restart_helper'),
  quitApp: () => call('quit_app'),
};
