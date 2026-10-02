import { afterEach, describe, expect, it, vi } from 'vitest';
import { connectionStatus, DesktopStore, localStatus, MAX_SNAPSHOT_AGE_MS, observedRuntime } from '../src/state';
import { apiMock, deferred, desktop, snapshot } from './fixtures';
import type { DesktopState } from '../src/contracts';

afterEach(() => vi.useRealTimers());
describe('snapshot authority and freshness', () => {
  it('separates local readiness from tunnel and ChatGPT project verification', () => {
    const runtime = snapshot();
    expect(localStatus(runtime).ready).toBe(true);
    expect(connectionStatus(runtime).verified).toBe(false);
    expect(connectionStatus(runtime).label).toContain('等待 ChatGPT 驗證');
    expect(localStatus(snapshot({ runtime_status: undefined, phase: 'verified' })).known).toBe(false);
    expect(connectionStatus(snapshot({ phase: 'verified', chat_gpt_verified_for_selected_project: true })).verified).toBe(true);
    for (const override of [{ tunnel_ready: false }, { chat_gpt_connected: false }, { runtime_status: undefined }, { selected_project: null }, { phase: 'stopped' as const }, { error: { code: 'bad', message: 'failure' } }]) {
      expect(connectionStatus(snapshot({ phase: 'verified', chat_gpt_verified_for_selected_project: true, ...override })).verified).toBe(false);
    }
  });
  it('withdraws verification when runtime dies while tunnel and project verification remain true', () => {
    const ready = snapshot().runtime_status!;
    for (const runtime_status of [{ ...ready, runtime_ready: false }, { ...ready, needs_attention: true }]) {
      const runtime = snapshot({ phase: 'verified', chat_gpt_connected: true, tunnel_ready: true,
        chat_gpt_verified_for_selected_project: true, runtime_status });
      expect(connectionStatus(runtime).verified).toBe(false);
      expect(connectionStatus(runtime).label).toContain('本地服務'); expect(localStatus(runtime).ready).toBe(false);
    }
  });
  it('clears all usable runtime facts immediately when refresh fails', async () => {
    const api = apiMock(desktop({ runtime: snapshot({ phase: 'verified', chat_gpt_verified_for_selected_project: true }) }));
    const store = new DesktopStore(api, () => 100);
    await store.refresh(); expect(observedRuntime(store.getSnapshot(), 100)?.phase).toBe('verified');
    vi.mocked(api.desktopState).mockRejectedValueOnce(new Error('helper died'));
    await store.refresh(); expect(store.getSnapshot().desktop?.runtime).toBeNull();
    expect(store.getSnapshot().freshness).toBe('unavailable'); expect(observedRuntime(store.getSnapshot(), 100)).toBeNull();
    await store.refresh(); expect(store.getSnapshot().freshness).toBe('fresh');
  });
  it.each(['stopping', 'stopped', 'failed'] as const)('ignores retained ready snapshot when helper is %s', async (state) => {
    const store = new DesktopStore(apiMock(desktop({ helper: { state, pid: null, error: 'exit' } })), () => 0);
    await store.refresh(); expect(store.getSnapshot().desktop?.runtime).toBeNull();
    expect(localStatus(observedRuntime(store.getSnapshot(), 0)).ready).toBe(false);
  });
  it('keeps exactly one desktop_state request in flight across refresh calls', async () => {
    const pending = deferred<DesktopState>(); const api = apiMock();
    vi.mocked(api.desktopState).mockReturnValue(pending.promise);
    const store = new DesktopStore(api, () => 0);
    const requests = [store.refresh(), store.refresh(), store.refresh()];
    expect(api.desktopState).toHaveBeenCalledTimes(1);
    pending.resolve(desktop()); await Promise.all(requests);
    await store.refresh(); expect(api.desktopState).toHaveBeenCalledTimes(2);
  });
  it('expires a ready screen while a new IPC remains pending, without overlapping calls', async () => {
    let now = 0; const api = apiMock(); const store = new DesktopStore(api, () => now);
    await store.refresh(); const pending = deferred<DesktopState>(); vi.mocked(api.desktopState).mockReturnValueOnce(pending.promise);
    const refresh = store.refresh(); now = MAX_SNAPSHOT_AGE_MS; store.checkAge();
    expect(observedRuntime(store.getSnapshot(), now)).toBeNull();
    void store.refresh(); expect(api.desktopState).toHaveBeenCalledTimes(2);
    pending.resolve(desktop()); await refresh;
    expect(store.getSnapshot().freshness).toBe('unavailable');
    await store.refresh(); expect(store.getSnapshot().freshness).toBe('fresh');
  });
  it('rejects a pre-action ready response and only applies a post-action poll', async () => {
    const pending = deferred<DesktopState>(); const api = apiMock();
    vi.mocked(api.desktopState).mockReturnValueOnce(pending.promise);
    const store = new DesktopStore(api, () => 0); const old = store.refresh();
    store.beginAction(); pending.resolve(desktop({ runtime: snapshot({ phase: 'verified', chat_gpt_verified_for_selected_project: true }) }));
    await old; expect(observedRuntime(store.getSnapshot(), 0)).toBeNull();
    await store.refresh(); expect(api.desktopState).toHaveBeenCalledTimes(1);
    store.endAction(); await store.refreshAfterAction(); expect(api.desktopState).toHaveBeenCalledTimes(2);
    expect(connectionStatus(observedRuntime(store.getSnapshot(), 0)).verified).toBe(false);
  });
  it('rejects decreasing revisions within a helper session but accepts a restarted helper', async () => {
    const api = apiMock(desktop({ runtime: snapshot({ state_revision: 10 }) })); const store = new DesktopStore(api, () => 0);
    await store.refresh(); vi.mocked(api.desktopState).mockResolvedValueOnce(desktop({ runtime: snapshot({ state_revision: 2 }) }));
    await store.refresh(); expect(store.getSnapshot().freshness).toBe('unavailable');
    vi.mocked(api.desktopState).mockResolvedValueOnce(desktop({ helper: { state: 'running', pid: 101, error: null }, runtime: snapshot({ state_revision: 1 }) }));
    await store.refresh(); expect(store.getSnapshot().freshness).toBe('fresh');
  });
  it('fails closed on invalid contract instead of keeping a ready screen', async () => {
    const api = apiMock(); const store = new DesktopStore(api, () => 0); await store.refresh();
    vi.mocked(api.desktopState).mockResolvedValueOnce({ ...desktop(), runtime: { phase: 'verified' } } as DesktopState);
    await store.refresh(); expect(store.getSnapshot().freshness).toBe('unavailable');
  });
  it('retains a single polling loop after StrictMode mount, unmount, remount', async () => {
    vi.useFakeTimers(); const pending = deferred<DesktopState>(); const api = apiMock();
    vi.mocked(api.desktopState).mockReturnValueOnce(pending.promise);
    const store = new DesktopStore(api); const stop = store.retain(); stop(); const stopAgain = store.retain();
    expect(api.desktopState).toHaveBeenCalledTimes(1); pending.resolve(desktop());
    await vi.advanceTimersByTimeAsync(1_500); expect(api.desktopState).toHaveBeenCalledTimes(2);
    stopAgain(); await vi.advanceTimersByTimeAsync(10_000); expect(api.desktopState).toHaveBeenCalledTimes(2);
  });
});
