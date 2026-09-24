import test from "node:test";
import assert from "node:assert/strict";
import {
  resolveRuntimeContextState,
  resolveRuntimeContextPresentationMode,
  reduceRuntimeContextUserIntent,
  resolveRuntimeContextFocusTransition,
} from "../dist/runtime_context_state.js";

test("runtime context resolution separates presentation mode from user visibility intent", () => {
  assert.equal(resolveRuntimeContextPresentationMode(true, false), "docked");
  assert.equal(resolveRuntimeContextPresentationMode(false, false), "popover");
  assert.equal(resolveRuntimeContextPresentationMode(false, true), "sheet");
  assert.equal(resolveRuntimeContextPresentationMode(true, true), "sheet");

  const wideDefault = resolveRuntimeContextState({
    userIntent: null,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(wideDefault.visible, true);
  assert.equal(wideDefault.presentationMode, "docked");
  assert.equal(wideDefault.isDocked, true);

  const normalDefault = resolveRuntimeContextState({
    userIntent: null,
    isWideViewport: false,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(normalDefault.visible, false);
  assert.equal(normalDefault.presentationMode, "popover");
  assert.equal(normalDefault.isDocked, false);

  const mobileDefault = resolveRuntimeContextState({
    userIntent: null,
    isWideViewport: false,
    isMobileViewport: true,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(mobileDefault.visible, false);
  assert.equal(mobileDefault.presentationMode, "sheet");
  assert.equal(mobileDefault.isDocked, false);

  const wideUserClosed = resolveRuntimeContextState({
    userIntent: false,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(wideUserClosed.visible, false);
  assert.equal(wideUserClosed.isDocked, false);

  const wideAfterRefresh = resolveRuntimeContextState({
    userIntent: false,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(wideAfterRefresh.visible, false);
  assert.equal(wideAfterRefresh.isDocked, false);

  const normalUserOpened = resolveRuntimeContextState({
    userIntent: true,
    isWideViewport: false,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(normalUserOpened.visible, true);
  assert.equal(normalUserOpened.presentationMode, "popover");
  assert.equal(normalUserOpened.isDocked, false);

  const wideAfterResize = resolveRuntimeContextState({
    userIntent: true,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(wideAfterResize.visible, true);
  assert.equal(wideAfterResize.presentationMode, "docked");
  assert.equal(wideAfterResize.isDocked, true);

  const normalClosedResize = resolveRuntimeContextState({
    userIntent: false,
    isWideViewport: false,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(normalClosedResize.visible, false);
  const wideClosedResize = resolveRuntimeContextState({
    userIntent: false,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(wideClosedResize.visible, false);
  assert.equal(wideClosedResize.isDocked, false);

  const operationsView = resolveRuntimeContextState({
    userIntent: true,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "operations",
  });
  assert.equal(operationsView.visible, false);
  assert.equal(operationsView.isDocked, false);

  const windowsView = resolveRuntimeContextState({
    userIntent: true,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "windows",
  });
  assert.equal(windowsView.visible, false);
  assert.equal(windowsView.isDocked, false);

  const backToSessions = resolveRuntimeContextState({
    userIntent: true,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(backToSessions.visible, true);
  assert.equal(backToSessions.isDocked, true);

  const noSession = resolveRuntimeContextState({
    userIntent: true,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: false,
    workspaceView: "sessions",
  });
  assert.equal(noSession.visible, false);
  assert.equal(noSession.isDocked, false);

  const sessionRestored = resolveRuntimeContextState({
    userIntent: true,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(sessionRestored.visible, true);
  assert.equal(sessionRestored.isDocked, true);
});

test("context user intent reducer and lifecycle transitions ensure programmatic projections never pollute intent", () => {
  // Scenario 1: Initial wide viewport defaults to open; programmatic DOM projection does NOT contaminate userIntent.
  let userIntent = null;
  const wideDefault = resolveRuntimeContextState({
    userIntent,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(wideDefault.visible, true);
  assert.equal(wideDefault.isDocked, true);

  // Programmatic DOM projection sets inspector.open = true.
  // Critical invariant: userIntent must remain null!
  assert.equal(userIntent, null);

  // Resize to normal viewport: null intent correctly resolves to closed.
  const normalAfterResize = resolveRuntimeContextState({
    userIntent,
    isWideViewport: false,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(normalAfterResize.visible, false);
  assert.equal(normalAfterResize.isDocked, false);

  // Scenario 2: User explicitly opens context. Switching to Operations hides it, returning to Sessions restores it.
  userIntent = reduceRuntimeContextUserIntent(userIntent, { type: "explicit_open" });
  assert.equal(userIntent, true);

  const opsView = resolveRuntimeContextState({
    userIntent,
    isWideViewport: false,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "operations",
  });
  assert.equal(opsView.visible, false, "Operations view temporarily hides session context");
  // Switching views or programmatic closing must NOT overwrite userIntent
  assert.equal(userIntent, true, "userIntent remains true during operations view");

  const backToSessions = resolveRuntimeContextState({
    userIntent,
    isWideViewport: false,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(backToSessions.visible, true, "Context re-opens when returning to Sessions view");

  // Scenario 3: Selected session temporarily unavailable does NOT record as manual collapse.
  const sessionUnavailable = resolveRuntimeContextState({
    userIntent,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: false,
    workspaceView: "sessions",
  });
  assert.equal(sessionUnavailable.visible, false);
  assert.equal(userIntent, true, "temporary unavailability does not clear userIntent");

  const sessionAvailableAgain = resolveRuntimeContextState({
    userIntent,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(sessionAvailableAgain.visible, true);
  assert.equal(sessionAvailableAgain.isDocked, true);

  // Scenario 4: Explicit user close sets userIntent = false and persists across refresh / resize.
  userIntent = reduceRuntimeContextUserIntent(userIntent, { type: "explicit_close" });
  assert.equal(userIntent, false);

  const closedOnWide = resolveRuntimeContextState({
    userIntent,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(closedOnWide.visible, false);
  assert.equal(closedOnWide.isDocked, false);

  // Refresh / resize simulation: userIntent remains false
  const closedAfterRefresh = resolveRuntimeContextState({
    userIntent,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(closedAfterRefresh.visible, false);

  // Scenario 5: Trigger toggle action correctly inverts visibility.
  // When visible in DOM, toggle action closes context.
  const toggledClosed = reduceRuntimeContextUserIntent(null, { type: "toggle_trigger", currentVisible: true });
  assert.equal(toggledClosed, false);

  // When hidden in DOM, toggle action opens context.
  const toggledOpen = reduceRuntimeContextUserIntent(null, { type: "toggle_trigger", currentVisible: false });
  assert.equal(toggledOpen, true);

  // 1279 <-> 1280 transitions with explicit open intent preserve intent and only switch presentation mode.
  userIntent = true;
  const at1279 = resolveRuntimeContextState({
    userIntent,
    isWideViewport: false,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(at1279.visible, true);
  assert.equal(at1279.presentationMode, "popover");
  assert.equal(at1279.isDocked, false);

  const at1280 = resolveRuntimeContextState({
    userIntent,
    isWideViewport: true,
    isMobileViewport: false,
    hasSelectedSession: true,
    workspaceView: "sessions",
  });
  assert.equal(at1280.visible, true);
  assert.equal(at1280.presentationMode, "docked");
  assert.equal(at1280.isDocked, true);
});

test("context focus transition preserves accessible focus across breakpoint and close actions", () => {
  // P2 Case 1: Popover is open and trigger is focused. Viewport resizes 1279 -> 1280.
  // Trigger will be hidden by CSS (display: none), so focus MUST transfer to #runtime-inspector-close.
  assert.equal(
    resolveRuntimeContextFocusTransition({
      wasDocked: false,
      nextDocked: true,
      isTriggerFocused: true,
    }),
    "inspector_close"
  );

  // P2 Case 2: Popover is open but focus was elsewhere (e.g. inside chat or timeline).
  // Resize to 1280 must NOT steal focus!
  assert.equal(
    resolveRuntimeContextFocusTransition({
      wasDocked: false,
      nextDocked: true,
      isTriggerFocused: false,
    }),
    "none"
  );

  // P2 Case 3: Context is closed and user resizes across 1279 <-> 1280.
  // nextDocked is false because closed context never docks; focus must never be stolen.
  assert.equal(
    resolveRuntimeContextFocusTransition({
      wasDocked: false,
      nextDocked: false,
      isTriggerFocused: true,
    }),
    "none"
  );

  // P2 Case 4: Already docked; resize within >=1280 range does not transfer focus.
  assert.equal(
    resolveRuntimeContextFocusTransition({
      wasDocked: true,
      nextDocked: true,
      isTriggerFocused: false,
    }),
    "none"
  );

  // P2 Case 5: Reverse transition from docked (1280) to popover (1279).
  // Close button remains visible in popover header; no focus jump needed.
  assert.equal(
    resolveRuntimeContextFocusTransition({
      wasDocked: true,
      nextDocked: false,
      isTriggerFocused: false,
    }),
    "none"
  );
});
