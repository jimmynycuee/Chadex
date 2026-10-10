import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import { probe, waitForUi, waitForIpc, invokeInstalled } from './windows_installed_probe.mjs';

function desktop({ smoke = false, restore = false, rendered = true, version = '0.6.4', credentialError = null } = {}) {
  const calls = [];
  const state = { version, startup_error: null, credential_error: credentialError, credential_stored: false,
    helper: { state: 'running', pid: 123 }, runtime: { chat_gpt_verified_for_selected_project: false },
    preferences: { last_project: restore ? 'C:/專案 A' : null, ferret_visible: !restore, notifications: !restore },
    paths: { helper: 'installed/helper/chadex-helper.exe', runtime: 'installed/chadex-runtime', data: 'data' } };
  const context = vm.createContext({
    document: {
      getElementById: () => ({ childElementCount: rendered ? 1 : 0 }),
      querySelector: selector => selector.includes('sidebar-project') ? { getAttribute: () => state.preferences.last_project } : null,
      querySelectorAll: () => [['Helper', '執行中'], ['本地服務', '本地服務可用']].map(([label, value]) => ({ querySelector: tag => ({ textContent: tag === 'dt' ? label : value }) })),
    },
    setTimeout: fn => { fn(); },
    window: { __TAURI_INTERNALS__: { invoke: async (name, args) => {
      calls.push([name, args]);
      if (name === 'desktop_state') return structuredClone(state);
      if (name === 'save_preferences') { state.preferences = args.preferences; return; }
      if (name === 'quit_app') return;
      if (name === 'smoke_checkpoint') throw new Error(smoke ? 'smoke_not_enabled' : 'Command smoke_checkpoint not found');
      if (name === 'runtime_action') {
        if (args.method === 'inspectProject') return { path: 'C:/專案 A' };
        if (args.method === 'activateProject') {
          state.preferences.last_project = args.params.path;
          state.runtime.selected_project = { path: args.params.path };
        }
        if (args.method === 'configureLocalSetup') {
          state.runtime.runtime_status = { runtime_ready: true };
          state.runtime.selected_project = { path: 'C:/專案 A' };
        }
        return {};
      }
      throw new Error('unexpected_command');
    } } },
  });
  let closed = false;
  return { calls, closed: () => closed, connector: async () => ({
    evaluate: expression => vm.runInContext(expression, context),
    close: () => { closed = true; },
  }) };
}

test('installed production starts runtime, saves preferences and quits', async () => {
  const app = desktop();
  const result = await probe(1234, 'C:/專案 A', 'initial', '0.6.4', app.connector);
  assert.equal(result.runtime_ready, true);
  assert.equal(result.smoke_ipc_rejected, true);
  assert.equal(app.calls.at(-1)[0], 'quit_app');
  assert.equal(app.closed(), true);
});

test('a smoke build without fixtures cannot pass the production probe', async () => {
  const app = desktop({ smoke: true });
  await assert.rejects(probe(1234, 'C:/專案 A', 'initial', '0.6.4', app.connector), /production_smoke_ipc_exposed/);
  assert.equal(app.calls.some(([name]) => name === 'quit_app'), false);
  assert.equal(app.closed(), true);
});

test('restored preferences are observed before a new setup request', async () => {
  const app = desktop({ restore: true });
  const result = await probe(1234, 'C:/專案 A', 'restore', '0.6.4', app.connector);
  assert.equal(result.preferences_restored, true);
  assert.equal(app.calls.some(([name]) => name === 'save_preferences'), false);
});

test('missing restored preferences do not get silently repaired', async () => {
  const app = desktop();
  await assert.rejects(probe(1234, 'C:/專案 A', 'restore', '0.6.4', app.connector), /installed_preferences_not_restored/);
  assert.equal(app.calls.some(([, args]) => args?.method === 'configureLocalSetup'), false);
});

test('an unrendered WebView or unexpected version cannot pass', async () => {
  for (const options of [{ rendered: false }, { version: '0.4.1' }]) {
    const app = desktop(options);
    await assert.rejects(probe(1234, 'C:/專案 A', 'initial', '0.6.4', app.connector));
    assert.equal(app.closed(), true);
  }
});

test('credential storage read failure is not an empty credential store', async () => {
  const app = desktop({ credentialError: 'credential_read_failed' });
  await assert.rejects(probe(1234, 'C:/專案 A', 'initial', '0.6.4', app.connector), /installed_credential_read_failed/);
});

test('backend readiness cannot replace the rendered frontend state', async () => {
  await assert.rejects(waitForUi({ evaluate: async () => false }, 'C:/專案 A', 0), /installed_ui_state_not_ready/);
});


test('IPC availability is observed without issuing a backend command', async () => {
  const expressions = [];
  await waitForIpc({ evaluate: async expression => { expressions.push(expression); return true; } }, 0);
  assert.equal(expressions.length, 1);
  assert.equal(expressions[0].includes('invoke('), false);
  await assert.rejects(waitForIpc({ evaluate: async () => false }, 0), /installed_ipc_not_ready/);
});

test('RPC failure identifies the step while discarding raw backend text', async () => {
  for (const [message, kind] of [['project_unavailable: C:/private/key', 'project_unavailable'], ['private unknown token', 'other'], ['Command runtime_action not found', 'command_not_found']]) {
    const context = vm.createContext({ window: { __TAURI_INTERNALS__: { invoke: async () => { throw message; } } } });
    const client = { evaluate: expression => vm.runInContext(expression, context) };
    await assert.rejects(invokeInstalled(client, 'runtime_action', { method: 'inspectProject' }), error => {
      assert.equal(error.message, `installed_rpc_inspectProject:${kind}`);
      return true;
    });
  }
});

test('RPC diagnostic does not retry a rejected state-changing command', async () => {
  let calls = 0;
  const context = vm.createContext({ window: { __TAURI_INTERNALS__: { invoke: async () => { calls++; throw 'private failure'; } } } });
  await assert.rejects(invokeInstalled({ evaluate: expression => vm.runInContext(expression, context) }, 'save_preferences'), /installed_rpc_save_preferences:other/);
  assert.equal(calls, 1);
});
