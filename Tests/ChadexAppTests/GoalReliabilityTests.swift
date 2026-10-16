import Foundation
import XCTest
@testable import ChadexApp

final class GoalReliabilityTests: XCTestCase {
    func testBeforeWorkPreflightCanOfferExplicitPreparationRetryOnly() throws {
        func decode(_ reason: String, operations: [[String: Any]] = [], state: String = "needs_attention") throws -> GoalStatusOutput {
            let value: [String: Any] = ["goal_run": ["goal_run_id": "synthetic-preflight", "state": state, "state_reason": reason, "committed_rounds": 0], "operations": operations]
            return try JSONDecoder.chadex.decode(GoalStatusOutput.self, from: JSONSerialization.data(withJSONObject: value))
        }
        let status = try decode("toolchain_preflight_blocked: executable unavailable")
        XCTAssertTrue(status.canRetryPreparation)
        XCTAssertFalse(status.canRetryAcceptance)
        XCTAssertFalse(status.goalRun.canResume)
        XCTAssertFalse(try decode("toolchain_preflight_outcome_unknown").canRetryPreparation)
        XCTAssertFalse(try decode("toolchain_preflight_blocked", operations: [["op_key": "agent:1", "kind": "agent", "state": "succeeded"]]).canRetryPreparation)
        XCTAssertFalse(try decode("toolchain_preflight_blocked", operations: [["op_key": "stage:1", "kind": "commit", "state": "succeeded"]]).canRetryPreparation)
        XCTAssertFalse(try decode("toolchain_preflight_blocked", state: "cancelled").canRetryPreparation)
        let confirmation = PlanGoalConfirmation.retryPreparation(goalRunId: "synthetic-preflight")
        XCTAssertFalse(confirmation.isDestructive)
        XCTAssertTrue(confirmation.id.contains("synthetic-preflight"))
    }

    func testVerifiedCheckpointCanRetryEvenWhenLegacyIterationIsZero() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","state_reason":"runner unavailable","committed_rounds":1,"checkpoint_sha":"0123456789abcdef0123456789abcdef01234567","iteration":0}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let status = try XCTUnwrap(output.output)
        let run = status.goalRun

