import Foundation
import XCTest
@testable import ChadexApp

final class ProtocolModelTests: XCTestCase {
    func testComputerSafetyStatusDecodesModesApprovalsAndAuditWithoutSensitivePayloads() throws {
        let json = #"""
        {
          "mode": "ask_before_control",
          "stopped": false,
          "generation": 12,
          "pending_approvals": [{
            "approval_id": "computer_approval_1",
            "action": "input_text",
            "created_at_ms": 1791100000000
          }],
          "audit": [{
            "sequence": 9,
            "timestamp_ms": 1791100000001,
            "event": "approval_requested",
            "action": "input_text",
            "reason": null
          }]
        }
        """#.data(using: .utf8)!

        let status = try JSONDecoder.chadex.decode(ComputerSafetyStatus.self, from: json)
        XCTAssertEqual(status.mode, .askBeforeControl)
        XCTAssertFalse(status.stopped)
        XCTAssertEqual(status.generation, 12)
        XCTAssertEqual(status.pendingApprovals.first?.action, "input_text")
        XCTAssertEqual(status.audit.first?.event, "approval_requested")
        XCTAssertFalse(json.contains(Data("secret text".utf8)))
        XCTAssertFalse(json.contains(Data("clipboard contents".utf8)))
    }

    func testProjectInstructionsInspectionDecodesEffectiveHierarchy() throws {
        let json = #"""
        {
          "target_path": "/tmp/demo/subproject",
          "status": "available",
          "reason_code": null,
          "projection_status": "loaded",
          "sources": [
            {
              "path": "AGENTS.md",
              "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              "truncated": false,
              "headings": ["# Root"],
              "content": "root rule"
            },
            {
              "path": "subproject/AGENTS.md",
              "fingerprint": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
              "truncated": false,
              "headings": ["# Nested"],
              "content": "nested rule"
            }
          ],
          "changed_sources": [],
          "truncated": false,
          "total_chars": 20,
          "content_included": true
        }
        """#.data(using: .utf8)!

        let inspection = try JSONDecoder.chadex.decode(ProjectInstructionsInspection.self, from: json)
        XCTAssertEqual(inspection.targetPath, "/tmp/demo/subproject")
        XCTAssertTrue(inspection.isAvailable)
        XCTAssertEqual(inspection.sources.map(\.path), ["AGENTS.md", "subproject/AGENTS.md"])
        XCTAssertTrue(inspection.effectiveContent.contains("# AGENTS.md"))
        XCTAssertTrue(inspection.effectiveContent.contains("# subproject/AGENTS.md"))
        XCTAssertTrue(inspection.effectiveContent.contains("nested rule"))
    }

    func testSkillCatalogInventoryAndLazyDefinitionDecode() throws {
        let catalogJSON = #"""
        {
          "project": "agent:test:demo",
          "catalog_revision": "wc_skillcat_example",
          "total_count": 2,
          "returned_count": 2,
          "skills": [
            {
              "skill_id": "wc_skill_project",
              "name": "project-flow",
              "description": "Project guidance",
              "definition_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
              "package_revision": null,
              "source_scope": "project",
              "trust": "project_content",
              "name_conflict": false
            },
            {
              "skill_id": "wc_skill_managed",
              "name": "managed-flow",
              "description": "Installed guidance",
              "definition_revision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
              "package_revision": "wc_skillpkg_example",
              "source_scope": "runner",
              "trust": "operator_installed_guidance",
              "name_conflict": false
            }
          ],
          "invalid_count": 0,
          "diagnostics": [],
          "discovery_truncated": false
        }
        """#.data(using: .utf8)!
        let inventoryJSON = #"""
        {
          "project": "agent:test:demo",
          "total_count": 1,
          "skills": [{
            "skill_id": "wc_skill_managed",
            "skill_key": "managed-flow",
            "state_revision": "wc_skillstate_example",
            "active_package_revision": null,
            "preferred_package_revision": "wc_skillpkg_example",
            "definition_revision": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "name": "managed-flow",
            "description": "Installed guidance",
            "total_versions": 2
          }]
        }
        """#.data(using: .utf8)!
        let definitionJSON = #"""
        {
          "skill_id": "wc_skill_project",
          "definition_revision": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
          "package_revision": null,
          "text": "PRIVATE_DEFINITION_BODY",
          "has_more": false
        }
        """#.data(using: .utf8)!

        let catalog = try JSONDecoder.chadex.decode(SkillCatalogInspection.self, from: catalogJSON)
        let inventory = try JSONDecoder.chadex.decode(SkillInventoryInspection.self, from: inventoryJSON)
        let definition = try JSONDecoder.chadex.decode(SkillDefinitionPreview.self, from: definitionJSON)
        XCTAssertEqual(catalog.skills.count, 2)
        XCTAssertEqual(catalog.skills[0].trust, "project_content")
        XCTAssertFalse(inventory.skills[0].isActive)
        XCTAssertEqual(inventory.skills[0].preferredPackageRevision, "wc_skillpkg_example")
        XCTAssertEqual(definition.text, "PRIVATE_DEFINITION_BODY")
        XCTAssertFalse(catalogJSON.contains(Data("PRIVATE_DEFINITION_BODY".utf8)))
    }

    func testProjectMemoryCatalogAndLazyRecordDecode() throws {
        let catalogJSON = #"""
        {
          "project": "agent:test:demo",
          "catalog_revision": "wc_memcat_example",
          "total_count": 1,
          "returned_count": 1,
          "memories": [{
            "memory_id": "wc_mem_example123456",
            "memory_key": "architecture.runtime-routing",
            "summary": "Keep runtime routing project-scoped.",
            "priority": "high",
            "bootstrap": true,
            "tags": ["architecture", "runtime"],
            "revision": "wc_memrev_example"
          }]
        }
        """#.data(using: .utf8)!
        let recordJSON = #"""
        {
          "project": "agent:test:demo",
          "memory_id": "wc_mem_example123456",
          "memory_key": "architecture.runtime-routing",
          "summary": "Keep runtime routing project-scoped.",
          "body": "PRIVATE_MEMORY_BODY",
          "priority": "high",
          "bootstrap": true,
          "tags": ["architecture", "runtime"],
          "revision": "wc_memrev_example",
          "created_at_unix_ms": 1700000000000,
          "updated_at_unix_ms": 1700000001000,
          "provenance": {
            "created_by_kind": "dev",
            "updated_by_kind": "shared-key"
          }
        }
        """#.data(using: .utf8)!

        let catalog = try JSONDecoder.chadex.decode(ProjectMemoryCatalog.self, from: catalogJSON)
        let record = try JSONDecoder.chadex.decode(ProjectMemoryRecord.self, from: recordJSON)
        XCTAssertEqual(catalog.memories.count, 1)
        XCTAssertEqual(catalog.memories[0].category, .architecture)
        XCTAssertEqual(catalog.memories[0].priority, "high")
        XCTAssertTrue(catalog.memories[0].bootstrap)
        XCTAssertEqual(record.body, "PRIVATE_MEMORY_BODY")
        XCTAssertEqual(record.provenance.updatedByKind, "shared-key")
        XCTAssertFalse(catalogJSON.contains(Data("PRIVATE_MEMORY_BODY".utf8)))
    }

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

    func testSnapshotRequestGateAcceptsOlderRequestWhenBackendRevisionIsNewer() {
        var gate = SnapshotRequestGate()
        let connectRequest = gate.issue()
        let pollingRequest = gate.issue()

        XCTAssertTrue(gate.shouldApply(
            pollingRequest,
            candidateRevision: 4,
            currentRevision: 3
        ))
        XCTAssertTrue(gate.shouldApply(
            connectRequest,
            candidateRevision: 5,
            currentRevision: 4
        ))
        XCTAssertFalse(gate.shouldApply(
            connectRequest,
            candidateRevision: 4,
            currentRevision: 4
        ))
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

    func testConnectionSettingsSaveConnectPolicyCoversFirstSetupWithoutReconnectingIntentionalStops() {
        XCTAssertTrue(AppModel.shouldConnectAfterSavingConnectionSettings(
            hadUsableCredentials: false,
            hadActiveConnection: false,
            hasSelectedProject: true
        ))
        XCTAssertTrue(AppModel.shouldConnectAfterSavingConnectionSettings(
            hadUsableCredentials: true,
            hadActiveConnection: true,
            hasSelectedProject: true
        ))
        XCTAssertFalse(AppModel.shouldConnectAfterSavingConnectionSettings(
            hadUsableCredentials: true,
            hadActiveConnection: false,
            hasSelectedProject: true
        ))
        XCTAssertFalse(AppModel.shouldConnectAfterSavingConnectionSettings(
            hadUsableCredentials: false,
            hadActiveConnection: false,
            hasSelectedProject: false
        ))
    }

    func testConnectingSuppressesStaleBackendErrorUntilAttemptFinishes() {
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

        XCTAssertEqual(
            ConnectionPresentation.phase(
                for: snapshot,
                isBootstrapping: false,
                isConnecting: true
            ),
            .preparing
        )
        XCTAssertNil(
            ConnectionPresentation.error(
                for: snapshot,
                actionError: nil,
                isBootstrapping: false,
                isConnecting: true
            )
        )
        XCTAssertEqual(ConnectionPresentation.phase(for: snapshot, isBootstrapping: false), .error)
        XCTAssertEqual(ConnectionPresentation.error(for: snapshot, actionError: nil, isBootstrapping: false), backendError)
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
