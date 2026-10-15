import Foundation
import XCTest
@testable import ChadexApp

final class PlanGoalModelTests: XCTestCase {
    private let planHash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

    private func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> PlanGoalEnvelope<T> {
        try JSONDecoder.chadex.decode(PlanGoalEnvelope<T>.self, from: Data(json.utf8))
    }

    func testOverviewOutputDecodesPlansAndRunsWithUnknownFields() throws {
        let json = #"""
        {"success":true,"output":{
          "plans":[{"plan_id":"plan_1","project_id":"p","title":"Refactor","state":"drafting",
            "latest_revision":2,"plan_mode_active":true,"plan_mode_expires_at_unix_ms":1791100000000,
            "created_at_unix_ms":1,"updated_at_unix_ms":2,"closed_reason":null,"future_field":1}],
          "goal_runs":[{"goal_run_id":"run_1","plan_id":"plan_1","revision":2,"content_sha256":"abc",
            "project":"/tmp/p","state":"needs_attention","state_reason":"check failed",
            "iteration":3,"max_iterations":5,"deadline_at_unix_ms":9,"provider":"codex",
            "workspace_path":null,"result_branch":"goal/run_1","tokens_used":1200,
            "cost":"tokens_reported_by_provider","created_at_unix_ms":1,"updated_at_unix_ms":2}],
          "emergency_stop_engaged":true,"plan_mode_locks":1,"supervisor_running":true}}
        """#
        let output = try XCTUnwrap(try decode(GoalOverviewOutput.self, json).output)
        XCTAssertEqual(output.plans?.first?.planId, "plan_1")
        XCTAssertEqual(output.plans?.first?.latestRevision, 2)
        XCTAssertTrue(output.plans?.first?.isPlanModeActive ?? false)
        XCTAssertNil(output.plans?.first?.closedReason)
        let run = try XCTUnwrap(output.goalRuns?.first)
        XCTAssertEqual(run.state, "needs_attention")
        XCTAssertFalse(run.canResume)
        XCTAssertFalse(run.canPause)
        XCTAssertEqual(run.resultBranch, "goal/run_1")
        XCTAssertNil(run.workspacePath)
        XCTAssertEqual(run.tokensUsed, 1200)
        XCTAssertEqual(output.emergencyStopEngaged, true)
        XCTAssertEqual(output.supervisorRunning, true)
    }

    func testInspectOutputDecodesRevisionDiffAndDecisions() throws {
        let json = """
        {"success":true,"output":{
          "plan":{"plan_id":"plan_1","title":"T","state":"approved","latest_revision":2},
          "revision":{"revision":2,"content":"# Plan\\nDo it","content_sha256":"\(planHash)","created_at_unix_ms":5,
            "spec":{"acceptance":[{"name":"tests","argv":["swift","test"],"cwd":"app","timeout_secs":300}],
                    "budget":{"max_iterations":4,"max_minutes":60,"agent_timeout_secs":600}}},
          "revisions":[{"revision":1,"content_sha256":"aa","created_at_unix_ms":1},
                       {"revision":2,"content_sha256":"\(planHash)","created_at_unix_ms":5}],
          "decisions":[{"approval_id":"ap_1","plan_id":"plan_1","revision":2,"content_sha256":"\(planHash)",
            "state":"active","binding":{"project_id":"p","runner_client_id":"c","project_root":"/p","base_commit":"deadbeef"},
            "approver":"operator","channel":"app","reason":null,"created_at_unix_ms":6,
            "expires_at_unix_ms":99,"consumed_by_goal_run_id":null}],
          "diff":{"changed_lines":2,"lines":[" same","+added","-removed"],"truncated":false}}}
        """
        let output = try XCTUnwrap(try decode(PlanInspectOutput.self, json).output)
        let revision = try XCTUnwrap(output.revision)
        XCTAssertEqual(revision.revision, 2)
        XCTAssertEqual(revision.shortHash, "0123456789ab")
        XCTAssertEqual(revision.spec?.acceptance?.first?.commandLine, "swift test")
        XCTAssertEqual(revision.spec?.acceptance?.first?.cwd, "app")
        XCTAssertEqual(revision.spec?.budget?.maxIterations, 4)
        XCTAssertEqual(output.revisions?.count, 2)
        XCTAssertEqual(output.diff?.lines, [" same", "+added", "-removed"])
        XCTAssertEqual(output.diff?.truncated, false)
        let decision = try XCTUnwrap(output.decisions?.first)
        XCTAssertTrue(decision.isActive)
        XCTAssertEqual(decision.binding?.baseCommit, "deadbeef")
        XCTAssertNil(decision.consumedByGoalRunId)
    }

    func testStatusOutputDecodesOperationsAndEvents() throws {
        let json = #"""
        {"success":true,"output":{
          "goal_run":{"goal_run_id":"run_1","state":"running","iteration":1,"max_iterations":5,"cost":"unknown"},
          "acceptance":[{"name":"tests","passed":false}],
          "operations":[{"op_key":"agent-1","kind":"agent","iteration":1,"state":"running","job_id":"j1",
            "result":null,"started_at_unix_ms":10,"finished_at_unix_ms":null}],
          "events":[{"seq":1,"at_unix_ms":11,"kind":"started","detail":{"iteration":1,"note":"go"}}],
          "emergency_stop_engaged":false}}
        """#
        let output = try XCTUnwrap(try decode(GoalStatusOutput.self, json).output)
        XCTAssertEqual(output.goalRun.goalRunId, "run_1")
        XCTAssertTrue(output.goalRun.canPause)
        XCTAssertNil(output.goalRun.tokensUsed)
        XCTAssertEqual(output.operations?.first?.opKey, "agent-1")
        XCTAssertEqual(output.operations?.first?.jobId, "j1")
        XCTAssertEqual(output.events?.first?.kind, "started")
        XCTAssertFalse(PlanGoalFormat.compact(output.events?.first?.detail).isEmpty)
        XCTAssertEqual(output.emergencyStopEngaged, false)
    }

    func testFailureEnvelopeCarriesErrorAndKind() throws {
        let json = #"{"success":false,"output":{"error_kind":"plan_revision_stale","message":"stale"},"error":"plan revision is stale"}"#
        let envelope = try decode(PlanInspectOutput.self, json)
        XCTAssertFalse(envelope.success)
        XCTAssertNil(envelope.output)
        XCTAssertEqual(envelope.error, "plan revision is stale")
        XCTAssertEqual(envelope.failure?.errorKind, "plan_revision_stale")
    }

    func testApproveParamsEncodeExactRevisionAndHash() throws {
        let params = PlanGoalOperatorParams(
            path: "/tmp/project",
            tool: "plan_review",
            arguments: PlanGoalArguments(action: "approve", planId: "plan_1", revision: 7, contentSha256: planHash)
        )
        let data = try JSONEncoder.chadex.encode(params)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(object["path"] as? String, "/tmp/project")
        XCTAssertEqual(object["tool"] as? String, "plan_review")
        let arguments = try XCTUnwrap(object["arguments"] as? [String: Any])
        XCTAssertEqual(arguments["action"] as? String, "approve")
        XCTAssertEqual(arguments["plan_id"] as? String, "plan_1")
        XCTAssertEqual(arguments["revision"] as? Int, 7)
        XCTAssertEqual(arguments["content_sha256"] as? String, planHash)
        XCTAssertNil(arguments["reason"])
    }

    func testClearBoardArgumentsLeaveProjectResolutionToHelper() throws {
        let data = try JSONEncoder.chadex.encode(
            PlanGoalArguments(action: "clear_board")
        )
        let arguments = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(arguments["action"] as? String, "clear_board")
        XCTAssertNil(arguments["project"])
        XCTAssertNil(arguments["path"])
    }

    func testGoalRunDecodesModeFieldsAndDefaultsToCodex() throws {
        let legacy = try XCTUnwrap(try decode(GoalRunOutput.self,
            #"{"success":true,"output":{"goal_run":{"goal_run_id":"r1","state":"running"}}}"#).output?.goalRun)
        XCTAssertNil(legacy.mode)
        XCTAssertFalse(legacy.isChatgptMode)
        XCTAssertFalse(legacy.isWaitingForChatgpt)
        XCTAssertFalse(legacy.isChatgptIdle)

        let waiting = try XCTUnwrap(try decode(GoalRunOutput.self, #"""
        {"success":true,"output":{"goal_run":{"goal_run_id":"r2","state":"running","mode":"chatgpt",
         "workspace_project":"/tmp/p","waiting_for_chatgpt":true,"next_action":"call goal_check"}}}
        """#).output?.goalRun)
        XCTAssertTrue(waiting.isChatgptMode)
        XCTAssertTrue(waiting.isWaitingForChatgpt)
        XCTAssertEqual(waiting.workspaceProject, "/tmp/p")
        XCTAssertEqual(waiting.nextAction, "call goal_check")

        let idle = try XCTUnwrap(try decode(GoalRunOutput.self, #"""
        {"success":true,"output":{"goal_run":{"goal_run_id":"r3","state":"paused",
         "state_reason":"chatgpt_idle: no goal_check","mode":"chatgpt"}}}
        """#).output?.goalRun)
        XCTAssertTrue(idle.isChatgptIdle)
        XCTAssertFalse(idle.isWaitingForChatgpt)
    }

    func testApproveAndStartArgumentsEncodeMode() throws {
        let args = PlanGoalArguments(action: "approve_and_start", planId: "p1", revision: 1,
                                     contentSha256: "abc", mode: "chatgpt")
        let json = String(decoding: try JSONEncoder.chadex.encode(args), as: UTF8.self)
        XCTAssertTrue(json.contains(#""mode":"chatgpt""#), json)
        let none = String(decoding: try JSONEncoder.chadex.encode(PlanGoalArguments(action: "approve")), as: UTF8.self)
        XCTAssertFalse(none.contains("mode"), none)
    }
}
