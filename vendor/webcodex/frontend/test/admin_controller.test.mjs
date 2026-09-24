import test from "node:test";
import assert from "node:assert/strict";
import {
  AdminHttpError,
  AdminRefreshController,
} from "../dist/admin_controller.js";

function deferred() {
  let resolve;
  let reject;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function abortError() {
  const error = new Error("aborted");
  error.name = "AbortError";
  return error;
}

function harness() {
  const requests = [];
  const state = {
    rendered: [],
    authenticated: false,
    locked: true,
    lockMessage: "",
    status: "Locked",
    errors: [],
    timerCallback: null,
    timerCleared: false,
  };
  const controller = new AdminRefreshController({
    request(token, signal) {
      const task = deferred();
      requests.push({ token, signal, task });
      return task.promise;
    },
    render(data) {
      state.rendered.push(data);
    },
    showAuthenticated() {
      state.authenticated = true;
      state.locked = false;
    },
    showLocked(message) {
      state.authenticated = false;
      state.locked = true;
      state.lockMessage = message;
    },
    setStatus(message) {
      state.status = message;
    },
    showError(message) {
      state.errors.push(message);
    },
    clearError() {
      state.errors = [];
    },
    setInterval(callback) {
      state.timerCallback = callback;
      state.timerCleared = false;
      return 17;
    },
    clearInterval(handle) {
      assert.equal(handle, 17);
      state.timerCleared = true;
      state.timerCallback = null;
    },
  });
  return { controller, requests, state };
}

async function flush() {
  await Promise.resolve();
  await Promise.resolve();
}

test("lock invalidates a request that later succeeds", async () => {
  const { controller, requests, state } = harness();
  const pending = controller.beginSession("token-a");
  assert.equal(requests.length, 1);
  controller.lock("Locked.");
  assert.equal(requests[0].signal.aborted, true);
  requests[0].task.resolve({ session: "a" });
  await pending;
  assert.equal(state.locked, true);
  assert.equal(state.authenticated, false);
  assert.deepEqual(state.rendered, []);
  assert.equal(state.status, "Locked");
});

test("lock invalidates a request that later fails", async () => {
  const { controller, requests, state } = harness();
  const pending = controller.beginSession("token-a");
  controller.lock("Locked.");
  requests[0].task.reject(new Error("old network failure"));
  await pending;
  assert.deepEqual(state.errors, []);
  assert.equal(state.locked, true);
  assert.equal(state.status, "Locked");
});

test("slow token A cannot overwrite fast token B", async () => {
  const { controller, requests, state } = harness();
  const first = controller.beginSession("token-a");
  const second = controller.beginSession("token-b");
  assert.equal(requests[0].signal.aborted, true);
  requests[1].task.resolve({ session: "b" });
  await second;
  requests[0].task.resolve({ session: "a" });
  await first;
  assert.deepEqual(state.rendered, [{ session: "b" }]);
  assert.equal(state.authenticated, true);
});

test("same-session refreshes are single-flight and cannot reorder", async () => {
  const { controller, requests, state } = harness();
  const first = controller.beginSession("token-a");
  const second = controller.refresh();
  const third = controller.refresh();
  assert.equal(requests.length, 1);
  assert.equal(second, first);
  assert.equal(third, first);
  requests[0].task.resolve({ sequence: 1 });
  await Promise.all([first, second, third]);
  assert.deepEqual(state.rendered, [{ sequence: 1 }]);
});

test("auto-refresh and manual refresh share one active request", async () => {
  const { controller, requests, state } = harness();
  const login = controller.beginSession("token-a");
  requests[0].task.resolve({ sequence: 1 });
  await login;
  controller.startAutoRefresh(1000);
  state.timerCallback();
  const manual = controller.refresh();
  assert.equal(requests.length, 2);
  state.timerCallback();
  assert.equal(requests.length, 2);
  requests[1].task.resolve({ sequence: 2 });
  await manual;
  assert.deepEqual(state.rendered, [{ sequence: 1 }, { sequence: 2 }]);
});

test("current 401 locks the current session", async () => {
  const { controller, requests, state } = harness();
  const pending = controller.beginSession("token-a");
  requests[0].task.reject(new AdminHttpError(401, "authentication required"));
  await pending;
  assert.equal(state.locked, true);
  assert.equal(state.authenticated, false);
  assert.equal(state.lockMessage, "Administrator authentication required.");
  const count = requests.length;
  await controller.refresh();
  assert.equal(requests.length, count);
});

test("stale 401 cannot lock a newer session", async () => {
  const { controller, requests, state } = harness();
  const oldRequest = controller.beginSession("token-a");
  const currentRequest = controller.beginSession("token-b");
  requests[1].task.resolve({ session: "b" });
  await currentRequest;
  requests[0].task.reject(new AdminHttpError(401, "old unauthorized"));
  await oldRequest;
  assert.equal(state.locked, false);
  assert.deepEqual(state.rendered, [{ session: "b" }]);
});

test("current 500 and network errors preserve session and data", async () => {
  const { controller, requests, state } = harness();
  const login = controller.beginSession("token-a");
  requests[0].task.resolve({ sequence: 1 });
  await login;

  const serverFailure = controller.refresh();
  requests[1].task.reject(new AdminHttpError(500, "Request failed (500)"));
  await serverFailure;
  assert.equal(state.authenticated, true);
  assert.equal(state.locked, false);
  assert.deepEqual(state.rendered, [{ sequence: 1 }]);
  assert.deepEqual(state.errors, ["Dashboard refresh failed"]);

  const networkFailure = controller.refresh();
  requests[2].task.reject(new Error("Network unavailable"));
  await networkFailure;
  assert.equal(state.authenticated, true);
  assert.deepEqual(state.rendered, [{ sequence: 1 }]);
  assert.deepEqual(state.errors, ["Dashboard refresh failed"]);

  const retry = controller.refresh();
  assert.equal(requests[3].token, "token-a");
  requests[3].task.resolve({ sequence: 2 });
  await retry;
  assert.deepEqual(state.rendered, [{ sequence: 1 }, { sequence: 2 }]);
});

test("AbortError is silent and does not alter the latest state", async () => {
  const { controller, requests, state } = harness();
  const login = controller.beginSession("token-a");
  requests[0].task.resolve({ sequence: 1 });
  await login;
  const previousStatus = state.status;
  const pending = controller.refresh();
  requests[1].task.reject(abortError());
  await pending;
  assert.deepEqual(state.errors, []);
  assert.equal(state.status, previousStatus);
  assert.equal(state.authenticated, true);
});

test("dispose models pagehide by aborting request and stopping timer", async () => {
  const { controller, requests, state } = harness();
  const login = controller.beginSession("token-a");
  requests[0].task.resolve({ sequence: 1 });
  await login;
  controller.startAutoRefresh(1000);
  const pending = controller.refresh();
  controller.dispose();
  assert.equal(requests[1].signal.aborted, true);
  assert.equal(state.timerCleared, true);
  requests[1].task.reject(abortError());
  await pending;
  assert.deepEqual(state.rendered, [{ sequence: 1 }]);
  assert.deepEqual(state.errors, []);
});

test("invalidateAndRefresh aborts old request and only renders forced result", async () => {
  const h = harness();
  const first = h.controller.beginSession("token");
  assert.equal(h.requests.length, 1);
  const forced = h.controller.invalidateAndRefresh();
  assert.equal(h.requests.length, 2);
  assert.equal(h.requests[0].signal.aborted, true);
  h.requests[0].task.resolve({ value: "old" });
  await first;
  assert.deepEqual(h.state.rendered, []);
  h.requests[1].task.resolve({ value: "new" });
  await forced;
  assert.deepEqual(h.state.rendered, [{ value: "new" }]);
});

test("current unauthorized delegates to unified lock callback", async () => {
  let unauthorized = 0;
  const requests = [];
  const controller = new AdminRefreshController({
    request(token, signal) { const task = deferred(); requests.push({ token, signal, task }); return task.promise; },
    render() {}, showAuthenticated() {}, showLocked() {}, setStatus() {}, showError() {}, clearError() {},
    onUnauthorized() { unauthorized += 1; },
  });
  const run = controller.beginSession("token");
  requests[0].task.reject(new AdminHttpError(401, "unauthorized"));
  await run;
  assert.equal(unauthorized, 1);
});
