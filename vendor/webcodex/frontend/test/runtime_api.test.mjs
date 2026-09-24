import test from "node:test";
import assert from "node:assert/strict";
import {
  isAbortError,
  abortController,
  writeClipboardText,
  RuntimeApiClient,
  RUNTIME_API_BASE,
} from "../dist/runtime_api.js";

test("isAbortError recognizes AbortError instances and objects", () => {
  assert.equal(isAbortError(new DOMException("The operation was aborted.", "AbortError")), true);
  assert.equal(isAbortError({ name: "AbortError" }), true);
  assert.equal(isAbortError(new Error("regular error")), false);
  assert.equal(isAbortError(null), false);
  assert.equal(isAbortError(undefined), false);
});

test("abortController aborts valid controller safely and ignores null", () => {
  let aborted = false;
  const mockController = {
    abort() { aborted = true; },
  };
  abortController(mockController);
  assert.equal(aborted, true);
  assert.doesNotThrow(() => abortController(null));
});

test("writeClipboardText writes to clipboard and handles empty strings and errors", async () => {
  let written = "";
  const mockClipboard = {
    async writeText(text) {
      if (text === "fail") throw new Error("clipboard error");
      written = text;
    },
  };

  assert.equal(await writeClipboardText("", mockClipboard), false);
  assert.equal(written, "");

  assert.equal(await writeClipboardText("hello world", mockClipboard), true);
  assert.equal(written, "hello world");

  assert.equal(await writeClipboardText("fail", mockClipboard), false);
});

test("RuntimeApiClient manages token and sends authorized JSON post requests", async () => {
  const originalFetch = globalThis.fetch;
  try {
    let capturedUrl = "";
    let capturedOptions = null;

    globalThis.fetch = async (url, options) => {
      capturedUrl = String(url);
      capturedOptions = options;
      return {
        ok: true,
        status: 200,
        async json() {
          return { success: true };
        },
      };
    };

    const client = new RuntimeApiClient("/test-api/");
    assert.equal(client.getToken(), "");

    client.setToken("test-token-123");
    assert.equal(client.getToken(), "test-token-123");

    const result = await client.post("test/path", { query: "webcodex" });
    assert.equal(capturedUrl, "/test-api/test/path");
    assert.equal(capturedOptions.method, "POST");
    assert.equal(capturedOptions.headers["Authorization"], "Bearer test-token-123");
    assert.equal(capturedOptions.headers["Content-Type"], "application/json");
    assert.equal(capturedOptions.body, JSON.stringify({ query: "webcodex" }));
    assert.deepEqual(result, { ok: true, status: 200, data: { success: true } });

    client.clearToken();
    assert.equal(client.getToken(), "");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("RuntimeApiClient handles AbortError as null and network error as status 0", async () => {
  const originalFetch = globalThis.fetch;
  try {
    const client = new RuntimeApiClient();

    globalThis.fetch = async () => {
      throw new DOMException("Aborted", "AbortError");
    };
    const abortedResult = await client.post("window", {});
    assert.equal(abortedResult, null);

    globalThis.fetch = async () => {
      throw new TypeError("Failed to fetch");
    };
    const networkResult = await client.post("window", {});
    assert.deepEqual(networkResult, { ok: false, status: 0, data: null });
  } finally {
    globalThis.fetch = originalFetch;
  }
});
