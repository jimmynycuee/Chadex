import { vi } from 'vitest';
import type { DesktopApi } from '../src/api';
import { defaultPreferences, type BackendSnapshot, type DesktopState, type TaskProgress } from '../src/contracts';

export function snapshot(overrides: Partial<BackendSnapshot> = {}): BackendSnapshot {
  return { phase: 'waiting_for_chatgpt_verification', selected_project: { path: 'C:\\work\\chadex', allowed_root: 'C:\\work\\chadex', is_git_repository: true, readable: true, writable: true },
    tunnel_ready: true, chat_gpt_connected: true, chat_gpt_verified_for_selected_project: false, last_verified_at_ms: null,
    current_operation: null, task_progress: null, mascot_jobs: [], error: null, activity_sequence: 0, state_revision: 1,
    runtime_status: { runtime_configured: true, runtime_ready: true, needs_attention: false, summary: '本地服務已啟動', next_action: null, summary_kind: 'ready', server: 'running', runner: 'running', exposure: 'local', project: 'selected' },
    tunnel_status: { configured: true, state: 'ready' }, ...overrides };
}
export function desktop(overrides: Partial<DesktopState> = {}): DesktopState {
  return { helper: { state: 'running', pid: 100, error: null }, runtime: snapshot(), activity: [],
    preferences: { ...defaultPreferences, recent_projects: ['C:\\work\\chadex'], tunnel_id: 'test-tunnel' },
    credential_stored: true, paths: { helper: 'C:\\app\\helper.exe', runtime: 'C:\\app\\runtime', data: 'C:\\user\\Chadex' }, version: '0.4.0', ...overrides };
}
export function task(overrides: Partial<TaskProgress> = {}): TaskProgress {
  return { task_id: 'task-1', project: 'C:\\work\\chadex', goal: 'Fix a bug', status: 'running', current_step: 0,
    total_steps: 3, completed_steps: 0, plan: ['edit', 'validate', 'read'], cancel_requested: false,
    started_at_ms: 0, validation: { status: 'pending' }, ...overrides };
}
export function apiMock(state = desktop()): DesktopApi {
  return { desktopState: vi.fn().mockResolvedValue(state), chooseProject: vi.fn().mockResolvedValue(null), openProject: vi.fn().mockResolvedValue(undefined),
    inspectProject: vi.fn().mockResolvedValue(state.runtime?.selected_project), runtimeAction: vi.fn().mockResolvedValue(snapshot()),
    savePreferences: vi.fn().mockImplementation(async (preferences) => preferences), storeCredential: vi.fn().mockResolvedValue(undefined),
    forgetCredential: vi.fn().mockResolvedValue(undefined), restartHelper: vi.fn().mockResolvedValue(undefined), quitApp: vi.fn().mockResolvedValue(undefined) };
}
export function deferred<T>() {
  let resolve!: (value: T) => void; let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}
