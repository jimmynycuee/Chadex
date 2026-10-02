import type { DesktopApi } from './api';
import type { DesktopState, BackendSnapshot } from './contracts';

export const MAX_SNAPSHOT_AGE_MS = 8_000;
export interface ViewState {
  desktop: DesktopState | null;
  freshness: 'loading' | 'fresh' | 'unavailable';
  error: string | null;
  receivedAt: number | null;
}
export function safeError(error: unknown): string {
  const message = typeof error === 'string' ? error : error instanceof Error ? error.message
    : error && typeof error === 'object' && 'message' in error ? String(error.message) : '操作失敗，請查看診斷資訊。';
  return message.replace(/bearer\s+[^\s"',;}\]]+/gi, '[redacted]')
    .replace(/(?:sk-|wc_pair_|wc_pat_|wc_agent_|webcodex_temporary_)[\w-]+/g, '[redacted]').slice(0, 1024);
}
function validSnapshot(snapshot: BackendSnapshot): boolean {
  return ['unconfigured', 'preparing', 'waiting_for_chatgpt_verification', 'verified', 'stopped', 'error'].includes(snapshot.phase)
    && Number.isSafeInteger(snapshot.state_revision) && snapshot.state_revision >= 0
    && ['tunnel_ready', 'chat_gpt_connected', 'chat_gpt_verified_for_selected_project'].every((key) => typeof snapshot[key as keyof BackendSnapshot] === 'boolean')
    && (!snapshot.selected_project || typeof snapshot.selected_project.path === 'string')
    && (!snapshot.runtime_status || typeof snapshot.runtime_status.runtime_ready === 'boolean');
}
export function validateDesktop(state: DesktopState): DesktopState {
  if (!state || !state.helper || !['running', 'stopping', 'stopped', 'failed'].includes(state.helper.state)
    || !state.preferences || !Array.isArray(state.preferences.recent_projects)
    || !state.paths || !Array.isArray(state.activity) || typeof state.credential_stored !== 'boolean'
    || typeof state.version !== 'string' || (state.runtime && !validSnapshot(state.runtime))) {
    throw new Error('桌面狀態格式不相容，請確認 app 與 helper 版本一致。');
  }
  // Even a accidentally retained backend snapshot cannot survive helper death.
  return state.helper.state === 'running' ? state : { ...state, runtime: null, activity: [], traces: [] };
}

// All desktop_state calls share this store, including manual refresh and React
// StrictMode mounts. Never release the slot on timeout: the IPC may still be live.
export class DesktopStore {
  private view: ViewState = { desktop: null, freshness: 'loading', error: null, receivedAt: null };
  private listeners = new Set<() => void>();
  private inFlight: Promise<void> | null = null;
  private epoch = 0;
  private actionPending = false;
  private loop = 0;
  private users = 0;
  private pollTimer: ReturnType<typeof setTimeout> | undefined;
  private ageTimer: ReturnType<typeof setInterval> | undefined;
  private revision: { pid: number | null; value: number } | null = null;

  constructor(private api: Pick<DesktopApi, 'desktopState'>, private now: () => number = Date.now) {}
  getSnapshot = (): ViewState => this.view;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private publish(view: ViewState) { this.view = view; this.listeners.forEach((fn) => fn()); }

  invalidate(message: string | null = null, freshness: ViewState['freshness'] = 'loading') {
    this.epoch += 1;
    this.publish({ ...this.view, desktop: this.view.desktop ? { ...this.view.desktop, runtime: null, activity: [], traces: [] } : null,
      freshness, error: message, receivedAt: null });
  }
  checkAge = () => {
    if (this.view.freshness === 'fresh' && this.view.receivedAt !== null
      && (this.now() - this.view.receivedAt >= MAX_SNAPSHOT_AGE_MS || this.now() < this.view.receivedAt)) {
      this.invalidate('狀態已過期，等待重新取得 helper 狀態。', 'unavailable');
    }
  };
  beginAction() { this.actionPending = true; this.invalidate(); }
  endAction() { this.actionPending = false; }
  refresh = (): Promise<void> => {
    if (this.inFlight) return this.inFlight;
    if (this.actionPending) return Promise.resolve();
    const epoch = this.epoch;
    this.inFlight = (async () => {
      try {
        const state = validateDesktop(await this.api.desktopState());
        if (epoch !== this.epoch) return;
        if (state.helper.state === 'running' && state.runtime) {
          if (this.revision?.pid === state.helper.pid && state.runtime.state_revision < this.revision.value) {
            throw new Error('收到較舊的 helper 狀態，等待最新快照。');
          }
          this.revision = { pid: state.helper.pid, value: state.runtime.state_revision };
        } else { this.revision = null; }
        this.publish({ desktop: state, freshness: 'fresh', receivedAt: this.now(), error: null });
      } catch (error) {
        if (epoch === this.epoch) this.invalidate(safeError(error), 'unavailable');
      } finally { this.inFlight = null; }
    })();
    return this.inFlight;
  };
  async refreshAfterAction() {
    // A poll that began before the action is fenced off by invalidate(). Wait
    // for its slot, then request a fresh desktop snapshot after action completion.
    if (this.inFlight) await this.inFlight;
    await this.refresh();
  }
  retain = () => {
    this.users += 1;
    if (this.users === 1) {
      const loop = ++this.loop;
      const poll = async () => {
        await this.refresh();
        if (this.users && this.loop === loop) this.pollTimer = setTimeout(poll, 1_500);
      };
      void poll();
      this.ageTimer = setInterval(this.checkAge, 500);
    }
    return () => {
      this.users -= 1;
      if (!this.users) { this.loop += 1; clearTimeout(this.pollTimer); clearInterval(this.ageTimer); }
    };
  };
}

export function observedRuntime(view: ViewState, now = Date.now()): BackendSnapshot | null {
  if (view.freshness !== 'fresh' || view.receivedAt === null || now < view.receivedAt
    || now - view.receivedAt >= MAX_SNAPSHOT_AGE_MS || view.desktop?.helper.state !== 'running') return null;
  return view.desktop.runtime;
}
export function connectionStatus(snapshot: BackendSnapshot | null): { label: string; tone: 'good' | 'quiet' | 'warn'; verified: boolean } {
  if (!snapshot) return { label: '狀態未確認', tone: 'quiet', verified: false };
  if (snapshot.error || snapshot.phase === 'error') return { label: '連線需要處理', tone: 'warn', verified: false };
  if (!snapshot.runtime_status) return { label: '本地服務狀態未確認', tone: 'quiet', verified: false };
  if (snapshot.runtime_status.needs_attention) return { label: '本地服務需要處理', tone: 'warn', verified: false };
  if (!snapshot.runtime_status.runtime_ready) return { label: '本地服務尚未可用', tone: 'quiet', verified: false };
  const verified = snapshot.phase === 'verified' && Boolean(snapshot.selected_project)
    && snapshot.tunnel_ready && snapshot.chat_gpt_connected && snapshot.chat_gpt_verified_for_selected_project;
  if (verified) return { label: '專案已通過 ChatGPT 驗證', tone: 'good', verified: true };
  if (snapshot.tunnel_ready) return { label: 'Tunnel 可用 · 等待 ChatGPT 驗證', tone: 'quiet', verified: false };
  if (snapshot.phase === 'preparing') return { label: '正在準備連線', tone: 'quiet', verified: false };
  return { label: snapshot.phase === 'stopped' ? '連線已停止' : '尚未連線', tone: 'quiet', verified: false };
}
export function localStatus(snapshot: BackendSnapshot | null) {
  const status = snapshot?.runtime_status;
  if (!status) return { label: '尚無本地服務狀態', ready: false, known: false };
  return { label: status.needs_attention ? '本地服務需要處理' : status.runtime_ready ? '本地服務可用' : '本地服務尚未可用',
    ready: status.runtime_ready && !status.needs_attention, known: true };
}
