// CI-only WebView2 CDP driver. No test IPC or debug switch enters the app.
import { pathToFileURL } from 'node:url';
import { resolve } from 'node:path';

export const sleep = ms => new Promise(done => setTimeout(done, ms));
export function check(value, code) { if (!value) throw new Error(code); }
export const rpcSteps = ['desktop_state', 'inspectProject', 'activateProject', 'save_preferences', 'configureLocalSetup'];
export const rpcReasons = ['type_error', 'command_not_found', 'permission_denied', 'project_invalid_path', 'project_unavailable', 'runtime_start_failed', 'runtime_not_found', 'helper_unavailable', 'other'];
const safeErrors = new Set([
  'installed_webview_missing', 'cdp_not_loopback', 'cdp_connect_timeout',
  'cdp_connect_failed', 'cdp_request_failed', 'cdp_closed', 'cdp_evaluate_timeout',
  'probe_mode_invalid', 'installed_rpc_failed', 'installed_helper_unavailable',
  'installed_version_or_startup_error', 'unexpected_installed_credential',
  'installed_ui_not_rendered', 'installed_project_inspection_failed',
  'installed_preferences_not_restored', 'installed_runtime_not_ready',
  'installed_selection_invalid', 'production_smoke_ipc_exposed',
  'installed_credential_read_failed', 'installed_ui_state_not_ready', 'installed_ipc_not_ready',
  ...rpcSteps.flatMap(step => rpcReasons.map(reason => `installed_rpc_${step}:${reason}`)),
]);

export async function waitForUi(client, project, timeout = 30000) {
  const deadline = Date.now() + timeout;
  do {
    const ready = await client.evaluate(`(() => {
      const rows = [...document.querySelectorAll('#main .facts > div')];
      const value = label => rows.find(row => row.querySelector('dt')?.textContent === label)?.querySelector('dd')?.textContent;
      return !document.querySelector('#main [role="alert"]')
        && document.querySelector('.sidebar-project span[title]')?.getAttribute('title') === ${JSON.stringify(project)}
        && value('Helper') === '執行中' && value('本地服務') === '本地服務可用';
    })()`);
    if (ready === true) return;
    if (Date.now() >= deadline) break;
    await sleep(200);
  } while (Date.now() <= deadline);
  throw new Error('installed_ui_state_not_ready');
}

export async function waitForIpc(client, timeout = 90000) {
  const deadline = Date.now() + timeout;
  do {
    if (await client.evaluate("typeof window.__TAURI_INTERNALS__?.invoke === 'function'") === true) return;
    if (Date.now() >= deadline) break;
    await sleep(200);
  } while (Date.now() <= deadline);
  throw new Error('installed_ipc_not_ready');
}

// Classify inside the page; raw backend errors can contain private paths.
export async function invokeInstalled(client, name, args = {}) {
  const step = name === 'runtime_action' ? args.method : name;
  check(rpcSteps.includes(step), 'installed_rpc_failed');
  const result = await client.evaluate(`(async () => {
    try { return { ok: true, value: await window.__TAURI_INTERNALS__.invoke(${JSON.stringify(name)},${JSON.stringify(args)}) }; }
    catch (error) {
      const text = String(error);
      let reason = 'other';
      if (error instanceof TypeError) reason = 'type_error';
      else if (text.includes('not found') && text.includes(${JSON.stringify(name)})) reason = 'command_not_found';
      else if (text.includes('not allowed') || text.includes('permission denied')) reason = 'permission_denied';
      else reason = ${JSON.stringify(rpcReasons.slice(3, -1))}.find(code => text.includes(code)) ?? 'other';
      return { ok: false, reason };
    }
  })()`);
  if (result?.ok !== true) {
    const reason = rpcReasons.includes(result?.reason) ? result.reason : 'other';
    throw new Error(`installed_rpc_${step}:${reason}`);
  }
  return result.value;
}

