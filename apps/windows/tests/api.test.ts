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
    await desktopApi.discoverExternalSkillSources(); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'discoverExternalSkillSources', params: {} });
    await desktopApi.getExternalSkillRoots(); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'getExternalSkillRoots', params: {} });
    const rootsParams = { roots: ['r'], script_roots: [], expected_revision: 'rev', verify_project_path: 'p' };
    await desktopApi.setExternalSkillRoots(rootsParams); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'setExternalSkillRoots', params: rootsParams });
    await desktopApi.getSkillCatalog('p'); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'getSkillCatalog', params: { path: 'p' } });
    await desktopApi.getSkillInventory('p'); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'getSkillInventory', params: { path: 'p' } });
    await desktopApi.installSkill('p', 'k', 'a/b.zip'); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'installSkill', params: { path: 'p', skill_key: 'k', artifact_path: 'a/b.zip' } });
    const entry = { skill_id: 'i', skill_key: 'k', state_revision: 's', preferred_package_revision: 'pk', definition_revision: 'd', name: 'n', description: '', total_versions: 1 };
    await desktopApi.activateSkill('p', entry); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'activateSkill', params: { path: 'p', skill_key: 'k', package_revision: 'pk', state_revision: 's' } });
    await desktopApi.deactivateSkill('p', entry); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'deactivateSkill', params: { path: 'p', skill_key: 'k', state_revision: 's' } });
    await desktopApi.chooseSkillFolder(); expect(core.invoke).toHaveBeenLastCalledWith('choose_skill_folder', undefined);
    await desktopApi.chooseSkillArchive('p'); expect(core.invoke).toHaveBeenLastCalledWith('choose_skill_archive', { project: 'p' });
    await desktopApi.savePreferences(desktop().preferences); expect(core.invoke).toHaveBeenLastCalledWith('save_preferences', { preferences: desktop().preferences });
    await desktopApi.getGlobalInstructions(); expect(core.invoke).toHaveBeenLastCalledWith('get_global_instructions', undefined);
    await desktopApi.saveGlobalInstructions('all projects'); expect(core.invoke).toHaveBeenLastCalledWith('save_global_instructions', { content: 'all projects' });
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
