function compareCollaborationText(left, right) {
    return left < right ? -1 : left > right ? 1 : 0;
}
export function emptyCollaborationState() {
    return {
        generation: 0,
        sessionId: "",
        messages: [],
        observationToken: "",
        available: true,
        phase: "idle",
        replyTargetId: "",
        editTargetId: "",
        uncertainMutation: null,
        mutationNotice: "",
    };
}
export function resetCollaborationState(collaboration) {
    if (!collaboration)
        return;
    collaboration.generation += 1;
    collaboration.sessionId = "";
    collaboration.messages = [];
    collaboration.observationToken = "";
    collaboration.available = true;
    collaboration.phase = "idle";
    collaboration.replyTargetId = "";
    collaboration.editTargetId = "";
    collaboration.uncertainMutation = null;
    collaboration.mutationNotice = "";
}
function messageCreatedAt(message) {
    return typeof message?.created_at === "number" ? message.created_at : 0;
}
const RUNTIME_COLLABORATION_MUTABLE_KINDS = new Set(["note", "guidance", "question", "todo"]);
export function runtimeCollaborationMessageCanMutate(message) {
    return !!message && message.status === "open" && RUNTIME_COLLABORATION_MUTABLE_KINDS.has(String(message.kind || ""));
}
export function runtimeCollaborationMessageSides(messages, locallyAuthoredMessageIds = new Set()) {
    const sides = new Map();
    for (const message of Array.isArray(messages) ? messages : []) {
        const id = typeof message?.message_id === "string" ? message.message_id : "";
        if (!id)
            continue;
        const side = message?.author_session_id
            ? "incoming"
            : locallyAuthoredMessageIds.has(id)
                ? "outgoing"
                : "neutral";
        sides.set(id, side);
    }
    return sides;
}
function collaborationMessageById(state, messageId) {
    return (Array.isArray(state?.collaboration?.messages) ? state.collaboration.messages : [])
        .find((message) => String(message?.message_id || "") === messageId) || null;
}
function reconcileRuntimeCollaborationMutationState(state, authoritativeRefresh = false) {
    const collaboration = state.collaboration;
    const uncertain = collaboration.uncertainMutation;
    if (uncertain) {
        const original = collaborationMessageById(state, String(uncertain.messageId || ""));
        const confirmedWithdraw = uncertain.kind === "withdraw" && original?.closure_kind === "withdrawn";
        let confirmedReplace = false;
        if (uncertain.kind === "replace") {
            const replacementId = original?.closure_kind === "superseded"
                ? String(original?.superseded_by_message_id || "")
                : "";
            const linkedReplacement = replacementId ? collaborationMessageById(state, replacementId) : null;
            const retainedReplacement = linkedReplacement || (Array.isArray(collaboration.messages)
                ? collaboration.messages.find((message) => message?.supersedes_message_id === uncertain.messageId && message?.message === uncertain.message)
                : null);
            confirmedReplace = !!retainedReplacement
                && retainedReplacement?.supersedes_message_id === uncertain.messageId
                && retainedReplacement?.message === uncertain.message;
        }
        if (confirmedWithdraw || confirmedReplace) {
            collaboration.mutationNotice = confirmedWithdraw
                ? "Withdraw observed after refresh; exact replay required to confirm durability."
                : "Replacement observed after refresh; exact replay required to confirm durability.";
        }
        else if (authoritativeRefresh) {
            collaboration.mutationNotice = "Outcome not observed in retained messages; exact replay required before live observation resumes.";
        }
    }
    if (collaboration.editTargetId) {
        const target = collaborationMessageById(state, collaboration.editTargetId);
        if (!runtimeCollaborationMessageCanMutate(target)) {
            collaboration.editTargetId = "";
            if (!collaboration.mutationNotice) {
                collaboration.mutationNotice = "Message changed while editing; current retained state was refreshed.";
            }
        }
    }
}
export function mergeRuntimeCollaborationMessages(current, updates) {
    const byId = new Map();
    for (const message of Array.isArray(current) ? current : []) {
        const id = typeof message?.message_id === "string" ? message.message_id : "";
        if (id)
            byId.set(id, message);
    }
    for (const message of Array.isArray(updates) ? updates : []) {
        const id = typeof message?.message_id === "string" ? message.message_id : "";
        if (id)
            byId.set(id, message);
    }
    return Array.from(byId.values()).sort((left, right) => messageCreatedAt(left) - messageCreatedAt(right) ||
        compareCollaborationText(String(left?.message_id || ""), String(right?.message_id || "")));
}
export function runtimeCollaborationObservationAction(payload) {
    if (payload?.history_lost)
        return "reload";
    if (payload?.has_more)
        return "drain";
    return "wait";
}
export function runtimeCollaborationRequest(state) {
    if (!state.selectedProject || !state.collaboration.sessionId)
        return null;
    return {
        credentialGeneration: state.credentialGeneration,
        project: state.selectedProject,
        projectGeneration: state.projectGeneration,
        sessionId: state.collaboration.sessionId,
        generation: state.collaboration.generation,
    };
}
export function isCurrentRuntimeCollaborationRequest(state, request) {
    return !!request && request.credentialGeneration === state.credentialGeneration &&
        request.project === state.selectedProject && request.projectGeneration === state.projectGeneration &&
        request.sessionId === state.collaboration.sessionId && request.generation === state.collaboration.generation;
}
export function setRuntimeCollaborationReplyTarget(state, messageId) {
    state.collaboration.replyTargetId = String(messageId || "");
    if (state.collaboration.replyTargetId)
        state.collaboration.editTargetId = "";
}
export function setRuntimeCollaborationEditTarget(state, messageId) {
    const id = String(messageId || "");
    const message = collaborationMessageById(state, id);
    if (!id || !runtimeCollaborationMessageCanMutate(message))
        return false;
    state.collaboration.editTargetId = id;
    state.collaboration.replyTargetId = "";
    state.collaboration.mutationNotice = "";
    return true;
}
export function clearRuntimeCollaborationEditTarget(state) {
    state.collaboration.editTargetId = "";
}
export function runtimeCollaborationEditTarget(state) {
    const id = String(state?.collaboration?.editTargetId || "");
    return id ? collaborationMessageById(state, id) : null;
}
export function markRuntimeCollaborationMutationUncertain(state, request, mutation) {
    if (!isCurrentRuntimeCollaborationRequest(state, request))
        return false;
    state.collaboration.uncertainMutation = {
        kind: mutation?.kind === "replace" ? "replace" : "withdraw",
        messageId: String(mutation?.messageId || ""),
        ...(mutation?.kind === "replace" ? { message: String(mutation?.message || "") } : {}),
    };
    state.collaboration.mutationNotice = "Outcome unknown; refresh retained messages before retrying.";
    return true;
}
export function runtimeCollaborationMutationRecovery(state, request) {
    if (!isCurrentRuntimeCollaborationRequest(state, request))
        return null;
    const mutation = state?.collaboration?.uncertainMutation;
    const messageId = String(mutation?.messageId || "");
    if (!mutation || !messageId)
        return null;
    return {
        kind: mutation.kind === "replace" ? "replace" : "withdraw",
        messageId,
        ...(mutation.kind === "replace" ? { message: String(mutation.message || "") } : {}),
    };
}
export function completeRuntimeCollaborationMutationRecovery(state, request, notice) {
    if (!isCurrentRuntimeCollaborationRequest(state, request))
        return false;
    state.collaboration.uncertainMutation = null;
    state.collaboration.mutationNotice = String(notice || "");
    return true;
}
export function takeRuntimeCollaborationMutationNotice(state) {
    const notice = String(state?.collaboration?.mutationNotice || "");
    state.collaboration.mutationNotice = "";
    return notice;
}
export function adoptRuntimeCollaborationList(state, request, messages) {
    if (!isCurrentRuntimeCollaborationRequest(state, request))
        return false;
    state.collaboration.messages = mergeRuntimeCollaborationMessages([], messages);
    reconcileRuntimeCollaborationMutationState(state, true);
    return true;
}
export function adoptRuntimeCollaborationObservation(state, request, payload) {
    if (!isCurrentRuntimeCollaborationRequest(state, request))
        return false;
    state.collaboration.messages = mergeRuntimeCollaborationMessages(state.collaboration.messages, Array.isArray(payload?.messages) ? payload.messages : []);
    if (typeof payload?.observation_token === "string")
        state.collaboration.observationToken = payload.observation_token;
    reconcileRuntimeCollaborationMutationState(state, false);
    return true;
}
export function setRuntimeCollaborationAvailable(state, request, available) {
    if (!isCurrentRuntimeCollaborationRequest(state, request))
        return false;
    state.collaboration.available = available;
    if (!available) {
        state.collaboration.editTargetId = "";
        state.collaboration.replyTargetId = "";
    }
    return true;
}
export function setRuntimeCollaborationPhase(state, request, phase) {
    if (!isCurrentRuntimeCollaborationRequest(state, request))
        return false;
    state.collaboration.phase = phase;
    return true;
}
export function runtimeCollaborationNeedsRefreshRecovery(state) {
    return state?.collaboration?.phase === "paused";
}
