import { useCallback, useEffect, useRef, useState } from 'react';
import type { DesktopApi, UpdateInfo } from './api';
import { safeError } from './state';

export type UpdateState =
  | { kind: 'idle' }
  | { kind: 'checking' }
  | { kind: 'current' }
  | { kind: 'available'; update: UpdateInfo }
  | { kind: 'installing'; downloaded: number; total?: number }
  | { kind: 'handoff' }
  | { kind: 'error'; operation: 'check' | 'install'; message: string };

/** Same cadence as the macOS app: once shortly after launch, then again on focus after six hours. */
export const AUTO_CHECK_DELAY_MS = 5_000;
export const AUTO_RECHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

export function updateErrorMessage(operation: 'check' | 'install', error: unknown) {
  const detail = safeError(error);
  if (/需要.*桌面環境/.test(detail)) return '請在 Chadex Windows 桌面應用程式中使用更新功能。';
  if (/plugin:updater|updater.*(?:not found|not registered|not allowed)/i.test(detail)) {
    return '目前安裝版本尚未啟用 App 內更新功能。請先手動安裝含更新功能的最新版 Windows 安裝檔。';
  }
  return operation === 'check'
    ? `檢查更新失敗：${detail}。請確認網路連線後重試。`
    : `安裝未完成：${detail}。請重新檢查更新後再試。`;
}

export interface WindowsUpdates {
  state: UpdateState;
  autoCheck: boolean;
  setAutoCheck(value: boolean): void;
  /** A silent check (the automatic one) never replaces what is shown with an error. */
  check(silent?: boolean): Promise<void>;
  install(): Promise<void>;
}

/** `autoCheck` comes from the app's preferences; `saveAutoCheck` stores a change there. */
export function useWindowsUpdates(api: DesktopApi, version: string | null, autoCheck: boolean,
  saveAutoCheck: (value: boolean) => void): WindowsUpdates {
  const [state, setState] = useState<UpdateState>({ kind: 'idle' });
  const stateRef = useRef(state);
  stateRef.current = state;
  const mounted = useRef(true);
  const operation = useRef(false);
  const lastCheckAt = useRef<number | null>(null);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);

  const check = useCallback(async (silent = false) => {
    if (operation.current) return;
    const before = stateRef.current;
    // An automatic check must not disturb an update the user is about to install, or one in progress.
    if (silent && (before.kind === 'available' || before.kind === 'installing' || before.kind === 'handoff')) return;
    operation.current = true;
    lastCheckAt.current = Date.now();
    setState({ kind: 'checking' });
    try {
      const update = await api.checkForUpdate();
      if (mounted.current) setState(update ? { kind: 'available', update } : { kind: 'current' });
    } catch (error) {
      if (mounted.current) setState(silent ? before : { kind: 'error', operation: 'check', message: updateErrorMessage('check', error) });
    } finally { operation.current = false; }
  }, [api]);

  const install = useCallback(async () => {
    if (operation.current || stateRef.current.kind !== 'available') return;
    operation.current = true;
    setState({ kind: 'installing', downloaded: 0 });
    try {
      await api.installUpdate((downloaded, total) => {
        if (!mounted.current) return;
        const validDownloaded = Number.isFinite(downloaded) ? Math.max(0, Math.min(Number.MAX_SAFE_INTEGER, Math.floor(downloaded))) : 0;
        const validTotal = typeof total === 'number' && Number.isFinite(total) && total > 0
          ? Math.floor(Math.min(Number.MAX_SAFE_INTEGER, total)) : undefined;
        setState((current) => {
          if (current.kind !== 'installing') return current;
          const nextTotal = validTotal ?? current.total;
          const boundedDownloaded = Math.max(current.downloaded, validDownloaded);
          return { kind: 'installing', downloaded: nextTotal === undefined ? boundedDownloaded : Math.min(boundedDownloaded, nextTotal), total: nextTotal };
        });
      });
      if (mounted.current) setState({ kind: 'handoff' });
    } catch (error) {
      if (mounted.current) setState({ kind: 'error', operation: 'install', message: updateErrorMessage('install', error) });
    } finally { operation.current = false; }
  }, [api]);

  useEffect(() => {
    if (!autoCheck || !version) return;
    const timer = window.setTimeout(() => { void check(true); }, AUTO_CHECK_DELAY_MS);
    const recheck = () => {
      if (document.visibilityState === 'hidden') return;
      if (lastCheckAt.current !== null && Date.now() - lastCheckAt.current >= AUTO_RECHECK_INTERVAL_MS) void check(true);
    };
    window.addEventListener('focus', recheck);
    document.addEventListener('visibilitychange', recheck);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener('focus', recheck);
      document.removeEventListener('visibilitychange', recheck);
    };
  }, [autoCheck, version, check]);

  return { state, autoCheck, setAutoCheck: saveAutoCheck, check, install };
}
