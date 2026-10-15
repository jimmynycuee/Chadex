import { beforeEach, describe, expect, it, vi } from 'vitest';
const core = vi.hoisted(() => {
  const channels: Array<{ onmessage?: (event: unknown) => void }> = [];
  const resources: Array<{ rid: number; close: ReturnType<typeof vi.fn> }> = [];
  class ChannelMock {
    onmessage?: (event: unknown) => void;
    constructor() { channels.push(this); }
  }
  class ResourceMock {
    close = vi.fn().mockResolvedValue(undefined);
    constructor(public rid: number) { resources.push(this); }
  }
  return { invoke: vi.fn().mockResolvedValue(undefined), isTauri: vi.fn().mockReturnValue(true), channels, resources,
    Channel: ChannelMock, Resource: ResourceMock };
});
vi.mock('@tauri-apps/api/core', () => core);
import { desktopApi, skillArchiveError } from '../src/api';
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
    await desktopApi.getSkillDefinition('p', 's1', 'd1', 'k1'); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'getSkillDefinition', params: { path: 'p', skill_id: 's1', definition_revision: 'd1', package_revision: 'k1' } });
    await desktopApi.getSkillInventory('p'); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'getSkillInventory', params: { path: 'p' } });
    await desktopApi.installSkill('p', 'k', 'a/b.zip'); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'installSkill', params: { path: 'p', skill_key: 'k', artifact_path: 'a/b.zip' } });
    const entry = { skill_id: 'i', skill_key: 'k', state_revision: 's', preferred_package_revision: 'pk', definition_revision: 'd', name: 'n', description: '', total_versions: 1 };
    await desktopApi.activateSkill('p', entry); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'activateSkill', params: { path: 'p', skill_key: 'k', package_revision: 'pk', state_revision: 's' } });
    await desktopApi.deactivateSkill('p', entry); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'deactivateSkill', params: { path: 'p', skill_key: 'k', state_revision: 's' } });
    await desktopApi.removeSkill('p', entry); expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'removeSkill', params: { path: 'p', skill_key: 'k', state_revision: 's' } });
    await desktopApi.chooseSkillFolder(); expect(core.invoke).toHaveBeenLastCalledWith('choose_skill_folder', undefined);
    await desktopApi.chooseSkillArchive('p'); expect(core.invoke).toHaveBeenLastCalledWith('choose_skill_archive', { project: 'p' });
    await desktopApi.savePreferences(desktop().preferences); expect(core.invoke).toHaveBeenLastCalledWith('save_preferences', { preferences: desktop().preferences });
    await desktopApi.getGlobalInstructions(); expect(core.invoke).toHaveBeenLastCalledWith('get_global_instructions', undefined);
    await desktopApi.saveGlobalInstructions('all projects'); expect(core.invoke).toHaveBeenLastCalledWith('save_global_instructions', { content: 'all projects' });
    await desktopApi.storeCredential('opaque'); expect(core.invoke).toHaveBeenLastCalledWith('store_credential', { credential: 'opaque' });
    await desktopApi.forgetCredential(); expect(core.invoke).toHaveBeenLastCalledWith('forget_credential', undefined);
    await desktopApi.restartHelper(); expect(core.invoke).toHaveBeenLastCalledWith('restart_helper', undefined);
    await desktopApi.hideWindow(); expect(core.invoke).toHaveBeenLastCalledWith('hide_main_window', undefined);
    expect('quitApp' in desktopApi).toBe(false);
  });
  it('fails honestly outside Tauri instead of substituting a demo-ready state', async () => {
    core.isTauri.mockReturnValueOnce(false);
    await expect(desktopApi.desktopState()).rejects.toThrow('需要 Chadex 桌面環境');
  });
  it('passes an absolute ZIP path to installSkill and turns skill_archive_* codes into readable messages', async () => {
    await desktopApi.installSkill('C:\\work\\p', 'k', 'C:\\Users\\a\\Downloads\\s.zip');
    expect(core.invoke).toHaveBeenLastCalledWith('runtime_action', { method: 'installSkill', params: { path: 'C:\\work\\p', skill_key: 'k', artifact_path: 'C:\\Users\\a\\Downloads\\s.zip' } });
    core.invoke.mockRejectedValueOnce('skill_archive_too_large');
    await expect(desktopApi.installSkill('p', 'k', 'C:\\a.zip')).rejects.toThrow('8 MB');
    core.invoke.mockRejectedValueOnce('skill_archive_not_zip');
    await expect(desktopApi.chooseSkillArchive('p')).rejects.toThrow('.zip');
    core.invoke.mockRejectedValueOnce('Chadex bridge: Backend (tool_failure)。請查看診斷並重試。');
    await expect(desktopApi.installSkill('p', 'k', 'C:\\a.zip')).rejects.toBe('Chadex bridge: Backend (tool_failure)。請查看診斷並重試。');
  });
  it('maps every skill_archive_* code the Rust side can return', () => {
    for (const code of ['skill_archive_not_zip', 'skill_archive_not_regular_file', 'skill_archive_too_large', 'skill_archive_unreadable',
      'skill_archive_project_unavailable', 'skill_archive_unsafe_directory', 'skill_archive_copy_failed']) {
      const mapped = skillArchiveError(code);
      expect(mapped).toBeInstanceOf(Error);
      expect((mapped as Error).message).not.toContain('skill_archive_');
    }
    expect(skillArchiveError(new Error('other'))).toEqual(new Error('other'));
  });
});

