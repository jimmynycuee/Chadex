import Foundation
import XCTest
@testable import ChadexApp

final class ProtocolModelTests: XCTestCase {
    func testPerformanceTraceAcceptsLegacyAndCorrelatedRecords() throws {
        var record: [String: Any] = [
            "sequence": 1, "started_at_ms": 1000, "methods": ["tools/call"],
            "tool_names": ["read_files"], "request_bytes": 10, "response_bytes": 20,
            "status_code": 200, "ingress_pre_backend_us": 1, "backend_headers_us": 2,
            "response_stream_us": 3, "total_us": 6, "completion": "completed"
        ]
        let legacy = try JSONDecoder.chadex.decode(
            McpPerformanceTraceEntry.self, from: JSONSerialization.data(withJSONObject: record)
        )
        XCTAssertNil(legacy.serverTraceId)
        XCTAssertNil(legacy.requestIdHashes)
        record["finished_at_ms"] = 1001
        record["request_id_hashes"] = [String(repeating: "a", count: 64)]
        record["server_trace_id"] = "b3e8101b-6692-4536-a6d0-3989772a2f51"
        let correlated = try JSONDecoder.chadex.decode(
            McpPerformanceTraceEntry.self, from: JSONSerialization.data(withJSONObject: record)
        )
        XCTAssertEqual(correlated.finishedAtMs, 1001)
        XCTAssertEqual(correlated.requestIdHashes?.first, String(repeating: "a", count: 64))
        XCTAssertEqual(correlated.serverTraceId, record["server_trace_id"] as? String)
    }