        XCTAssertFalse(run.canResume)
        XCTAssertTrue(status.canRetryAcceptance)
        XCTAssertFalse(status.canAdoptCheckpoint)
        XCTAssertFalse(run.canPause)
        XCTAssertTrue(run.canCancel)
    }

    func testProgressConsistentGitCheckpointCanOfferConfirmedRetry() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-2","state":"needs_attention","state_reason":"runner unavailable","iteration":1,"committed_rounds":1,"completed_acceptance_rounds":0,"checkpoint_sha":"0123456789abcdef0123456789abcdef01234567"}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let run = try XCTUnwrap(output.output?.goalRun)

        XCTAssertTrue(run.canRetryAcceptance)
    }

    func testLegacyCommittedCheckpointWithoutGitObjectIDRequiresHumanAdoption() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-legacy","state":"needs_attention","state_reason":"runner unavailable","iteration":0,"committed_rounds":1}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let status = try XCTUnwrap(output.output)

        XCTAssertFalse(status.canRetryAcceptance)
        XCTAssertTrue(status.canAdoptCheckpoint)
        XCTAssertTrue(status.goalRun.hasCheckpointProgressMetrics)
    }

    func testInvalidCheckpointGitObjectIDRequiresAdoption() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-legacy","state":"needs_attention","committed_rounds":1,"checkpoint_sha":"6d6e811"}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let status = try XCTUnwrap(output.output)

        XCTAssertFalse(status.canRetryAcceptance)
        XCTAssertTrue(status.canAdoptCheckpoint)
    }

    func testUnknownOutcomeCannotResumeOrRetryEvenWithCheckpoint() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","state_reason":"outcome_unknown: verify op","checkpoint_sha":"0123456789abcdef0123456789abcdef01234567"}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let status = try XCTUnwrap(output.output)

        XCTAssertTrue(status.hasUnknownOutcome)
        XCTAssertFalse(status.canRetryAcceptance)
        XCTAssertFalse(status.canAdoptCheckpoint)
        XCTAssertFalse(status.goalRun.canResume)
    }

    func testUnknownOutcomeCannotUseGeneralResumeWhenRunIsPaused() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"paused","state_reason":"outcome_unknown: operation result unavailable","checkpoint_sha":"0123456789abcdef0123456789abcdef01234567"}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )

        XCTAssertFalse(try XCTUnwrap(output.output?.goalRun).canResume)
    }

    func testUnknownOutcomeEventDisablesRetryWhenStateReasonIsNotSpecific() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","state_reason":"runner unavailable","checkpoint_sha":"0123456789abcdef0123456789abcdef01234567"},"events":[{"seq":7,"kind":"outcome_unknown","detail":{"op":"verify:1:0"}}]}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let status = try XCTUnwrap(output.output)

        XCTAssertTrue(status.hasUnknownOutcome)
        XCTAssertFalse(status.canRetryAcceptance)
    }

    func testStructuredUnknownOutcomeCauseDisablesRetryAndAdoption() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-legacy","state":"needs_attention","committed_rounds":1},"blocking_cause":{"cause":"outcome_unknown"}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let status = try XCTUnwrap(output.output)

        XCTAssertTrue(status.hasUnknownOutcome)
        XCTAssertFalse(status.canRetryAcceptance)
        XCTAssertFalse(status.canAdoptCheckpoint)
    }

    func testMissingCheckpointDoesNotOfferRetry() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","state_reason":"runner unavailable","iteration":0}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let status = try XCTUnwrap(output.output)

        XCTAssertFalse(status.canRetryAcceptance)
        XCTAssertFalse(status.goalRun.hasCheckpointProgressMetrics)
    }

    func testRetryAcceptanceEncodesOnlyTheExplicitRunAction() throws {
        let data = try JSONEncoder.chadex.encode(
            PlanGoalArguments(action: "retry_acceptance", goalRunId: "run-1")
        )
        let arguments = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])

        XCTAssertEqual(arguments["action"] as? String, "retry_acceptance")
        XCTAssertEqual(arguments["goal_run_id"] as? String, "run-1")
        XCTAssertEqual(arguments.count, 2)
    }

    func testAdoptCheckpointEncodesOnlyTheExplicitRunAction() throws {
        let data = try JSONEncoder.chadex.encode(
            PlanGoalArguments(action: "adopt_checkpoint", goalRunId: "run-legacy")
        )
        let arguments = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])

        XCTAssertEqual(arguments["action"] as? String, "adopt_checkpoint")
        XCTAssertEqual(arguments["goal_run_id"] as? String, "run-legacy")
        XCTAssertEqual(arguments.count, 2)
    }

    func testIterationZeroDoesNotCreateProgressEvidence() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","iteration":0,"max_iterations":20}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let run = try XCTUnwrap(output.output?.goalRun)

        XCTAssertEqual(run.iteration, 0)
        XCTAssertFalse(run.hasCheckpointProgressMetrics)
        XCTAssertNil(run.checkpointSha)
    }

    func testAcceptanceEditorLoadsCurrentJSONAndDetectsValidChanges() {
        let current: [JSONValue] = [
            .object(["name": .string("offline"), "argv": .array([.string("swift"), .string("test")])])
        ]
        let requirements: [JSONValue] = [.object([
            "source_path": .string("evidence/private/input.json"),
            "source_sha256": .string(String(repeating: "a", count: 64)),
            "destination_path": .string("fixtures/input.json"),
            "permission": .string("read_only")
        ])]
        let dependencies: [JSONValue] = [.object([
            "path": .string("node_modules"),
            "provenance": .string("package-lock.json")
        ])]
        var draft = GoalAcceptanceEditorDraft(
            currentAcceptance: current,
            workspaceRequirements: requirements,
            dependencies: dependencies
        )

        XCTAssertTrue(draft.isValid)
        XCTAssertFalse(draft.hasChanges)
        XCTAssertFalse(draft.canReview)
        XCTAssertTrue(draft.currentText.contains("\"offline\""))
        XCTAssertTrue(draft.currentText.contains("\"workspace_requirements\""))
        XCTAssertTrue(draft.currentText.contains("\"dependencies\""))

        draft.text = """
        {
          "acceptance": [
            {"name":"offline","argv":["swift","test"]},
            {"name":"format","argv":["swift","format","--lint"]}
          ],
          "workspace_requirements": [
            {"source_path":"evidence/private/input.json","source_sha256":"\(String(repeating: "a", count: 64))","destination_path":"fixtures/revised-input.json","permission":"read_only"}
          ],
          "dependencies": [
            {"path":"node_modules","provenance":"updated package-lock.json"}
          ]
        }
        """

        XCTAssertTrue(draft.isValid)
        XCTAssertTrue(draft.hasChanges)
        XCTAssertTrue(draft.canReview)
        XCTAssertEqual(draft.parsedSpec?.acceptance, [
            .object(["name": .string("offline"), "argv": .array([.string("swift"), .string("test")])]),
            .object(["name": .string("format"), "argv": .array([.string("swift"), .string("format"), .string("--lint")])])
        ])
        XCTAssertEqual(draft.parsedSpec?.workspaceRequirements, [.object([
            "source_path": .string("evidence/private/input.json"),
            "source_sha256": .string(String(repeating: "a", count: 64)),
            "destination_path": .string("fixtures/revised-input.json"),
            "permission": .string("read_only")
        ])])
        XCTAssertEqual(draft.parsedSpec?.dependencies, [.object([
            "path": .string("node_modules"),
            "provenance": .string("updated package-lock.json")
        ])])
        XCTAssertNotEqual(draft.parsedSpec?.workspaceRequirements, requirements)
        XCTAssertNotEqual(draft.parsedSpec?.dependencies, dependencies)
    }

    func testAcceptanceEditorRejectsInvalidJSONWithoutProducingAReviewValue() {
        var draft = GoalAcceptanceEditorDraft(
            currentAcceptance: [.string("check")],
            workspaceRequirements: [],
            dependencies: []
        )
        draft.text = #"{"acceptance":["check"]}"#

        XCTAssertFalse(draft.isValid)
        XCTAssertFalse(draft.hasChanges)
        XCTAssertFalse(draft.canReview)
        XCTAssertNotNil(draft.parsedValue)
        XCTAssertNil(draft.parsedSpec)
    }

    func testAcceptanceEditorRequiresAllManifestArraysAndRejectsUnknownKeys() {
        var draft = GoalAcceptanceEditorDraft(
            currentAcceptance: [], workspaceRequirements: [], dependencies: []
        )
        draft.text = #"{"acceptance":[],"workspace_requirements":[]}"#
        XCTAssertFalse(draft.isValid)
        XCTAssertFalse(draft.canReview)

        draft.text = #"{"acceptance":[],"workspace_requirements":[],"dependencies":[],"budget":{}}"#
        XCTAssertFalse(draft.isValid)
        XCTAssertFalse(draft.canReview)
    }

    func testAcceptanceJSONFormattingPreservesObjectKeys() throws {
        let value = JSONValue.object([
            "executionPolicy": .object(["allowLoopback": .bool(false)]),
            "acceptance": .array([.string("swift test")])
        ])
        let encoded = PlanGoalFormat.json(value)
        let decoded = try JSONDecoder().decode(JSONValue.self, from: Data(encoded.utf8))

        XCTAssertEqual(decoded, value)
        XCTAssertTrue(encoded.contains("\"executionPolicy\""))
        XCTAssertTrue(encoded.contains("\"allowLoopback\""))
    }

    func testCheckpointProgressFieldsDecodeExactStableWireNames() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","iteration":0,"max_iterations":20,"committed_rounds":1,"completed_acceptance_rounds":0,"current_attempt":{"check":"spawn","number":1},"checkpoint_sha":"0123456789abcdef0123456789abcdef01234567","acceptance_revision":2}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let run = try XCTUnwrap(output.output?.goalRun)

        XCTAssertEqual(run.iteration, 0)
        XCTAssertTrue(run.hasCheckpointProgressMetrics)
        XCTAssertEqual(run.committedRounds, 1)
        XCTAssertEqual(run.completedAcceptanceRounds, 0)
        XCTAssertEqual(run.currentAttempt, .object([
            "check": .string("spawn"),
            "number": .number(1)
        ]))
        XCTAssertEqual(run.checkpointSha, "0123456789abcdef0123456789abcdef01234567")
        XCTAssertEqual(run.acceptanceRevision, 2)
    }

    func testGitCheckpointAcceptsSHA1AndGitSHA256ButAcceptanceHashesStaySHA256() {
        XCTAssertTrue(GoalWireHash.isGitObjectID("0123456789abcdef0123456789abcdef01234567"))
        XCTAssertTrue(GoalWireHash.isGitObjectID(String(repeating: "A", count: 64)))
        XCTAssertFalse(GoalWireHash.isGitObjectID("6d6e811"))
        XCTAssertFalse(GoalWireHash.isGitObjectID(String(repeating: "g", count: 40)))
        XCTAssertFalse(GoalWireHash.isSHA256(String(repeating: "a", count: 40)))
        XCTAssertTrue(GoalWireHash.isSHA256(String(repeating: "a", count: 64)))
    }

    func testInitialPlanSpecDecodesBothWorkspaceManifests() throws {
        let json = #"{"acceptance":[],"workspace_requirements":[{"source_path":"evidence/source.json","source_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","destination_path":"fixtures/source.json","permission":"read_only"}],"dependencies":[{"path":"node_modules","provenance":"package-lock.json"}]}"#
        let spec = try JSONDecoder.chadex.decode(PlanSpec.self, from: Data(json.utf8))

        XCTAssertEqual(spec.workspaceRequirements?.first?.sourcePath, "evidence/source.json")
        XCTAssertEqual(spec.workspaceRequirements?.first?.sourceSha256, String(repeating: "a", count: 64))
        XCTAssertEqual(spec.workspaceRequirements?.first?.destinationPath, "fixtures/source.json")
        XCTAssertEqual(spec.workspaceRequirements?.first?.permission, "read_only")
        XCTAssertEqual(spec.dependencies?.first?.path, "node_modules")
        XCTAssertEqual(spec.dependencies?.first?.provenance, "package-lock.json")
    }

    func testAcceptanceCheckDecodesLaneDefaultsAndExecutionMetadata() throws {
        let json = #"[{"name":"offline","argv":["swift","test"],"loopback_ports":[8787],"depends_on":["fixture"],"external_gate":{"owner":"reviewer","action":"sign off","evidence":"signed report","next_step":"attach receipt"}},{"name":"legacy","argv":["true"]}]"#
        let checks = try XCTUnwrap(PlanGoalFormat.acceptanceChecks(
            JSONDecoder().decode([JSONValue].self, from: Data(json.utf8))
        ))

        XCTAssertEqual(checks[0].effectiveLane, "offline_sandbox")
        XCTAssertEqual(checks[0].effectiveKind, "test")
        XCTAssertEqual(checks[0].loopbackPorts, [8787])
        XCTAssertEqual(checks[0].dependsOn, ["fixture"])
        XCTAssertEqual(checks[0].externalGate?.owner, "reviewer")
        XCTAssertEqual(checks[0].externalGate?.action, "sign off")
        XCTAssertEqual(checks[0].externalGate?.evidence, "signed report")
        XCTAssertEqual(checks[0].externalGate?.nextStep, "attach receipt")
        XCTAssertEqual(checks[1].effectiveLane, "offline_sandbox")
        XCTAssertEqual(checks[1].effectiveKind, "test")
    }

    func testStructuredBlockingEvidenceDisplaysKnownFieldsWithoutInferringRepair() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention"},"events":[{"seq":3,"kind":"preflight_blocked","detail":{"blocking_check":"sandbox","executable":"/usr/bin/codex","repair":"configure the runner path","cause":"SPAWN_ENOENT"}}]}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let evidence = try XCTUnwrap(GoalBlockingEvidence(events: output.output?.events))

        XCTAssertEqual(evidence.blockingCheck, .string("sandbox"))
        XCTAssertEqual(evidence.executable, .string("/usr/bin/codex"))
        XCTAssertEqual(evidence.repair, .string("configure the runner path"))
        XCTAssertEqual(evidence.cause, .string("SPAWN_ENOENT"))
    }

    func testDirectBlockingCauseTakesPrecedenceAndEventsFillMissingFields() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention"},"blocking_cause":{"blocking_check":"acceptance","repair":"fix the configured command","cause":"CHECK_FAILED"},"events":[{"seq":3,"kind":"preflight_blocked","detail":{"blocking_check":"older event check","executable":"/usr/bin/codex","repair":"older event repair","cause":"OLD_CAUSE"}}]}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let status = try XCTUnwrap(output.output)
        let evidence = try XCTUnwrap(GoalBlockingEvidence(
            blockingCause: status.blockingCause,
            events: status.events
        ))

        XCTAssertEqual(evidence.blockingCheck, .string("acceptance"))
        XCTAssertEqual(evidence.executable, .string("/usr/bin/codex"))
        XCTAssertEqual(evidence.repair, .string("fix the configured command"))
        XCTAssertEqual(evidence.cause, .string("CHECK_FAILED"))
    }

    func testAcceptanceEditorRequiresVerifiedCheckpointAndSourceRevision() throws {
        let valid = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","checkpoint_sha":"0123456789abcdef0123456789abcdef01234567","acceptance_revision":3},"acceptance":[],"workspace_requirements":[],"dependencies":[]}}"#
        let validOutput = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(valid.utf8)
        )
        XCTAssertTrue(try XCTUnwrap(validOutput.output).canOpenAcceptanceEditor)

        let noCheckpoint = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","acceptance_revision":3},"acceptance":[],"workspace_requirements":[],"dependencies":[]}}"#
        let noCheckpointOutput = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(noCheckpoint.utf8)
        )
        XCTAssertFalse(try XCTUnwrap(noCheckpointOutput.output).canOpenAcceptanceEditor)

        let noRevision = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","checkpoint_sha":"0123456789abcdef0123456789abcdef01234567"},"acceptance":[],"workspace_requirements":[],"dependencies":[]}}"#
        let noRevisionOutput = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(noRevision.utf8)
        )
        XCTAssertFalse(try XCTUnwrap(noRevisionOutput.output).canOpenAcceptanceEditor)
    }

    func testAcceptanceAndApprovalArgumentsEncodeExactWireFields() throws {
        let acceptanceSha256 = String(repeating: "d", count: 64)
        let acceptance: JSONValue = .array([.object(["name": .string("offline")])])
        let requirements: JSONValue = .array([.object([
            "source_path": .string("evidence/input.json"),
            "source_sha256": .string(String(repeating: "a", count: 64)),
            "destination_path": .string("fixtures/input.json"),
            "permission": .string("read_only")
        ])])
        let dependencies: JSONValue = .array([.object([
            "path": .string("node_modules"),
            "provenance": .string("package-lock.json")
        ])])
        let proposal = try JSONEncoder.chadex.encode(PlanGoalArguments(
            action: "propose_acceptance",
            goalRunId: "run-1",
            acceptance: acceptance,
            workspaceRequirements: requirements,
            dependencies: dependencies,
            expectedAcceptanceRevision: 3
        ))
        let proposalObject = try XCTUnwrap(JSONSerialization.jsonObject(with: proposal) as? [String: Any])
        XCTAssertEqual(proposalObject["action"] as? String, "propose_acceptance")
        XCTAssertEqual(proposalObject["goal_run_id"] as? String, "run-1")
        XCTAssertEqual(proposalObject["expected_acceptance_revision"] as? Int, 3)
        let encodedAcceptance = try JSONDecoder().decode(
            JSONValue.self,
            from: JSONSerialization.data(withJSONObject: proposalObject["acceptance"] as Any)
        )
        XCTAssertEqual(encodedAcceptance, acceptance)
        let encodedRequirements = try JSONDecoder().decode(
            JSONValue.self,
            from: JSONSerialization.data(withJSONObject: proposalObject["workspace_requirements"] as Any)
        )
        let encodedDependencies = try JSONDecoder().decode(
            JSONValue.self,
            from: JSONSerialization.data(withJSONObject: proposalObject["dependencies"] as Any)
        )
        XCTAssertEqual(encodedRequirements, requirements)
        XCTAssertEqual(encodedDependencies, dependencies)

        let approval = try JSONEncoder.chadex.encode(PlanGoalArguments(
            action: "approve_acceptance",
            goalRunId: "run-1",
            expectedAcceptanceRevision: 3,
            acceptanceSha256: acceptanceSha256,
            proposalId: "proposal-9"
        ))
        let approvalObject = try XCTUnwrap(JSONSerialization.jsonObject(with: approval) as? [String: Any])
        XCTAssertEqual(approvalObject["action"] as? String, "approve_acceptance")
        XCTAssertEqual(approvalObject["expected_acceptance_revision"] as? Int, 3)
        XCTAssertEqual(approvalObject["acceptance_sha256"] as? String, acceptanceSha256)
        XCTAssertEqual(approvalObject["proposal_id"] as? String, "proposal-9")
        XCTAssertNil(approvalObject["acceptance"])
        XCTAssertNil(approvalObject["workspace_requirements"])
    }

    func testProposalResponseNormalizesServerPendingRevisionAndHashes() throws {
        let json = #"{"success":true,"output":{"proposal_id":"proposal-9","goal_run_id":"run-1","source_acceptance_revision":3,"checkpoint_sha":"0123456789abcdef0123456789abcdef01234567","source_acceptance_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","proposed_acceptance_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","acceptance":[{"name":"offline","lane":"offline_sandbox","kind":"test"}],"workspace_requirements":[{"source_path":"evidence/input.json","source_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","destination_path":"fixtures/input.json","permission":"read_only"}],"dependencies":[{"path":"node_modules","provenance":"package-lock.json"}],"proposed_at_unix_ms":1791968400000}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalAcceptanceProposal>.self,
            from: Data(json.utf8)
        )
        let proposal = try XCTUnwrap(output.output)
        let pending = proposal.pendingAcceptance

        XCTAssertEqual(proposal.proposalId, "proposal-9")
        XCTAssertEqual(proposal.sourceAcceptanceRevision, 3)
        XCTAssertEqual(proposal.checkpointSha, "0123456789abcdef0123456789abcdef01234567")
        XCTAssertEqual(proposal.sourceAcceptanceSha256, String(repeating: "a", count: 64))
        XCTAssertEqual(proposal.proposedAcceptanceSha256, String(repeating: "b", count: 64))
        XCTAssertEqual(pending.proposalId, "proposal-9")
        XCTAssertEqual(pending.revision, 4)
        XCTAssertEqual(pending.specSha256, String(repeating: "b", count: 64))
        XCTAssertEqual(pending.spec?.acceptance, proposal.acceptance)
        XCTAssertEqual(pending.spec?.workspaceRequirements, proposal.workspaceRequirements)
        XCTAssertEqual(pending.spec?.dependencies, proposal.dependencies)
        XCTAssertEqual(pending.state, "proposed")
    }

    func testStatusDecodesNormalizedPendingAcceptanceForReviewAndApproval() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention","checkpoint_sha":"0123456789abcdef0123456789abcdef01234567","acceptance_revision":3},"acceptance":[{"name":"old"}],"workspace_requirements":[{"source_path":"evidence/old.json","source_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","destination_path":"fixtures/old.json","permission":"read_only"}],"dependencies":[{"path":"node_modules","provenance":"package-lock.json"}],"pending_acceptance":{"proposal_id":"proposal-9","revision":4,"spec_sha256":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","spec":{"acceptance":[{"name":"new"}],"workspace_requirements":[{"source_path":"evidence/new.json","source_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","destination_path":"fixtures/new.json","permission":"read_only"}],"dependencies":[{"path":"node_modules","provenance":"updated package-lock.json"}]},"state":"proposed"}}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let pending = try XCTUnwrap(output.output?.pendingAcceptance)

        XCTAssertEqual(pending.proposalId, "proposal-9")
        XCTAssertEqual(pending.revision, 4)
        XCTAssertEqual(pending.specSha256, String(repeating: "c", count: 64))
        XCTAssertEqual(pending.spec?.acceptance?.first, .object(["name": .string("new")]))
        XCTAssertEqual(output.output?.workspaceRequirements?.first, .object([
            "source_path": .string("evidence/old.json"),
            "source_sha256": .string(String(repeating: "a", count: 64)),
            "destination_path": .string("fixtures/old.json"),
            "permission": .string("read_only")
        ]))
        XCTAssertEqual(pending.spec?.workspaceRequirements?.first, .object([
            "source_path": .string("evidence/new.json"),
            "source_sha256": .string(String(repeating: "b", count: 64)),
            "destination_path": .string("fixtures/new.json"),
            "permission": .string("read_only")
        ]))
        XCTAssertEqual(pending.spec?.dependencies?.first, .object([
            "path": .string("node_modules"),
            "provenance": .string("updated package-lock.json")
        ]))
        XCTAssertEqual(pending.state, "proposed")
    }

    func testStatusDecodesAcceptanceRevisionHistory() throws {
        let json = #"{"success":true,"output":{"goal_run":{"goal_run_id":"run-1","state":"needs_attention"},"acceptance_revisions":[{"revision":2,"spec_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","state":"approved"}]}}"#
        let output = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStatusOutput>.self,
            from: Data(json.utf8)
        )
        let revisions = try XCTUnwrap(output.output?.acceptanceRevisions)

        XCTAssertEqual(revisions.count, 1)
        XCTAssertEqual(revisions.first, .object([
            "revision": .number(2),
            "spec_sha256": .string(String(repeating: "a", count: 64)),
            "state": .string("approved")
        ]))
    }
}
