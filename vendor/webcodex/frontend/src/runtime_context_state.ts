export type RuntimeContextPresentationMode = "docked" | "popover" | "sheet";

export interface RuntimeContextLayoutOptions {
  userIntent: boolean | null;
  isWideViewport: boolean;
  isMobileViewport: boolean;
  hasSelectedSession: boolean;
  workspaceView: "sessions" | "operations" | "windows";
}

export interface RuntimeContextResolvedState {
  visible: boolean;
  presentationMode: RuntimeContextPresentationMode;
  isDocked: boolean;
}

export function resolveRuntimeContextPresentationMode(
  isWideViewport: boolean,
  isMobileViewport: boolean
): RuntimeContextPresentationMode {
  if (isMobileViewport) return "sheet";
  if (isWideViewport) return "docked";
  return "popover";
}

export function resolveRuntimeContextState(
  options: RuntimeContextLayoutOptions
): RuntimeContextResolvedState {
  const presentationMode = resolveRuntimeContextPresentationMode(
    options.isWideViewport,
    options.isMobileViewport
  );
  const sessionAvailable = options.hasSelectedSession && options.workspaceView === "sessions";
  if (!sessionAvailable) {
    return {
      visible: false,
      presentationMode,
      isDocked: false,
    };
  }
  const visible = options.userIntent !== null
    ? options.userIntent
    : options.isWideViewport;
  const isDocked = visible && presentationMode === "docked";
  return {
    visible,
    presentationMode,
    isDocked,
  };
}

export type RuntimeContextUserAction =
  | { type: "toggle_trigger"; currentVisible: boolean }
  | { type: "explicit_open" }
  | { type: "explicit_close" };

export function reduceRuntimeContextUserIntent(
  _previousIntent: boolean | null,
  action: RuntimeContextUserAction
): boolean {
  switch (action.type) {
    case "toggle_trigger":
      return !action.currentVisible;
    case "explicit_open":
      return true;
    case "explicit_close":
      return false;
  }
}

export interface RuntimeContextFocusTransitionOptions {
  wasDocked: boolean;
  nextDocked: boolean;
  isTriggerFocused: boolean;
}

export type RuntimeContextFocusTarget = "inspector_close" | "none";

export function resolveRuntimeContextFocusTransition(
  options: RuntimeContextFocusTransitionOptions
): RuntimeContextFocusTarget {
  if (!options.wasDocked && options.nextDocked && options.isTriggerFocused) {
    return "inspector_close";
  }
  return "none";
}
