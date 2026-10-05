import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { App } from '../src/App';
import { DesktopStore } from '../src/state';
import { apiMock, deferred, desktop, snapshot } from './fixtures';

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
