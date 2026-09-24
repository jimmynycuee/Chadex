import test from "node:test";
import assert from "node:assert/strict";
import {
  initialRuntimeConsoleState,
  beginRuntimeCredential,
  selectRuntimeProject,
  selectRuntimeWorkflowSession,
} from "../dist/runtime_console_state.js";
import {
  runtimeCollaborationRequest,
  adoptRuntimeCollaborationList,
  adoptRuntimeCollaborationObservation,
  setRuntimeCollaborationAvailable,
  setRuntimeCollaborationPhase,
  runtimeCollaborationNeedsRefreshRecovery,
  mergeRuntimeCollaborationMessages,
  runtimeCollaborationObservationAction,
  runtimeCollaborationMessageCanMutate,
  runtimeCollaborationMessageSides,
  setRuntimeCollaborationReplyTarget,
  setRuntimeCollaborationEditTarget,
  runtimeCollaborationEditTarget,
  markRuntimeCollaborationMutationUncertain,
  runtimeCollaborationMutationRecovery,
  completeRuntimeCollaborationMutationRecovery,
  takeRuntimeCollaborationMutationNotice,
} from "../dist/runtime_collaboration_state.js";

test("collaboration delta replaces message state by id and completion renders todo resolution plus answer", () => {
  const state = initialRuntimeConsoleState();
  beginRuntimeCredential(state);
  selectRuntimeProject(state, "runner", "agent:runner:project");
  selectRuntimeWorkflowSession(state, "wc_sess_a");
  const request = runtimeCollaborationRequest(state);
  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_todo", kind: "todo", status: "open", created_at: 1, message: "do work" },
  ]);
  assert.equal(adoptRuntimeCollaborationObservation(state, request, {
    observation_token: "opaque-1",
    messages: [
      { message_id: "wc_msg_todo", kind: "todo", status: "resolved", created_at: 1, message: "do work", resolved_by_message_id: "wc_msg_answer" },
      { message_id: "wc_msg_answer", kind: "answer", status: "open", created_at: 2, message: "done", reply_to: "wc_msg_todo", author_session_id: "wc_sess_worker" },
    ],
  }), true);
  assert.equal(state.collaboration.messages.length, 2);
  assert.equal(state.collaboration.messages[0].status, "resolved");
  assert.equal(state.collaboration.messages[1].reply_to, "wc_msg_todo");
  assert.equal(state.collaboration.observationToken, "opaque-1");
});

test("history loss reloads and has_more drains without duplicate message ids", () => {
  assert.equal(runtimeCollaborationObservationAction({ history_lost: true, has_more: true }), "reload");
  assert.equal(runtimeCollaborationObservationAction({ history_lost: false, has_more: true }), "drain");
  assert.equal(runtimeCollaborationObservationAction({ wait_outcome: "timeout" }), "wait");
  const merged = mergeRuntimeCollaborationMessages(
    [{ message_id: "a", created_at: 1, status: "open" }],
    [{ message_id: "a", created_at: 1, status: "resolved" }, { message_id: "b", created_at: 2 }]
  );
  assert.deepEqual(merged.map((message) => message.message_id), ["a", "b"]);
  assert.equal(merged[0].status, "resolved");
});

test("project-read-only degradation keeps project selection while collaboration is marked unavailable", () => {
  const state = initialRuntimeConsoleState();
  beginRuntimeCredential(state);
  selectRuntimeProject(state, "runner", "agent:runner:project");
  selectRuntimeWorkflowSession(state, "wc_sess_a");
  const request = runtimeCollaborationRequest(state);
  assert.equal(setRuntimeCollaborationAvailable(state, request, false), true);
  assert.equal(state.selectedProject, "agent:runner:project");
  assert.equal(state.workflow.selectedSessionId, "wc_sess_a");
  assert.equal(state.collaboration.available, false);
});

test("manual Refresh recovery is required only after collaboration is paused", () => {
  const state = initialRuntimeConsoleState();
  beginRuntimeCredential(state);
  selectRuntimeProject(state, "runner", "agent:runner:project");
  selectRuntimeWorkflowSession(state, "wc_sess_a");
  const requestA = runtimeCollaborationRequest(state);
  assert.equal(setRuntimeCollaborationPhase(state, requestA, "live"), true);
  assert.equal(runtimeCollaborationNeedsRefreshRecovery(state), false);
  assert.equal(setRuntimeCollaborationPhase(state, requestA, "paused"), true);
  assert.equal(runtimeCollaborationNeedsRefreshRecovery(state), true);
  selectRuntimeWorkflowSession(state, "wc_sess_b");
  assert.equal(state.collaboration.phase, "idle");
  assert.equal(setRuntimeCollaborationPhase(state, requestA, "paused"), false);
  assert.equal(runtimeCollaborationNeedsRefreshRecovery(state), false);
});