describe('Windows updater API', () => {
  beforeEach(() => {
    core.invoke.mockClear(); core.isTauri.mockClear().mockReturnValue(true);
    core.channels.length = 0; core.resources.length = 0;
  });

  it('checks only on request, maps the release metadata, and installs only after a separate call', async () => {
    core.invoke.mockResolvedValueOnce({ rid: 41, version: '0.7.2', date: '2026-10-15', body: 'Windows update' });
    await expect(desktopApi.checkForUpdate()).resolves.toEqual({ version: '0.7.2', date: '2026-10-15', body: 'Windows update' });
    expect(core.invoke).toHaveBeenLastCalledWith('plugin:updater|check');

    const onProgress = vi.fn();
    const installing = desktopApi.installUpdate(onProgress);
    const [, args] = core.invoke.mock.calls.at(-1)!;
    expect(core.invoke).toHaveBeenLastCalledWith('plugin:updater|download_and_install', expect.objectContaining({ rid: 41, onEvent: expect.any(Object) }));
    const channel = core.channels.at(-1)!;
    channel.onmessage?.({ event: 'Started', data: { contentLength: 100 } });
    channel.onmessage?.({ event: 'Progress', data: { chunkLength: 25 } });
    channel.onmessage?.({ event: 'Progress', data: { chunkLength: 75 } });
    channel.onmessage?.({ event: 'Finished' });
    await installing;
    expect(onProgress.mock.calls).toEqual([[0, 100], [25, 100], [100, 100], [100, 100]]);
    expect(args).toEqual(expect.objectContaining({ rid: 41 }));
    await expect(desktopApi.installUpdate()).rejects.toThrow('先檢查更新');
  });

  it('returns null when no update is available and does not start an installer', async () => {
    core.invoke.mockResolvedValueOnce(null);
    await expect(desktopApi.checkForUpdate()).resolves.toBeNull();
    expect(core.invoke).toHaveBeenLastCalledWith('plugin:updater|check');
    expect(core.invoke.mock.calls.some(([command]) => command === 'plugin:updater|download_and_install')).toBe(false);
  });

  it('fails safely outside Tauri and on malformed update metadata', async () => {
    core.isTauri.mockReturnValueOnce(false);
    await expect(desktopApi.checkForUpdate()).rejects.toThrow('桌面環境');
    core.invoke.mockResolvedValueOnce({ rid: 42, version: '' });
    await expect(desktopApi.checkForUpdate()).rejects.toThrow('無效的版本資訊');
    expect(core.resources.at(-1)?.rid).toBe(42);
    expect(core.resources.at(-1)?.close).toHaveBeenCalledOnce();
  });

  it('clears and releases an update resource after an installation error', async () => {
    core.invoke.mockResolvedValueOnce({ rid: 43, version: '0.7.2' });
    await desktopApi.checkForUpdate();
    core.invoke.mockRejectedValueOnce(new Error('download failed'));
    await expect(desktopApi.installUpdate()).rejects.toThrow('download failed');
    expect(core.resources.at(-1)?.close).toHaveBeenCalledOnce();
    await expect(desktopApi.installUpdate()).rejects.toThrow('先檢查更新');
  });
});
