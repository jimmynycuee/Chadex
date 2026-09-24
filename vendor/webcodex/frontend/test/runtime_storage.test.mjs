import test from "node:test";
import assert from "node:assert/strict";
import {
  RUNTIME_CREDENTIAL_SESSION_KEY,
  APPEARANCE_STORAGE_KEY,
  WORKSPACE_VIEW_STORAGE_KEY,
  DRAFT_STORAGE_PREFIX,
  DEVICE_DISCLOSURE_STORAGE_PREFIX,
  appearancePreference,
  loadAppearancePreference,
  persistAppearancePreference,
  resolvedAppearance,
  workspaceViewPreference,
  loadWorkspaceViewPreference,
  persistWorkspaceViewPreference,
  loadRememberedRuntimeCredential,
  persistRuntimeCredentialForTab,
  clearRememberedRuntimeCredential,
  currentDraftStorageKey,
  loadDraft,
  saveDraft,
  clearDraft,
  clearRuntimeDrafts,
  deviceDisclosureStorageKey,
  storedDeviceDisclosure,
  persistDeviceDisclosure,
} from "../dist/runtime_storage.js";

function createMockStorage() {
  const store = new Map();
  return {
    get length() {
      return store.size;
    },
    key(index) {
      return Array.from(store.keys())[index] || null;
    },
    getItem(key) {
      return store.has(key) ? store.get(key) : null;
    },
    setItem(key, value) {
      store.set(key, String(value));
    },
    removeItem(key) {
      store.delete(key);
    },
    clear() {
      store.clear();
    },
  };
}

function withMockStorage(run) {
  const previousWindow = globalThis.window;
  const localStorage = createMockStorage();
  const sessionStorage = createMockStorage();
  try {
    globalThis.window = {
      localStorage,
      sessionStorage,
    };
    run({ localStorage, sessionStorage });
  } finally {
    globalThis.window = previousWindow;
  }
}

test("appearancePreference validates input and resolves system preference", () => {
  assert.equal(appearancePreference("light"), "light");
  assert.equal(appearancePreference("dark"), "dark");
  assert.equal(appearancePreference("system"), "system");
  assert.equal(appearancePreference("invalid"), "system");
  assert.equal(appearancePreference(null), "system");

  assert.equal(resolvedAppearance("light", false), "light");
  assert.equal(resolvedAppearance("dark", true), "dark");
  assert.equal(resolvedAppearance("system", true), "light");
  assert.equal(resolvedAppearance("system", false), "dark");
});

test("loadAppearancePreference and persistAppearancePreference interact with localStorage", () => {
  withMockStorage(({ localStorage }) => {
    assert.equal(loadAppearancePreference(), "system");

    persistAppearancePreference("dark");
    assert.equal(localStorage.getItem(APPEARANCE_STORAGE_KEY), "dark");
    assert.equal(loadAppearancePreference(), "dark");

    persistAppearancePreference("system");
    assert.equal(loadAppearancePreference(), "system");
  });
});

test("workspaceViewPreference validates input and loads from localStorage", () => {
  assert.equal(workspaceViewPreference("sessions"), "sessions");
  assert.equal(workspaceViewPreference("operations"), "operations");
  assert.equal(workspaceViewPreference("windows"), "windows");
  assert.equal(workspaceViewPreference("invalid"), "sessions");

  withMockStorage(({ localStorage }) => {
    assert.equal(loadWorkspaceViewPreference(), "sessions");
    persistWorkspaceViewPreference("operations");
    assert.equal(localStorage.getItem(WORKSPACE_VIEW_STORAGE_KEY), "operations");
    assert.equal(loadWorkspaceViewPreference(), "operations");
  });
});

test("runtime credential persistence interacts with sessionStorage correctly", () => {
  withMockStorage(({ sessionStorage }) => {
    assert.equal(loadRememberedRuntimeCredential(), "");

    persistRuntimeCredentialForTab("secret-token-123", true);
    assert.equal(sessionStorage.getItem(RUNTIME_CREDENTIAL_SESSION_KEY), "secret-token-123");
    assert.equal(loadRememberedRuntimeCredential(), "secret-token-123");

    persistRuntimeCredentialForTab("secret-token-123", false);
    assert.equal(sessionStorage.getItem(RUNTIME_CREDENTIAL_SESSION_KEY), null);
    assert.equal(loadRememberedRuntimeCredential(), "");

    persistRuntimeCredentialForTab("token-2", true);
    clearRememberedRuntimeCredential();
    assert.equal(loadRememberedRuntimeCredential(), "");
  });
});

test("draft storage keys and draft management in sessionStorage", () => {
  assert.equal(currentDraftStorageKey("", "sess-1"), "");
  assert.equal(currentDraftStorageKey("proj/a", ""), "");
  assert.equal(
    currentDraftStorageKey("proj/a", "sess/1"),
    DRAFT_STORAGE_PREFIX + "proj%2Fa.sess%2F1"
  );

  withMockStorage(({ sessionStorage }) => {
    assert.equal(loadDraft("proj-1", "sess-1"), "");

    saveDraft("proj-1", "sess-1", "draft text");
    assert.equal(loadDraft("proj-1", "sess-1"), "draft text");

    saveDraft("proj-1", "sess-2", "another draft");
    assert.equal(sessionStorage.length, 2);

    clearDraft("proj-1", "sess-1");
    assert.equal(loadDraft("proj-1", "sess-1"), "");
    assert.equal(loadDraft("proj-1", "sess-2"), "another draft");

    clearRuntimeDrafts();
    assert.equal(loadDraft("proj-1", "sess-2"), "");
    assert.equal(sessionStorage.length, 0);
  });
});

test("device disclosure persistence in localStorage", () => {
  assert.equal(
    deviceDisclosureStorageKey("runner-1"),
    DEVICE_DISCLOSURE_STORAGE_PREFIX + "runner-1"
  );

  withMockStorage(({ localStorage }) => {
    assert.equal(storedDeviceDisclosure("runner-1"), null);

    persistDeviceDisclosure("runner-1", true);
    assert.equal(localStorage.getItem(deviceDisclosureStorageKey("runner-1")), "open");
    assert.equal(storedDeviceDisclosure("runner-1"), true);

    persistDeviceDisclosure("runner-1", false);
    assert.equal(localStorage.getItem(deviceDisclosureStorageKey("runner-1")), "closed");
    assert.equal(storedDeviceDisclosure("runner-1"), false);
  });
});