test("collaboration Edit and Reply are mutually exclusive and context switches clear edit state", () => {
  const state = initialRuntimeConsoleState();
  beginRuntimeCredential(state);
  selectRuntimeProject(state, "runner", "agent:runner:project");
  selectRuntimeWorkflowSession(state, "wc_sess_a");
  let request = runtimeCollaborationRequest(state);
  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_edit", kind: "guidance", status: "open", priority: "high", requires_ack: true, first_ack_observed_at: 10, created_at: 1, message: "old" },
  ]);
  assert.equal(setRuntimeCollaborationEditTarget(state, "wc_msg_edit"), true);
  assert.equal(runtimeCollaborationEditTarget(state).message_id, "wc_msg_edit");
  assert.equal(state.collaboration.replyTargetId, "");
  setRuntimeCollaborationReplyTarget(state, "wc_msg_edit");
  assert.equal(runtimeCollaborationEditTarget(state), null);
  assert.equal(state.collaboration.replyTargetId, "wc_msg_edit");
  assert.equal(setRuntimeCollaborationEditTarget(state, "wc_msg_edit"), true);
  assert.equal(state.collaboration.replyTargetId, "");

  selectRuntimeWorkflowSession(state, "wc_sess_b");
  assert.equal(runtimeCollaborationEditTarget(state), null);
  assert.equal(state.collaboration.replyTargetId, "");
  request = runtimeCollaborationRequest(state);
  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_b", kind: "note", status: "open", created_at: 2, message: "b" },
  ]);
  assert.equal(setRuntimeCollaborationEditTarget(state, "wc_msg_b"), true);
  selectRuntimeProject(state, "other", "agent:other:project");
  assert.equal(runtimeCollaborationEditTarget(state), null);
  assert.equal(state.collaboration.replyTargetId, "");
});

test("incoming authoritative closure cancels edit while preserving refreshed state", () => {
  const state = initialRuntimeConsoleState();
  beginRuntimeCredential(state);
  selectRuntimeProject(state, "runner", "agent:runner:project");
  selectRuntimeWorkflowSession(state, "wc_sess_a");
  const request = runtimeCollaborationRequest(state);
  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_todo", kind: "todo", status: "open", created_at: 1, message: "work" },
  ]);
  assert.equal(setRuntimeCollaborationEditTarget(state, "wc_msg_todo"), true);
  adoptRuntimeCollaborationObservation(state, request, {
    messages: [{ message_id: "wc_msg_todo", kind: "todo", status: "resolved", resolved_at: 2, created_at: 1, message: "work" }],
  });
  assert.equal(runtimeCollaborationEditTarget(state), null);
  assert.equal(state.collaboration.messages.length, 1);
  assert.equal(state.collaboration.messages[0].status, "resolved");
  assert.equal(takeRuntimeCollaborationMutationNotice(state), "Message changed while editing; current retained state was refreshed.");
});

test("withdraw and replacement responses merge by message id without duplicate history", () => {
  const state = initialRuntimeConsoleState();
  beginRuntimeCredential(state);
  selectRuntimeProject(state, "runner", "agent:runner:project");
  selectRuntimeWorkflowSession(state, "wc_sess_a");
  const request = runtimeCollaborationRequest(state);
  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_old", kind: "note", status: "open", created_at: 1, message: "wrong" },
  ]);
  adoptRuntimeCollaborationObservation(state, request, {
    messages: [
      { message_id: "wc_msg_old", kind: "note", status: "resolved", closure_kind: "superseded", superseded_by_message_id: "wc_msg_new", created_at: 1, message: "wrong" },
      { message_id: "wc_msg_new", kind: "note", status: "open", supersedes_message_id: "wc_msg_old", created_at: 2, message: "right" },
    ],
  });
  assert.deepEqual(state.collaboration.messages.map((message) => message.message_id), ["wc_msg_old", "wc_msg_new"]);
  adoptRuntimeCollaborationObservation(state, request, {
    messages: [{ message_id: "wc_msg_new", kind: "note", status: "resolved", closure_kind: "withdrawn", created_at: 2, message: "right" }],
  });
  assert.equal(state.collaboration.messages.length, 2);
  assert.equal(state.collaboration.messages[1].closure_kind, "withdrawn");
});

