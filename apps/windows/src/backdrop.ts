import { isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import type { Preferences } from './contracts';

interface UADataValues { platform?: string; platformVersion?: string }
interface UAData { getHighEntropyValues?(hints: string[]): Promise<UADataValues> }

/**
 * The window asks for Mica in tauri.conf.json, but only Windows 11 draws it.
 * WebView2 reports Windows 11 as platformVersion 13 or later (Microsoft's
 * documented check); anywhere else the page keeps its opaque background so a
 * Windows 10 window never turns see-through.
 */
export async function micaAvailable(): Promise<boolean> {
  if (!isTauri()) return false;
  const data = (navigator as Navigator & { userAgentData?: UAData }).userAgentData;
  if (!data?.getHighEntropyValues) return false;
  try {
    const { platform, platformVersion } = await data.getHighEntropyValues(['platformVersion']);
    return platform === 'Windows' && Number.parseInt(platformVersion ?? '0', 10) >= 13;
  } catch { return false; }
}

/** Mica takes its tint from the window theme, so it follows the in-app appearance choice. */
export function syncWindowTheme(theme: Preferences['theme']) {
  if (!isTauri()) return;
  void getCurrentWindow().setTheme(theme === 'system' ? null : theme).catch(() => undefined);
}
