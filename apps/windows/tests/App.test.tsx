import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { App } from '../src/App';
import { DesktopStore } from '../src/state';
import { apiMock, deferred, desktop, skillSource, snapshot } from './fixtures';

afterEach(() => { cleanup(); localStorage.clear(); sessionStorage.clear(); });
async function mount(state = desktop()) {
  const api = apiMock(state); const store = new DesktopStore(api);
  render(<App api={api} store={store} />);
  await waitFor(() => expect(store.getSnapshot().freshness).toBe('fresh'));
  return { api, store };
}
describe('desktop pages and IPC behavior', () => {
  it('renders the core pages and distinguishes local ready from verified connection', async () => {
    await mount();
    expect(screen.getAllByText('本地服務可用').length).toBeGreaterThan(0);
    expect(screen.queryByText('專案已通過 ChatGPT 驗證')).toBeNull();
    for (const [button, heading] of [['專案', '專案'], ['連線', '連線'], ['活動紀錄', '活動紀錄'], ['設定', '設定'], ['診斷', '診斷']] as const) {
      fireEvent.click(screen.getByRole('button', { name: button }));
      expect(screen.getByRole('heading', { level: 1, name: heading })).toBeTruthy();
    }
    fireEvent.click(screen.getByRole('button', { name: /Windows Desktop/ }));
    expect(screen.getByRole('heading', { level: 1, name: '版本與更新' })).toBeTruthy();
    expect(screen.getByText('尚未提供更新資訊')).toBeTruthy();
  });
  it('does not show fake connection success from an action result', async () => {
    const { api } = await mount();
    vi.mocked(api.runtimeAction).mockResolvedValue(snapshot({ phase: 'verified', chat_gpt_verified_for_selected_project: true }));
    fireEvent.click(screen.getByRole('button', { name: '連線' }));
    fireEvent.click(screen.getByRole('button', { name: '連接 ChatGPT' }));
    await waitFor(() => expect(api.runtimeAction).toHaveBeenCalledWith('connectChatGPT', {}));
    await waitFor(() => expect(screen.queryByText('連接 ChatGPT…')).toBeNull());
    expect(screen.queryByText('專案已通過 ChatGPT 驗證')).toBeNull();
    expect(screen.getByText('waiting_for_chatgpt_verification')).toBeTruthy();
  });
  it('removes ready and verified text after a poll failure', async () => {
    const { api, store } = await mount(desktop({ runtime: snapshot({ phase: 'verified', chat_gpt_verified_for_selected_project: true }) }));
    expect(screen.getAllByText('本地服務可用').length).toBeGreaterThan(0);
    vi.mocked(api.desktopState).mockRejectedValueOnce(new Error('pipe closed'));
    await act(async () => { await store.refresh(); });
    expect(screen.queryByText('本地服務可用')).toBeNull();
    expect(screen.queryByText('ChatGPT 已驗證')).toBeNull();
    expect(screen.getByText('pipe closed')).toBeTruthy();
  });
  it('withdraws visible ChatGPT verification when runtime readiness dies', async () => {
    const verified = snapshot({ phase: 'verified', chat_gpt_verified_for_selected_project: true });
    const { api, store } = await mount(desktop({ runtime: verified }));
    expect(screen.getByText('ChatGPT 已驗證')).toBeTruthy();
    vi.mocked(api.desktopState).mockResolvedValueOnce(desktop({ runtime: { ...verified, state_revision: 2,
      runtime_status: { ...verified.runtime_status!, runtime_ready: false, needs_attention: true } } }));
    await act(async () => { await store.refresh(); });
    expect(screen.queryByText('ChatGPT 已驗證')).toBeNull();
    expect(screen.queryByText('專案已通過 ChatGPT 驗證')).toBeNull();
    expect(screen.queryByText('本地服務可用')).toBeNull();
    expect(screen.getAllByText('本地服務需要處理').length).toBeGreaterThan(0);
  });
  it('shows helper stopping and optional diagnostics errors without blocking the app', async () => {
    await mount(desktop({ helper: { state: 'stopping', pid: 100, error: null }, startup_error: 'preferences_invalid', credential_error: 'credential_inaccessible', runtime_version: '2.0.0' }));
    expect(screen.getByText('Helper 正在停止')).toBeTruthy();
    expect(screen.queryByText('本地服務可用')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: '診斷' }));
    expect(screen.getByText('preferences_invalid')).toBeTruthy(); expect(screen.getByText('credential_inaccessible')).toBeTruthy();
    expect(screen.getByText('2.0.0')).toBeTruthy();
  });
  it('stores credential through the secure IPC only, clears the input, and never re-displays it', async () => {
    const { api } = await mount();
    const pending = deferred<void>(); vi.mocked(api.storeCredential).mockReturnValueOnce(pending.promise);
    fireEvent.click(screen.getByRole('button', { name: '連線' }));
    const input = screen.getByLabelText('API credential') as HTMLInputElement;
    expect(input.type).toBe('password');
    fireEvent.change(input, { target: { value: 'private-opaque-token' } });
    fireEvent.click(screen.getByRole('button', { name: '安全儲存' }));
    await waitFor(() => expect(api.storeCredential).toHaveBeenCalledWith('private-opaque-token'));
    expect(input.value).toBe(''); expect(localStorage.length).toBe(0); expect(sessionStorage.length).toBe(0);
    await act(async () => { pending.resolve(undefined); });
    await waitFor(() => expect(screen.queryByText('安全儲存憑證…')).toBeNull());
    expect(document.body.textContent).not.toContain('private-opaque-token');
    expect(api.runtimeAction).not.toHaveBeenCalled();
  });
  it('does not expose a credential even if backend rejects with that secret in the error', async () => {
    const { api } = await mount();
    vi.mocked(api.storeCredential).mockRejectedValueOnce(new Error('opaque-private-secret'));
    fireEvent.click(screen.getByRole('button', { name: '連線' }));
    fireEvent.change(screen.getByLabelText('API credential'), { target: { value: 'opaque-private-secret' } });
    fireEvent.click(screen.getByRole('button', { name: '安全儲存' }));
    await waitFor(() => expect(screen.getByText(/憑證儲存失敗/)).toBeTruthy());
    expect(document.body.textContent).not.toContain('opaque-private-secret');
  });
  it('treats native folder picker cancellation as a no-op', async () => {
    const { api } = await mount();
    fireEvent.click(screen.getByRole('button', { name: '專案' }));
    fireEvent.click(screen.getByRole('button', { name: /選擇資料夾/ }));
    await waitFor(() => expect(api.chooseProject).toHaveBeenCalledTimes(1));
    expect(api.inspectProject).not.toHaveBeenCalled(); expect(api.runtimeAction).not.toHaveBeenCalled();
    expect(api.savePreferences).not.toHaveBeenCalled();
  });
  it('activates through the real method and delegates recent project persistence to backend', async () => {
    const { api } = await mount(desktop({ runtime: snapshot({ selected_project: null }), preferences: { ...desktop().preferences, recent_projects: [] } }));
    const checked = { ...snapshot().selected_project!, path: 'C:\\new-project', allowed_root: 'C:\\new-project' };
    vi.mocked(api.chooseProject).mockResolvedValueOnce(checked.path); vi.mocked(api.inspectProject).mockResolvedValueOnce(checked);
    fireEvent.click(screen.getByRole('button', { name: '專案' }));
    fireEvent.click(screen.getByRole('button', { name: /選擇資料夾/ }));
    await waitFor(() => expect(screen.getByRole('heading', { level: 1, name: '專案詳情' })).toBeTruthy());
    await waitFor(() => expect(screen.queryByText('選擇專案…')).toBeNull());
    fireEvent.click(screen.getByRole('button', { name: '設為目前專案' }));
    await waitFor(() => expect(api.runtimeAction).toHaveBeenCalledWith('activateProject', { path: checked.path }));
    expect(api.savePreferences).not.toHaveBeenCalled();
  });
  it('edits Chadex Global Instructions without a selected project or runtime action', async () => {
    const state = desktop({ runtime: snapshot({ selected_project: null }) });
    const api = apiMock(state);
    vi.mocked(api.getGlobalInstructions).mockResolvedValueOnce('shared global rule');
    const store = new DesktopStore(api);
    render(<App api={api} store={store} />);
    await waitFor(() => expect(store.getSnapshot().freshness).toBe('fresh'));
    await waitFor(() => expect(api.getGlobalInstructions).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: '設定' }));
    const editor = await screen.findByLabelText('Chadex Global Instructions') as HTMLTextAreaElement;
    expect(editor.value).toBe('shared global rule');
    fireEvent.change(editor, { target: { value: '' } });
    fireEvent.click(screen.getByRole('button', { name: '儲存 Global Instructions' }));
    await waitFor(() => expect(api.saveGlobalInstructions).toHaveBeenCalledWith(''));
    expect(api.runtimeAction).not.toHaveBeenCalled();
  });

  it('keeps the companion in sidebar and allows disabling it from backend preferences', async () => {
    const { api, store } = await mount();
    expect(screen.getByLabelText(/Code Ferret：/).closest('aside')).toBeTruthy();
    vi.mocked(api.desktopState).mockResolvedValueOnce(desktop({ preferences: { ...desktop().preferences, ferret_visible: false } }));
    await act(async () => { await store.refresh(); });
    expect(screen.queryByLabelText(/Code Ferret：/)).toBeNull();
  });
  it('renders raw helper trace and job facts, including real tool name, completion, status and elapsed', async () => {
    // Same safe snake_case projection as Rust McpPerformanceTrace, not a
    // message-derived synthetic command or a demo state in production code.
    await mount(desktop({ runtime: snapshot({ mascot_jobs: [{ job_id: 'job-fixture-1', status: 'exited', exit_code: 0, started_at_ms: 100, finished_at_ms: 1_334 }] }),
      traces: [{ sequence: 7, started_at_ms: 100, finished_at_ms: 1_334, tool_names: ['run_process', 'read_project_files'],
        tool_failed: false, status_code: 200, total_us: 1_234_000, completion: 'completed' }] }));
    fireEvent.click(screen.getByRole('button', { name: '活動紀錄' }));
    expect(screen.getByText('job-fixture-1')).toBeTruthy(); expect(screen.getByText('exited')).toBeTruthy();
    expect(screen.getByText('0')).toBeTruthy(); expect(screen.getByText('run_process')).toBeTruthy();
    expect(screen.getByText('read_project_files')).toBeTruthy(); expect(screen.getByText('completed')).toBeTruthy();
    expect(screen.getByText('200')).toBeTruthy(); expect(screen.getByText('1,234 ms')).toBeTruthy();
  });
  it.each([undefined, null])('uses an empty trace fallback for older helper projection (%s)', async (traces) => {
    await mount(desktop({ traces })); fireEvent.click(screen.getByRole('button', { name: '活動紀錄' }));
    expect(screen.getByText('目前沒有可觀測的工具呼叫紀錄。')).toBeTruthy();
    expect(screen.getByText('目前沒有可觀測的 Job。')).toBeTruthy();
  });
  it('withdraws stale traces and jobs when desktop refresh fails', async () => {
    const { api, store } = await mount(desktop({ runtime: snapshot({ mascot_jobs: [{ job_id: 'job-live', status: 'running', exit_code: null, started_at_ms: 0, finished_at_ms: null }] }),
      traces: [{ sequence: 1, started_at_ms: 0, finished_at_ms: null, tool_names: ['run_process'], completion: 'running' }] }));
    fireEvent.click(screen.getByRole('button', { name: '活動紀錄' }));
    expect(screen.getByText('run_process')).toBeTruthy(); expect(screen.getByText('job-live')).toBeTruthy();
    vi.mocked(api.desktopState).mockRejectedValueOnce(new Error('helper observation failed'));
    await act(async () => { await store.refresh(); });
    expect(screen.queryByText('run_process')).toBeNull(); expect(screen.queryByText('job-live')).toBeNull();
    expect(screen.getByText('工具呼叫狀態未確認。')).toBeTruthy(); expect(screen.getByText('Job 狀態未確認。')).toBeTruthy();
  });
});