test("unknown mutation outcome stays fenced until exact replay confirms durability", () => {
  const state = initialRuntimeConsoleState();
  beginRuntimeCredential(state);
  selectRuntimeProject(state, "runner", "agent:runner:project");
  selectRuntimeWorkflowSession(state, "wc_sess_a");
  const request = runtimeCollaborationRequest(state);
  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_old", kind: "guidance", status: "open", priority: "high", requires_ack: true, created_at: 1, message: "wrong" },
  ]);
  assert.equal(setRuntimeCollaborationEditTarget(state, "wc_msg_old"), true);
  assert.equal(markRuntimeCollaborationMutationUncertain(state, request, { kind: "replace", messageId: "wc_msg_old", message: "right" }), true);
  assert.equal(state.collaboration.messages.length, 1);
  assert.equal(state.collaboration.uncertainMutation.messageId, "wc_msg_old");

  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_old", kind: "guidance", status: "resolved", closure_kind: "superseded", superseded_by_message_id: "wc_msg_new", priority: "high", requires_ack: true, created_at: 1, message: "wrong" },
    { message_id: "wc_msg_new", kind: "guidance", status: "open", supersedes_message_id: "wc_msg_old", priority: "high", requires_ack: true, created_at: 2, message: "right" },
  ]);
  assert.equal(state.collaboration.uncertainMutation.messageId, "wc_msg_old");
  assert.equal(runtimeCollaborationEditTarget(state), null);
  assert.equal(
    takeRuntimeCollaborationMutationNotice(state),
    "Replacement observed after refresh; exact replay required to confirm durability."
  );
  assert.deepEqual(runtimeCollaborationMutationRecovery(state, request), {
    kind: "replace",
    messageId: "wc_msg_old",
    message: "right",
  });
  assert.equal(
    completeRuntimeCollaborationMutationRecovery(
      state, request, "Replacement durably confirmed after exact replay."
    ),
    true
  );
  assert.equal(state.collaboration.uncertainMutation, null);
  assert.equal(
    takeRuntimeCollaborationMutationNotice(state),
    "Replacement durably confirmed after exact replay."
  );
  assert.equal(state.collaboration.messages.length, 2);
});

test("unknown replace outcome remains recoverable when retained source was evicted", () => {
  const state = initialRuntimeConsoleState();
  beginRuntimeCredential(state);
  selectRuntimeProject(state, "runner", "agent:runner:project");
  selectRuntimeWorkflowSession(state, "wc_sess_a");
  const request = runtimeCollaborationRequest(state);
  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_old", kind: "note", status: "open", created_at: 1, message: "wrong" },
  ]);
  assert.equal(markRuntimeCollaborationMutationUncertain(state, request, { kind: "replace", messageId: "wc_msg_old", message: "right" }), true);
  adoptRuntimeCollaborationList(state, request, [
    { message_id: "wc_msg_new", kind: "note", status: "open", supersedes_message_id: "wc_msg_old", created_at: 2, message: "right" },
  ]);
  assert.equal(state.collaboration.uncertainMutation.messageId, "wc_msg_old");
  assert.equal(
    takeRuntimeCollaborationMutationNotice(state),
    "Replacement observed after refresh; exact replay required to confirm durability."
  );
  assert.deepEqual(runtimeCollaborationMutationRecovery(state, request), {
    kind: "replace",
    messageId: "wc_msg_old",
    message: "right",
  });
});

test("only eligible open Human Join kinds expose mutation actions and ACK is not a lock", () => {
  for (const kind of ["note", "guidance", "question", "todo"]) {
    assert.equal(runtimeCollaborationMessageCanMutate({ kind, status: "open" }), true, kind);
  }
  assert.equal(runtimeCollaborationMessageCanMutate({ kind: "guidance", status: "open", requires_ack: true, first_ack_observed_at: 100 }), true);
  for (const kind of ["answer", "progress", "decision", "risk", "proposal"]) {
    assert.equal(runtimeCollaborationMessageCanMutate({ kind, status: "open" }), false, kind);
  }
  assert.equal(runtimeCollaborationMessageCanMutate({ kind: "note", status: "resolved", closure_kind: "withdrawn" }), false);
  assert.equal(runtimeCollaborationMessageCanMutate({ kind: "todo", status: "resolved", closure_kind: "superseded" }), false);
});

test("conversation presentation never infers authorship from reply topology or message kind", () => {
  const sides = runtimeCollaborationMessageSides([
    { message_id: "user-root", kind: "note", message: "hello" },
    { message_id: "reply-without-provenance", kind: "note", reply_to: "user-root", message: "received" },
    { message_id: "local-reply", kind: "question", reply_to: "reply-without-provenance", message: "why" },
    { message_id: "trusted-agent", kind: "progress", author_session_id: "wc_sess_worker", message: "working" },
    { message_id: "answer-without-provenance", kind: "answer", message: "done" },
    { message_id: "retained-reply", kind: "note", reply_to: "missing", message: "retained" },
  ], new Set(["user-root", "local-reply"]));
  assert.equal(sides.get("user-root"), "outgoing");
  assert.equal(sides.get("reply-without-provenance"), "neutral");
  assert.equal(sides.get("local-reply"), "outgoing");
  assert.equal(sides.get("trusted-agent"), "incoming");
  assert.equal(sides.get("answer-without-provenance"), "neutral");
  assert.equal(sides.get("retained-reply"), "neutral");
});
