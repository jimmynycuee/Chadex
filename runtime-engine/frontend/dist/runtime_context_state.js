export function resolveRuntimeContextPresentationMode(isWideViewport, isMobileViewport) {
    if (isMobileViewport)
        return "sheet";
    if (isWideViewport)
        return "docked";
    return "popover";
}
export function resolveRuntimeContextState(options) {
    const presentationMode = resolveRuntimeContextPresentationMode(options.isWideViewport, options.isMobileViewport);
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
export function reduceRuntimeContextUserIntent(_previousIntent, action) {
    switch (action.type) {
        case "toggle_trigger":
            return !action.currentVisible;
        case "explicit_open":
            return true;
        case "explicit_close":
            return false;
    }
}
export function resolveRuntimeContextFocusTransition(options) {
    if (!options.wasDocked && options.nextDocked && options.isTriggerFocused) {
        return "inspector_close";
    }
    return "none";
}
