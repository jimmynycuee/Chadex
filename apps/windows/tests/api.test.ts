import { describe, expect, it, vi } from 'vitest';
const core = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue(undefined), isTauri: vi.fn().mockReturnValue(true) }));
vi.mock('@tauri-apps/api/core', () => core);
import { desktopApi } from '../src/api';
import { desktop } from './fixtures';

describe('Tauri 2 command contract', () => {
  it('uses named IPC commands and keeps secrets away from runtime_action', async () => {
    await desktopApi.desktopState(); expect(core.invoke).toHaveBeenLastCalledWith('desktop_state', undefined);
    await desktopApi.chooseProject(); expect(core.invoke).toHaveBeenLastCalledWith('choose_project', undefined);
    await desktopApi.runtimeAction('connectChatGPT'); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'connectChatGPT', params: {} });
    await desktopApi.inspectProject('C:\\project'); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'inspectProject', params: { path: 'C:\\project' } });
    await desktopApi.savePreferences(desktop().preferences); expect(core.invoke).toHaveBeenLastCalledWith('save_preferences', { preferences: desktop().preferences });
    await desktopApi.storeCredential('opaque'); expect(core.invoke).toHaveBeenLastCalledWith('store_credential', { credential: 'opaque' });
    await desktopApi.forgetCredential(); expect(core.invoke).toHaveBeenLastCalledWith('forget_credential', undefined);
    await desktopApi.restartHelper(); expect(core.invoke).toHaveBeenLastCalledWith('restart_helper', undefined);
    await desktopApi.quitApp(); expect(core.invoke).toHaveBeenLastCalledWith('quit_app', undefined);
  });
  it('fails honestly outside Tauri instead of substituting a demo-ready state', async () => {
    core.isTauri.mockReturnValueOnce(false);
    await expect(desktopApi.desktopState()).rejects.toThrow('需要 Chadex 桌面環境');
  });
});