    func testBackendSnapshotDecodesTruthfulWaitingState() throws {
        let json = #"""
        {
          "protocol_version": 1,
          "request_id": "r1",
          "result": {
            "phase": "waiting_for_chatgpt_verification",
            "selected_project": null,
            "tunnel_ready": true,
            "chat_gpt_connected": true,
            "chat_gpt_verified_for_selected_project": false,
            "last_verified_at_ms": null,
            "current_operation": null,
            "error": null,
            "activity_sequence": 2,
            "state_revision": 42
          }
        }
        """#.data(using: .utf8)!

        let response = try JSONDecoder.chadex.decode(HelperResponse.self, from: json)
        let result = try XCTUnwrap(response.result)
        let resultData = try JSONEncoder.chadex.encode(result)
        let snapshot = try JSONDecoder.chadex.decode(BackendSnapshot.self, from: resultData)

        XCTAssertEqual(snapshot.phase, .waitingForChatGPTVerification)
        XCTAssertTrue(snapshot.tunnelReady)
        XCTAssertTrue(snapshot.chatGPTConnected)
        XCTAssertFalse(snapshot.chatGPTVerifiedForSelectedProject)
        XCTAssertEqual(snapshot.stateRevision, 42)
    }

    func testBackendSnapshotDecodesPhase9TaskProgressWithoutRawLogs() throws {
        let json = #"""
        {
          "phase": "verified",
          "selected_project": null,
          "tunnel_ready": true,
          "chat_gpt_connected": true,
          "chat_gpt_verified_for_selected_project": true,
          "last_verified_at_ms": 1789846000000,
          "current_operation": null,
          "task_progress": {
            "task_id": "chadex_task_0123456789abcdef0123456789abcdef",
            "project": "agent:test:pricing",
            "goal": "Clamp excessive discount",
            "status": "running",
            "current_step": 3,
            "total_steps": 5,
            "completed_steps": 3,
            "plan": ["search", "read", "edit", "validate", "review"],
            "cancel_requested": false,
            "started_at_ms": 1789846000000,
            "finished_at_ms": null,
            "duration_ms": null,
            "validation": {
              "status": "not_run",
              "checks_passed": 0,
              "checks_failed": 0
            },
            "review": {
              "status": "not_run"
            },
            "steps": [
              {
                "index": 0,
                "kind": "search",
                "status": "completed",
                "duration_ms": 12,
                "attempts": 1,
                "retries": 0
              },
              {
                "index": 2,
                "kind": "edit",
                "status": "completed",
                "duration_ms": 18,
                "attempts": 1,
                "retries": 0
              }
            ]
          },
          "error": null,
          "activity_sequence": 4,
          "state_revision": 43
        }
        """#.data(using: .utf8)!

        let snapshot = try JSONDecoder.chadex.decode(BackendSnapshot.self, from: json)
        let task = try XCTUnwrap(snapshot.taskProgress)

        XCTAssertEqual(task.goal, "Clamp excessive discount")
        XCTAssertEqual(task.status, "running")
        XCTAssertEqual(task.currentStep, 3)
        XCTAssertEqual(task.plan, ["search", "read", "edit", "validate", "review"])
        XCTAssertEqual(task.steps.count, 2)
        XCTAssertTrue(task.canCancel)
        XCTAssertTrue(task.isActive)
        XCTAssertEqual(task.validation.status, "not_run")
        XCTAssertNil(task.review.changedFileCount)
    }

    func testBackendSnapshotAcceptsLegacyRustWaitingPhaseSpelling() throws {
        let json = #"""
        {
          "phase": "waiting_for_chat_gpt_verification",
          "selected_project": null,
          "tunnel_ready": true,
          "chat_gpt_connected": false,
          "chat_gpt_verified_for_selected_project": false,
          "last_verified_at_ms": null,
          "current_operation": null,
          "error": null,
          "activity_sequence": 0,
          "state_revision": 1
        }
        """#.data(using: .utf8)!

        let snapshot = try JSONDecoder.chadex.decode(BackendSnapshot.self, from: json)
        XCTAssertEqual(snapshot.phase, .waitingForChatGPTVerification)
        XCTAssertTrue(snapshot.tunnelReady)
    }

    func testOlderSnapshotCannotSupersedeNewerSnapshot() {
        let newer = BackendSnapshot(
            phase: .verified,
            selectedProject: nil,
            tunnelReady: true,
            chatGPTConnected: true,
            chatGPTVerifiedForSelectedProject: true,
            lastVerifiedAtMs: nil,
            currentOperation: nil,
            error: nil,
            activitySequence: 10,
            stateRevision: 100
        )
        let older = BackendSnapshot(
            phase: .stopped,
            selectedProject: nil,
            tunnelReady: false,
            chatGPTConnected: false,
            chatGPTVerifiedForSelectedProject: false,
            lastVerifiedAtMs: nil,
            currentOperation: nil,
            error: nil,
            activitySequence: 9,
            stateRevision: 99
        )

        XCTAssertFalse(older.isAtLeastAsFresh(as: newer))
        XCTAssertTrue(newer.isAtLeastAsFresh(as: older))
    }

    func testSnapshotRequestGateRejectsOlderRequestFinishingLate() {
        var gate = SnapshotRequestGate()
        let olderRequest = gate.issue()
        let newerRequest = gate.issue()

        XCTAssertTrue(gate.shouldApply(newerRequest))
        XCTAssertFalse(gate.shouldApply(olderRequest))
    }

    func testActivityRefreshGateOnlyRefreshesWhenSequenceChanges() {
        var gate = ActivityRefreshGate()

        XCTAssertTrue(gate.shouldRefresh(for: 0))
        gate.markLoaded(0)
        XCTAssertFalse(gate.shouldRefresh(for: 0))
        XCTAssertTrue(gate.shouldRefresh(for: 1))
        gate.markLoaded(1)
        XCTAssertFalse(gate.shouldRefresh(for: 1))
        gate.reset()
        XCTAssertTrue(gate.shouldRefresh(for: 1))
    }

    func testSnapshotFreshnessGateSkipsOnlyRecentForegroundRefreshes() {
        var gate = SnapshotFreshnessGate()

        XCTAssertTrue(gate.shouldRefresh(at: 100, maxAge: 1.5))
        gate.markApplied(at: 100)
        XCTAssertFalse(gate.shouldRefresh(at: 101.49, maxAge: 1.5))
        XCTAssertTrue(gate.shouldRefresh(at: 101.51, maxAge: 1.5))
        XCTAssertTrue(gate.shouldRefresh(at: 99, maxAge: 1.5))
    }

    func testProjectSwitchGateRejectsOverlappingSwitchesUntilActiveSwitchFinishes() {
        var gate = ProjectSwitchGate()
        let first = UUID()
        let second = UUID()

        XCTAssertTrue(gate.begin(first))
        XCTAssertFalse(gate.begin(second))
        gate.finish(second)
        XCTAssertFalse(gate.begin(second))
        gate.finish(first)
        XCTAssertTrue(gate.begin(second))
    }

    func testProjectSwitchPresentationNeverExposesStaleReadyState() {
        let snapshot = BackendSnapshot(
            phase: .verified,
            selectedProject: nil,
            tunnelReady: true,
            chatGPTConnected: true,
            chatGPTVerifiedForSelectedProject: true,
            lastVerifiedAtMs: nil,
            currentOperation: nil,
            error: nil,
            activitySequence: 1,
            stateRevision: 1
        )

        XCTAssertEqual(
            ConnectionPresentation.phase(
                for: snapshot,
                isBootstrapping: false,
                isSwitchingProject: true
            ),
            .preparing
        )
        XCTAssertNil(
            ConnectionPresentation.error(
                for: snapshot,
                actionError: nil,
                isBootstrapping: false,
                isSwitchingProject: true
            )
        )
    }

    func testPerformanceTraceDecodesWithoutRequestContents() throws {
        let json = #"""
        {
          "sequence": 7,
          "started_at_ms": 1789730000000,
          "methods": ["tools/call"],
          "tool_names": ["read_files"],
          "request_bytes": 512,
          "response_bytes": 2048,
          "status_code": 200,
          "ingress_pre_backend_us": 1200,
          "backend_headers_us": 420000,
          "response_stream_us": 3100,
          "total_us": 424300,
          "completion": "completed"
        }
        """#.data(using: .utf8)!

        let trace = try JSONDecoder.chadex.decode(McpPerformanceTraceEntry.self, from: json)
        XCTAssertEqual(trace.sequence, 7)
        XCTAssertEqual(trace.toolNames, ["read_files"])
        XCTAssertEqual(trace.statusCode, 200)
        XCTAssertEqual(trace.totalMilliseconds, 424.3, accuracy: 0.001)
    }

    func testLifecyclePerformanceTraceDecodesPhaseTiming() throws {
        let json = #"""
        {
          "sequence": 3,
          "started_at_ms": 1789730000000,
          "operation": "connect",
          "phase": "runtime_ensure",
          "total_us": 287500,
          "completion": "completed"
        }
        """#.data(using: .utf8)!

        let trace = try JSONDecoder.chadex.decode(LifecyclePerformanceTraceEntry.self, from: json)
        XCTAssertEqual(trace.operation, "connect")
        XCTAssertEqual(trace.phase, "runtime_ensure")
        XCTAssertEqual(trace.totalMilliseconds, 287.5, accuracy: 0.001)
        XCTAssertEqual(trace.completion, "completed")
    }

    func testBootstrapPresentationSuppressesTransientBackendError() {
        let backendError = HelperErrorPayload(
            code: "runtime_unavailable",
            message: "Chadex local service needs attention",
            recovery: "Retry",
            details: nil
        )
        let snapshot = BackendSnapshot(
            phase: .error,
            selectedProject: nil,
            tunnelReady: false,
            chatGPTConnected: false,
            chatGPTVerifiedForSelectedProject: false,
            lastVerifiedAtMs: nil,
            currentOperation: nil,
            error: backendError,
            activitySequence: 0,
            stateRevision: 1
        )

        XCTAssertEqual(ConnectionPresentation.phase(for: snapshot, isBootstrapping: true), .preparing)
        XCTAssertNil(ConnectionPresentation.error(for: snapshot, actionError: nil, isBootstrapping: true))
    }

    func testPersistentBackendErrorReturnsAfterBootstrap() {
        let backendError = HelperErrorPayload(
            code: "runtime_unavailable",
            message: "Chadex local service needs attention",
            recovery: "Retry",
            details: nil
        )
        let snapshot = BackendSnapshot(
            phase: .error,
            selectedProject: nil,
            tunnelReady: false,
            chatGPTConnected: false,
            chatGPTVerifiedForSelectedProject: false,
            lastVerifiedAtMs: nil,
            currentOperation: nil,
            error: backendError,
            activitySequence: 0,
            stateRevision: 1
        )

        XCTAssertEqual(ConnectionPresentation.phase(for: snapshot, isBootstrapping: false), .error)
        XCTAssertEqual(ConnectionPresentation.error(for: snapshot, actionError: nil, isBootstrapping: false), backendError)
    }

    func testProtocolRequestUsesVersionAndRequestID() throws {
        let request = HelperRequest(requestId: "abc", method: "getStatus", params: .object([:]))
        let data = try JSONEncoder.chadex.encode(request)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertEqual(object["protocol_version"] as? Int, 1)
        XCTAssertEqual(object["request_id"] as? String, "abc")
        XCTAssertEqual(object["method"] as? String, "getStatus")
    }

    func testConnectionPhaseNamesAreStable() throws {
        let snapshot = BackendSnapshot(
            phase: .waitingForChatGPTVerification,
            selectedProject: nil,
            tunnelReady: true,
            chatGPTConnected: true,
            chatGPTVerifiedForSelectedProject: false,
            lastVerifiedAtMs: nil,
            currentOperation: nil,
            error: nil,
            activitySequence: 0,
            stateRevision: 7
        )
        let data = try JSONEncoder.chadex.encode(snapshot)
        let text = try XCTUnwrap(String(data: data, encoding: .utf8))
        XCTAssertTrue(text.contains("waiting_for_chatgpt_verification"))
        XCTAssertTrue(text.contains("chat_gpt_connected"))
        XCTAssertTrue(text.contains("\"state_revision\":7"))
    }
}
