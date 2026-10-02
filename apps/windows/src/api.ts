import { invoke, isTauri } from '@tauri-apps/api/core';
import type { DesktopState, Preferences, ProjectInspection, RuntimeMethod } from './contracts';

export interface DesktopApi {
  desktopState(): Promise<DesktopState>;
  chooseProject(): Promise<string | null>;
  openProject(path: string): Promise<void>;
  runtimeAction(method: RuntimeMethod, params?: Record<string, unknown>): Promise<unknown>;
  inspectProject(path: string): Promise<ProjectInspection>;
  savePreferences(preferences: Preferences): Promise<Preferences>;
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
  savePreferences: (preferences) => call('save_preferences', { preferences }),
  storeCredential: (credential) => call('store_credential', { credential }),
  forgetCredential: () => call('forget_credential'),
  restartHelper: () => call('restart_helper'),
  quitApp: () => call('quit_app'),
};