export async function connect(port) {
  const deadline = Date.now() + 90000;
  let page;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(2000) });
      const pages = (await response.json()).filter(p => p.type === 'page' && /^(https?:\/\/tauri\.localhost|tauri:\/\/localhost)/.test(p.url));
      if (pages.length === 1) { page = pages[0]; break; }
    } catch {}
    await sleep(200);
  }
  check(page, 'installed_webview_missing');
  const address = new URL(page.webSocketDebuggerUrl);
  check(address.protocol === 'ws:' && ['localhost', '127.0.0.1'].includes(address.hostname) && address.port === String(port), 'cdp_not_loopback');
  const socket = new WebSocket(address);
  await new Promise((done, reject) => {
    const timeout = setTimeout(() => reject(new Error('cdp_connect_timeout')), 10000);
    socket.addEventListener('open', () => { clearTimeout(timeout); done(); }, { once: true });
    socket.addEventListener('error', () => { clearTimeout(timeout); reject(new Error('cdp_connect_failed')); }, { once: true });
  });
  let sequence = 0;
  const pending = new Map();
  socket.addEventListener('message', event => {
    const message = JSON.parse(event.data);
    const slot = pending.get(message.id);
    if (!slot) return;
    pending.delete(message.id); clearTimeout(slot.timeout);
    if (message.error) slot.reject(new Error('cdp_request_failed')); else slot.done(message.result);
  });
  socket.addEventListener('close', () => {
    for (const slot of pending.values()) { clearTimeout(slot.timeout); slot.reject(new Error('cdp_closed')); }
    pending.clear();
  });
  return {
    close: () => socket.close(),
    evaluate: async expression => {
      const id = ++sequence;
      const result = await new Promise((done, reject) => {
        const timeout = setTimeout(() => { pending.delete(id); reject(new Error('cdp_evaluate_timeout')); }, 90000);
        pending.set(id, { done, reject, timeout });
        socket.send(JSON.stringify({ id, method: 'Runtime.evaluate', params: { expression, awaitPromise: true, returnByValue: true } }));
      });
      check(!result.exceptionDetails && result.result?.subtype !== 'error', 'installed_rpc_failed');
      return result.result?.value;
    },
  };
}

export async function probe(port, project, mode, version, connector = connect) {
  check(['initial', 'restore'].includes(mode), 'probe_mode_invalid');
  const client = await connector(port);
  try {
    await waitForIpc(client);
    const invoke = (name, args = {}) => invokeInstalled(client, name, args);
    let state;
    const deadline = Date.now() + 90000;
    do {
      state = await invoke('desktop_state');
      if (state?.helper?.state === 'running' && state.runtime) break;
      await sleep(200);
    } while (Date.now() < deadline);
    check(state?.helper?.state === 'running' && state.runtime, 'installed_helper_unavailable');
    check(state.version === version && state.startup_error === null, 'installed_version_or_startup_error');
    check(state.credential_error === null, 'installed_credential_read_failed');
    check(state.credential_stored === false && !state.runtime.chat_gpt_verified_for_selected_project, 'unexpected_installed_credential');
    check(await client.evaluate(`document.getElementById('root')?.childElementCount > 0`), 'installed_ui_not_rendered');
    const action = (method, params = {}) => invoke('runtime_action', { method, params });
    const inspected = await action('inspectProject', { path: project });
    check(typeof inspected.path === 'string', 'installed_project_inspection_failed');
    if (mode === 'initial') {
      await action('activateProject', { path: inspected.path });
      state = await invoke('desktop_state');
      await invoke('save_preferences', { preferences: { ...state.preferences, ferret_visible: false, notifications: false } });
    } else {
      check(state.preferences.last_project === inspected.path && state.preferences.ferret_visible === false && state.preferences.notifications === false, 'installed_preferences_not_restored');
    }
    await action('configureLocalSetup');
    const readyDeadline = Date.now() + 90000;
    do {
      state = await invoke('desktop_state');
      if (state.runtime?.runtime_status?.runtime_ready === true) break;
      await sleep(200);
    } while (Date.now() < readyDeadline);
    check(state.runtime?.runtime_status?.runtime_ready === true, 'installed_runtime_not_ready');
    check(state.runtime?.selected_project?.path === inspected.path && !state.runtime.chat_gpt_verified_for_selected_project, 'installed_selection_invalid');
    await waitForUi(client, inspected.path);
    const smokeRejected = await client.evaluate(`(async () => { try { await window.__TAURI_INTERNALS__.invoke('smoke_checkpoint', {stage:'webview_ready',observation:{}}); return false; } catch (error) { const text=String(error); return text.includes('smoke_checkpoint') && text.includes('not found'); } })()`);
    check(smokeRejected === true, 'production_smoke_ipc_exposed');
    // Paths and PID are returned only to the local wrapper for exact validation;
    // they are never included in its public report.
    const result = { paths: state.paths, helper_pid: state.helper.pid, version: state.version,
      rendered: true, ui_state_ready: true, runtime_ready: true, preferences_restored: mode === 'restore', smoke_ipc_rejected: true };
    await client.evaluate(`setTimeout(() => window.__TAURI_INTERNALS__.invoke('quit_app'), 150); true`);
    return result;
  } finally { client.close(); }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const [, , port, project, mode, version] = process.argv;
    const result = await probe(Number(port), project, mode, version);
    process.stdout.write(JSON.stringify(result));
  } catch (error) {
    const code = safeErrors.has(error?.message) ? error.message : 'installed_probe_failed';
    process.stderr.write(`${code}\n`);
    process.exitCode = 1;
  }
}