describe('Skills page', () => {
  async function openSkills(setup?: (api: ReturnType<typeof apiMock>) => void) {
    const api = apiMock(desktop()); setup?.(api); const store = new DesktopStore(api);
    render(<App api={api} store={store} />);
    await waitFor(() => expect(store.getSnapshot().freshness).toBe('fresh'));
    fireEvent.click(screen.getByRole('button', { name: 'Skills' }));
    await waitFor(() => expect(api.getExternalSkillRoots).toHaveBeenCalled());
    return api;
  }
  it('makes import prominent when no valid source is detected', async () => {
    await openSkills();
    const button = await screen.findByRole('button', { name: '匯入 Skill（ZIP）…' });
    expect(button.className).toContain('primary');
  });
  it('demotes import to a secondary action when a source is found, and connects with scripts off', async () => {
    const source = skillSource();
    const api = await openSkills((mock) => { vi.mocked(mock.discoverExternalSkillSources).mockResolvedValue({ format: 'f', sources: [source], recommended_roots: [] }); });
    const button = await screen.findByRole('button', { name: '匯入 Skill（ZIP）…' });
    expect(button.className).not.toContain('primary');
    const [connect, scripts] = await screen.findAllByRole('checkbox') as HTMLInputElement[];
    expect(scripts.disabled).toBe(true);
    fireEvent.click(connect);
    await waitFor(() => expect(api.setExternalSkillRoots).toHaveBeenCalledWith({ roots: [source.canonical_path], script_roots: [], expected_revision: 'rev1', verify_project_path: 'C:\\work\\chadex' }));
    await waitFor(() => expect((screen.getAllByRole('checkbox')[1] as HTMLInputElement).disabled).toBe(false));
    fireEvent.click(screen.getAllByRole('checkbox')[1]);
    await waitFor(() => expect(api.setExternalSkillRoots).toHaveBeenLastCalledWith(expect.objectContaining({ roots: [source.canonical_path], script_roots: [source.canonical_path], expected_revision: 'rev2' })));
  });
  it('reloads roots and shows a clear message on a revision conflict', async () => {
    const source = skillSource();
    const api = await openSkills((mock) => { vi.mocked(mock.discoverExternalSkillSources).mockResolvedValue({ format: 'f', sources: [source], recommended_roots: [] });
      vi.mocked(mock.setExternalSkillRoots).mockRejectedValue('Chadex bridge: Backend (external_skill_roots_conflict)。請查看診斷並重試。'); });
    fireEvent.click((await screen.findAllByRole('checkbox'))[0]);
    expect(await screen.findByText(/已在其他地方變更/)).toBeTruthy();
    await waitFor(() => expect(api.getExternalSkillRoots).toHaveBeenCalledTimes(2));
  });
  it('connects a chosen folder using its exact canonical path', async () => {
    const api = await openSkills((mock) => { vi.mocked(mock.chooseSkillFolder).mockResolvedValue('\\\\?\\D:\\my-skills'); });
    await waitFor(() => expect((screen.getByRole('button', { name: '選擇資料夾…' }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole('button', { name: '選擇資料夾…' }));
    await waitFor(() => expect(api.setExternalSkillRoots).toHaveBeenCalledWith(expect.objectContaining({ roots: ['\\\\?\\D:\\my-skills'] })));
    expect(await screen.findByText('D:\\my-skills')).toBeTruthy();
  });
  it('lists catalog skills, flags scripts-off, and toggles managed skills with the inventory revision', async () => {
    const managed = { skill_id: 'm1', skill_key: 'mine', state_revision: 'st1', active_package_revision: 'pk1', preferred_package_revision: 'pk1', definition_revision: 'def-managed-1', name: 'Mine', description: 'managed one', total_versions: 1 };
    const api = await openSkills((mock) => {
      vi.mocked(mock.getSkillCatalog).mockResolvedValue({ project: 'p', catalog_revision: 'c', total_count: 2, returned_count: 2, invalid_count: 0, diagnostics: [], discovery_truncated: false, skills: [
        { skill_id: 'm1', name: 'Mine', description: 'managed one', definition_revision: 'def-managed-1', package_revision: 'pk1', source_scope: 'runner', trust: 'operator_installed_guidance', name_conflict: false, scripts_allowed: true },
        { skill_id: 'x1', name: 'Ext', description: 'external', definition_revision: 'def-ext', source_scope: 'configured', trust: 'operator_configured_guidance', name_conflict: false, scripts_allowed: false }] });
      vi.mocked(mock.getSkillInventory).mockResolvedValue({ project: 'p', total_count: 1, skills: [managed] });
    });
    expect(await screen.findByText('腳本關閉')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: '停用 Mine' }));
    await waitFor(() => expect(api.deactivateSkill).toHaveBeenCalledWith('C:\\work\\chadex', managed));
  });
  const managedFixtures = (mock: ReturnType<typeof apiMock>, active = true) => {
    const managed = { skill_id: 'm1', skill_key: 'mine', state_revision: 'st1', active_package_revision: active ? 'pk1' : null, preferred_package_revision: 'pk1', definition_revision: 'def-managed-1', name: 'Mine', description: 'managed one', total_versions: 2 };
    vi.mocked(mock.getSkillCatalog).mockResolvedValue({ project: 'p', catalog_revision: 'c', total_count: 2, returned_count: 2, invalid_count: 0, diagnostics: [], discovery_truncated: false, skills: [
      { skill_id: 'm1', name: 'Mine', description: 'managed one', definition_revision: 'def-managed-1', package_revision: 'pk1', source_scope: 'runner', trust: 'operator_installed_guidance', name_conflict: false, scripts_allowed: true },
      { skill_id: 'x1', name: 'Ext', description: 'external', definition_revision: 'def-ext', source_scope: 'configured', trust: 'operator_configured_guidance', name_conflict: false, scripts_allowed: false }] });
    vi.mocked(mock.getSkillInventory).mockResolvedValue({ project: 'p', total_count: 1, skills: [managed] });
    return managed;
  };
  it('removes an installed skill only after confirmation, then refreshes', async () => {
    let managed!: ReturnType<typeof managedFixtures>;
    const api = await openSkills((mock) => { managed = managedFixtures(mock, false); });
    fireEvent.click(await screen.findByRole('button', { name: '展開 Mine' }));
    fireEvent.click(await screen.findByRole('button', { name: '移除 Mine…' }));
    expect(api.removeSkill).not.toHaveBeenCalled();
    expect(screen.getByText(/所有版本，且無法復原/)).toBeTruthy();
    const before = vi.mocked(api.getSkillCatalog).mock.calls.length;
    fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: '移除' }));
    await waitFor(() => expect(api.removeSkill).toHaveBeenCalledWith('C:\\work\\chadex', managed));
    await waitFor(() => expect(vi.mocked(api.getSkillCatalog).mock.calls.length).toBeGreaterThan(before));
  });
  it('cancelling the removal confirmation changes nothing', async () => {
    const api = await openSkills((mock) => { managedFixtures(mock); });
    fireEvent.click(await screen.findByRole('button', { name: '展開 Mine' }));
    fireEvent.click(await screen.findByRole('button', { name: '移除 Mine…' }));
    fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: '取消' }));
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(api.removeSkill).not.toHaveBeenCalled();
  });
  it('shows the removal failure after the refresh instead of hiding it', async () => {
    await openSkills((mock) => { managedFixtures(mock, false);
      vi.mocked(mock.removeSkill).mockRejectedValue('Chadex bridge: Backend (skill_remove_revision_failed)。請查看診斷並重試。'); });
    fireEvent.click(await screen.findByRole('button', { name: '展開 Mine' }));
    fireEvent.click(await screen.findByRole('button', { name: '移除 Mine…' }));
    fireEvent.click(within(screen.getByRole('alertdialog')).getByRole('button', { name: '移除' }));
    expect(await screen.findByText(/再移除一次/)).toBeTruthy();
  });
  it('shows an enable failure after the refresh instead of hiding it', async () => {
    await openSkills((mock) => { managedFixtures(mock, false);
      vi.mocked(mock.activateSkill).mockRejectedValue('Chadex bridge: Backend (skill_activate_failed)。請查看診斷並重試。'); });
    fireEvent.click(await screen.findByRole('button', { name: '啟用 Mine' }));
    expect(await screen.findByText(/啟用狀態/)).toBeTruthy();
  });
  it('offers no removal for external skills', async () => {
    await openSkills((mock) => { managedFixtures(mock); });
    fireEvent.click(await screen.findByRole('button', { name: '展開 Ext' }));
    expect(screen.queryByRole('button', { name: '移除 Ext…' })).toBeNull();
  });
  const skillFixtures = (mock: ReturnType<typeof apiMock>, scripts = false) => {
    vi.mocked(mock.getSkillCatalog).mockResolvedValue({ project: 'p', catalog_revision: 'c', total_count: 1, returned_count: 1, invalid_count: 0, diagnostics: [], discovery_truncated: false, skills: [
      { skill_id: 'x1', name: 'Ext', description: 'external one', definition_revision: 'def-1', package_revision: null, source_scope: 'configured', trust: 'operator_configured_guidance', name_conflict: false, scripts_allowed: scripts }] });
  };
  it('loads SKILL.md only when a skill is expanded', async () => {
    const api = await openSkills((mock) => { skillFixtures(mock);
      vi.mocked(mock.getSkillDefinition).mockResolvedValue({ skill_id: 'x1', definition_revision: 'def-1', package_revision: null, text: '# Ext skill body', has_more: true }); });
    const toggle = await screen.findByRole('button', { name: '展開 Ext' });
    expect(api.getSkillDefinition).not.toHaveBeenCalled();
    fireEvent.click(toggle);
    expect(await screen.findByText('# Ext skill body')).toBeTruthy();
    expect(api.getSkillDefinition).toHaveBeenCalledWith('C:\\work\\chadex', 'x1', 'def-1', null);
    expect(screen.getByText('內容過長，僅顯示前段。')).toBeTruthy();
  });
  it('shows a SKILL.md load error and refreshes the list', async () => {
    const api = await openSkills((mock) => { skillFixtures(mock);
      vi.mocked(mock.getSkillDefinition).mockRejectedValue('Chadex bridge: Backend (tool_failure)。請查看診斷並重試。'); });
    const before = vi.mocked(api.getSkillCatalog).mock.calls.length;
    fireEvent.click(await screen.findByRole('button', { name: '展開 Ext' }));
    expect(await screen.findByText(/無法載入 SKILL.md/)).toBeTruthy();
    await waitFor(() => expect(vi.mocked(api.getSkillCatalog).mock.calls.length).toBeGreaterThan(before));
  });
  it('refreshes instead of showing a SKILL.md whose revision no longer matches', async () => {
    const api = await openSkills((mock) => { skillFixtures(mock);
      vi.mocked(mock.getSkillDefinition).mockResolvedValue({ skill_id: 'x1', definition_revision: 'other', package_revision: null, text: 'STALE', has_more: false }); });
    const before = vi.mocked(api.getSkillCatalog).mock.calls.length;
    fireEvent.click(await screen.findByRole('button', { name: '展開 Ext' }));
    await waitFor(() => expect(vi.mocked(api.getSkillCatalog).mock.calls.length).toBeGreaterThan(before));
    expect(screen.queryByText('STALE')).toBeNull();
  });
  it('explains a remote runtime and disables import and connecting', async () => {
    await openSkills((mock) => { vi.mocked(mock.getSkillCatalog).mockRejectedValue('Chadex bridge: Backend (skill_management_requires_local_runtime)。請查看診斷並重試。'); });
    expect(await screen.findByText(/遠端 runtime 不支援匯入或管理 Skill/)).toBeTruthy();
    expect((screen.getByRole('button', { name: '匯入 Skill（ZIP）…' }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: '選擇資料夾…' }) as HTMLButtonElement).disabled).toBe(true);
  });
  it('installs a ZIP from any location and surfaces helper errors', async () => {
    const api = await openSkills((mock) => { vi.mocked(mock.chooseSkillArchive).mockResolvedValue('skills/pack.zip');
      vi.mocked(mock.installSkill).mockRejectedValue('Chadex bridge: Backend (tool_failure)。請查看診斷並重試。'); });
    fireEvent.click(await screen.findByRole('button', { name: '匯入 Skill（ZIP）…' }));
    fireEvent.change(screen.getByLabelText('Skill key'), { target: { value: 'my-skill' } });
    fireEvent.click(screen.getByRole('button', { name: '選擇 ZIP…' }));
    await screen.findByText('skills/pack.zip');
    fireEvent.click(screen.getByRole('button', { name: '安裝並啟用' }));
    await waitFor(() => expect(api.installSkill).toHaveBeenCalledWith('C:\\work\\chadex', 'my-skill', 'skills/pack.zip'));
    expect(await screen.findByText(/tool_failure/)).toBeTruthy();
  });
});
describe('launch runtime prewarm', () => {
  const prefs = (extra: object) => ({ ...desktop().preferences, ...extra });
  it('prewarms once on start, refreshes status afterwards and shows no busy UI', async () => {
    const api = apiMock(desktop({ runtime: snapshot({ phase: 'stopped', tunnel_ready: false, chat_gpt_connected: false }) }));
    const pending = deferred<unknown>(); vi.mocked(api.prewarmRuntime).mockReturnValue(pending.promise);
    const store = new DesktopStore(api); render(<App api={api} store={store} />);
    await waitFor(() => expect(api.prewarmRuntime).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole('status')).toBeNull();
    const before = vi.mocked(api.desktopState).mock.calls.length;
    pending.resolve({}); await waitFor(() => expect(vi.mocked(api.desktopState).mock.calls.length).toBeGreaterThan(before));
    await store.refresh(); expect(api.prewarmRuntime).toHaveBeenCalledTimes(1);
    expect(api.runtimeAction).not.toHaveBeenCalled();
  });
  it('does not prewarm when the setting is off', async () => {
    const { api } = await mount(desktop({ preferences: prefs({ prepare_service_on_launch: false }) }));
    await new Promise((resolve) => setTimeout(resolve, 20)); expect(api.prewarmRuntime).not.toHaveBeenCalled();
  });
  it('treats a missing setting as on', async () => {
    const { prepare_service_on_launch: _omit, ...legacy } = desktop().preferences;
    const { api } = await mount(desktop({ preferences: legacy }));
    await waitFor(() => expect(api.prewarmRuntime).toHaveBeenCalledTimes(1));
  });
  it('does not prewarm without a project or a configured runtime', async () => {
    const none = await mount(desktop({ runtime: snapshot({ selected_project: null }) }));
    await new Promise((resolve) => setTimeout(resolve, 20)); expect(none.api.prewarmRuntime).not.toHaveBeenCalled();
    cleanup();
    const unconfigured = await mount(desktop({ runtime: snapshot({ runtime_status: { ...snapshot().runtime_status!, runtime_configured: false } }) }));
    await new Promise((resolve) => setTimeout(resolve, 20)); expect(unconfigured.api.prewarmRuntime).not.toHaveBeenCalled();
  });
  it('swallows prewarm errors without a banner', async () => {
    const api = apiMock(); vi.mocked(api.prewarmRuntime).mockRejectedValue(new Error('prewarm boom'));
    const store = new DesktopStore(api); render(<App api={api} store={store} />);
    await waitFor(() => expect(api.prewarmRuntime).toHaveBeenCalledTimes(1));
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(screen.queryByRole('alert')).toBeNull(); expect(screen.queryByText(/prewarm boom/)).toBeNull();
  });
  it('persists the prewarm toggle from settings', async () => {
    const { api } = await mount();
    fireEvent.click(screen.getByRole('button', { name: '設定' }));
    fireEvent.click(screen.getByRole('checkbox', { name: /啟動時在背景準備本機服務/ }));
    fireEvent.click(screen.getByRole('button', { name: '儲存偏好' }));
    await waitFor(() => expect(api.savePreferences).toHaveBeenCalledWith(expect.objectContaining({ prepare_service_on_launch: false })));
  });
});
