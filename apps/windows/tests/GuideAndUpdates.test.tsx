import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { App } from '../src/App';
import { DesktopStore } from '../src/state';
import { defaultPreferences } from '../src/contracts';
import { AUTO_CHECK_DELAY_MS, AUTO_RECHECK_INTERVAL_MS } from '../src/updates';
import { apiMock, deferred, desktop, snapshot } from './fixtures';

afterEach(() => { cleanup(); vi.useRealTimers(); localStorage.clear(); sessionStorage.clear(); });
async function mount(state = desktop(), api = apiMock(state)) {
  const store = new DesktopStore(api);
  render(<App api={api} store={store} />);
  await waitFor(() => expect(store.getSnapshot().freshness).toBe('fresh'));
  return { api, store };
}

describe('guide page', () => {
  it('follows the macOS steps and opens its pages by fixed key', async () => {
    const { api } = await mount();
    fireEvent.click(screen.getByRole('button', { name: '使用說明' }));
    expect(screen.getByRole('heading', { level: 1, name: '使用說明' })).toBeTruthy();
    for (const title of ['先在 OpenAI Platform 準備', '回到 Chadex', '設定 ChatGPT 網頁版', '最後測試', '遇到問題？']) {
      expect(screen.getByRole('heading', { level: 2, name: title })).toBeTruthy();
    }
    fireEvent.click(screen.getByRole('button', { name: '開啟 OpenAI Projects' }));
    fireEvent.click(screen.getByRole('button', { name: 'OpenAI MCP 官方說明' }));
    expect(api.openGuideLink).toHaveBeenNthCalledWith(1, 'openai_projects');
    expect(api.openGuideLink).toHaveBeenNthCalledWith(2, 'mcp_guide');
    // The fixture has a project, stored credentials and a ready tunnel.
    expect(screen.getByText('已連線，可以到 ChatGPT 使用')).toBeTruthy();
    expect(screen.getByText(/點系統匣的 Chadex 圖示/)).toBeTruthy();
  });

  it('remembers the manual confirmations', async () => {
    await mount();
    fireEvent.click(screen.getByRole('button', { name: '使用說明' }));
    fireEvent.click(screen.getByRole('checkbox', { name: '我已建立 Chadex Project' }));
    expect(localStorage.getItem('chadex.guide.openAIProjectConfirmed')).toBe('true');
  });

  it('opens on the guide once for a fresh install', async () => {
    const fresh = desktop({ credential_stored: false, preferences: { ...defaultPreferences } });
    await mount(fresh);
    expect(screen.getByRole('heading', { level: 1, name: '使用說明' })).toBeTruthy();
    cleanup();
    await mount(fresh);
    expect(screen.getByRole('heading', { level: 1, name: '總覽' })).toBeTruthy();
  });

  it('does not reroute an install that is already set up', async () => {
    await mount();
    expect(screen.getByRole('heading', { level: 1, name: '總覽' })).toBeTruthy();
  });
});

describe('automatic update check and sidebar button', () => {
  it('checks shortly after launch and shows a sidebar button that keeps its color while installing', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const api = apiMock(desktop({ version: '0.7.2' }));
    vi.mocked(api.checkForUpdate).mockResolvedValue({ version: '0.7.3' });
    const pending = deferred<void>();
    vi.mocked(api.installUpdate).mockImplementationOnce(async (onProgress) => { onProgress?.(50, 100); await pending.promise; });
    await mount(undefined, api);
    expect(api.checkForUpdate).not.toHaveBeenCalled();
    await act(async () => { vi.advanceTimersByTime(AUTO_CHECK_DELAY_MS); });
    const button = await screen.findByRole('button', { name: /更新至 v0\.7\.3/ });
    expect(api.installUpdate).not.toHaveBeenCalled();
    fireEvent.click(button);
    expect(await screen.findByRole('button', { name: /更新中… 50%/ })).toBeTruthy();
    const working = screen.getByRole('button', { name: /更新中…/ }) as HTMLButtonElement;
    expect(working.disabled).toBe(false);
    expect(working.getAttribute('aria-busy')).toBe('true');
    fireEvent.click(working);
    expect(api.installUpdate).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('heading', { level: 1, name: '版本與更新' })).toBeTruthy();
    await act(async () => { pending.resolve(undefined); });
    expect(await screen.findByRole('button', { name: /重新啟動中…/ })).toBeTruthy();
  });

  it('rechecks on focus only after six hours and stays quiet when a check fails', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const api = apiMock(desktop({ version: '0.7.2' }));
    vi.mocked(api.checkForUpdate).mockRejectedValueOnce(new Error('offline')).mockResolvedValue(null);
    await mount(undefined, api);
    await act(async () => { vi.advanceTimersByTime(AUTO_CHECK_DELAY_MS); });
    await waitFor(() => expect(api.checkForUpdate).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByRole('button', { name: /Windows Desktop/ }));
    expect(screen.queryByText('無法檢查更新')).toBeNull();
    await act(async () => { window.dispatchEvent(new Event('focus')); });
    expect(api.checkForUpdate).toHaveBeenCalledTimes(1);
    await act(async () => { vi.setSystemTime(Date.now() + AUTO_RECHECK_INTERVAL_MS); window.dispatchEvent(new Event('focus')); });
    await waitFor(() => expect(api.checkForUpdate).toHaveBeenCalledTimes(2));
  });

  it('can be turned off', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    localStorage.setItem('chadex.updates.autoCheck', 'false');
    const { api } = await mount(desktop({ version: '0.7.2' }));
    await act(async () => { vi.advanceTimersByTime(AUTO_CHECK_DELAY_MS * 2); });
    expect(api.checkForUpdate).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: /Windows Desktop/ }));
    expect((screen.getByRole('checkbox', { name: /自動檢查更新/ }) as HTMLInputElement).checked).toBe(false);
  });

  it('does not install while work is running', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const state = desktop({ version: '0.7.2', runtime: snapshot({ current_operation: { id: 'op', kind: 'connect', phase: 'running', started_at_ms: 0, cancellable: false } }) });
    const api = apiMock(state);
    vi.mocked(api.checkForUpdate).mockResolvedValue({ version: '0.7.3' });
    await mount(state, api);
    await act(async () => { vi.advanceTimersByTime(AUTO_CHECK_DELAY_MS); });
    fireEvent.click(await screen.findByRole('button', { name: /更新至 v0\.7\.3/ }));
    expect(api.installUpdate).not.toHaveBeenCalled();
    expect(screen.getByText(/目前有工作正在執行/)).toBeTruthy();
    expect((screen.getByRole('button', { name: '下載並安裝 v0.7.3' }) as HTMLButtonElement).disabled).toBe(true);
  });
});
