import Foundation
import XCTest
@testable import ChadexApp

final class PlanGoalStartStateTests: XCTestCase {
    private let projectScopeID = UUID(uuidString: "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE")!
    private var key: GoalStartKey {
        GoalStartKey(
            projectScopeID: projectScopeID,
            projectId: "project-a",
            planId: "plan-1",
            revision: 4,
            contentSha256: "sha-4"
        )
    }

    private func run(
        id: String = "run-1",
        planId: String = "plan-1",
        revision: Int = 4,
        hash: String = "sha-4",
        projectId: String = "project-a",
        mode: String = "chatgpt",
        state: String = "queued",
        updatedAt: Int? = nil
    ) throws -> GoalRun {
        let updatedAtField = updatedAt.map { ",\n          \"updated_at_unix_ms\": \($0)" } ?? ""
        let json = """
        {
          "goal_run_id": "\(id)",
          "plan_id": "\(planId)",
          "revision": \(revision),
          "content_sha256": "\(hash)",
          "project": "\(projectId)",
          "mode": "\(mode)",
          "state": "\(state)"\(updatedAtField)
        }
        """
        return try JSONDecoder.chadex.decode(GoalRun.self, from: Data(json.utf8))
    }

    private func consumedDecision(
        planId: String = "plan-1",
        revision: Int = 4,
        hash: String = "sha-4",
        projectId: String = "project-a",
        runId: String? = "run-1"
    ) -> PlanDecision {
        PlanDecision(
            approvalId: "approval-1",
            planId: planId,
            revision: revision,
            contentSha256: hash,
            state: "consumed",
            binding: PlanDecisionBinding(
                projectId: projectId,
                runnerClientId: nil,
                projectRoot: nil,
                baseCommit: nil
            ),
            approver: nil,
            channel: nil,
            reason: nil,
            createdAtUnixMs: nil,
            expiresAtUnixMs: nil,
            consumedByGoalRunId: runId
        )
    }

    func testReservationBlocksBothModesBeforeConfirmationAndSubmissionIsSingleFlight() {
        var ledger = GoalStartLedger()

        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .starting
        )
        XCTAssertFalse(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))

        let otherProjectKey = GoalStartKey(
            projectScopeID: UUID(),
            projectId: "project-b",
            planId: key.planId,
            revision: key.revision,
            contentSha256: key.contentSha256
        )
        XCTAssertEqual(
            ledger.presentation(
                for: otherProjectKey, projectId: "project-b", decisions: [], runs: []
            ),
            .available
        )
        XCTAssertTrue(ledger.beginSubmission(key))
        XCTAssertFalse(ledger.beginSubmission(key))
        XCTAssertFalse(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
    }

    func testCancellingConfirmationReleasesOnlyTheUnsubmittedReservation() {
        var ledger = GoalStartLedger()
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))

        ledger.cancelConfirmation(key)
        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .available
        )
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))

        XCTAssertTrue(ledger.beginSubmission(key))
        ledger.cancelConfirmation(key)
        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .starting
        )
    }

    func testSuccessfulStartKeepsActualRunVisibleEvenWithoutRefreshData() throws {
        var ledger = GoalStartLedger()
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertTrue(ledger.beginSubmission(key))
        let startedRun = try run(mode: "codex", state: "running")
        ledger.complete(key, run: startedRun, runId: startedRun.goalRunId)

        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .existingRun(startedRun)
        )
        XCTAssertFalse(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
    }

    func testNewerDurablePollStateSupersedesCachedStartedRun() throws {
        var ledger = GoalStartLedger()
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertTrue(ledger.beginSubmission(key))
        let cachedRun = try run(mode: "codex", state: "running", updatedAt: 100)
        ledger.complete(key, run: cachedRun, runId: cachedRun.goalRunId)

        let polledRun = try run(mode: "codex", state: "paused", updatedAt: 200)
        XCTAssertEqual(
            ledger.presentation(
                for: key, projectId: "project-a", decisions: [], runs: [polledRun]
            ),
            .existingRun(polledRun)
        )
    }

    func testUnknownTransportOutcomeStaysBlockedUntilAuthoritativeReconciliation() {
        var ledger = GoalStartLedger()
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertTrue(ledger.beginSubmission(key))
        ledger.recordFailure(key, kind: nil)

        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .checkingStatus
        )
        XCTAssertFalse(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))

        ledger.reconcileUnknown(
            key, projectId: "project-a", decisions: [], runs: [], authoritative: false
        )
        ledger.resetForProjectChange()
        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .checkingStatus
        )

        ledger.reconcileUnknown(
            key, projectId: "project-a", decisions: [], runs: [], authoritative: true
        )
        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .available
        )
    }

    func testProjectSwitchKeepsAnInFlightSubmissionBlockedAsUnknown() {
        var ledger = GoalStartLedger()
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertTrue(ledger.beginSubmission(key))
        let unconfirmedKey = GoalStartKey(
            projectScopeID: projectScopeID,
            projectId: "project-a",
            planId: "plan-2",
            revision: 1,
            contentSha256: "sha-1"
        )
        XCTAssertTrue(ledger.reserve(unconfirmedKey, projectId: "project-a", decisions: [], runs: []))

        ledger.resetForProjectChange()

        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .checkingStatus
        )
        XCTAssertFalse(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertEqual(
            ledger.presentation(
                for: unconfirmedKey, projectId: "project-a", decisions: [], runs: []
            ),
            .available
        )
    }

    func testOutcomeUnknownFailureKindKeepsReservationChecking() {
        var ledger = GoalStartLedger()
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertTrue(ledger.beginSubmission(key))

        ledger.recordFailure(key, kind: "outcome_unknown")

        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .checkingStatus
        )
        XCTAssertFalse(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
    }

    func testUnknownTransportOutcomeReconcilesToDurableRunState() throws {
        var ledger = GoalStartLedger()
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertTrue(ledger.beginSubmission(key))
        ledger.markOutcomeUnknown(key)

        let durableRun = try run(mode: "chatgpt", state: "succeeded", updatedAt: 300)
        ledger.reconcileUnknown(
            key, projectId: "project-a", decisions: [], runs: [durableRun], authoritative: true
        )
        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .existingRun(durableRun)
        )
    }

    func testExplicitBackendRefusalReleasesTheStartReservation() {
        var ledger = GoalStartLedger()
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
        XCTAssertTrue(ledger.beginSubmission(key))
        ledger.recordFailure(key, kind: "stale_revision")
        XCTAssertEqual(
            ledger.presentation(for: key, projectId: "project-a", decisions: [], runs: []),
            .available
        )
        XCTAssertTrue(ledger.reserve(key, projectId: "project-a", decisions: [], runs: []))
    }

    func testConsumedDecisionAndDurableRunRestoreStartedPresentation() throws {
        let decision = consumedDecision()
        let durableState = GoalStartLedger().presentation(
            for: key, projectId: "project-a", decisions: [decision], runs: []
        )
        XCTAssertEqual(durableState, .existingRunID("run-1"))

        let startedRun = try run(mode: "chatgpt", state: "queued")
        let durableRunState = GoalStartLedger().presentation(
            for: key, projectId: "project-a", decisions: [], runs: [startedRun]
        )
        XCTAssertEqual(durableRunState, .existingRun(startedRun))

        var ledger = GoalStartLedger()
        XCTAssertFalse(ledger.reserve(key, projectId: "project-a", decisions: [decision], runs: []))
        XCTAssertFalse(ledger.reserve(key, projectId: "project-a", decisions: [], runs: [startedRun]))
    }

    func testForeignProjectRunAndDifferentRevisionDoNotBlockStart() throws {
        let foreignRun = try run(projectId: "project-b")
        XCTAssertEqual(
            GoalStartLedger().presentation(
                for: key, projectId: "project-a", decisions: [], runs: [foreignRun]
            ),
            .available
        )

        let foreignDecision = consumedDecision(projectId: "project-b")
        XCTAssertEqual(
            GoalStartLedger().presentation(
                for: key, projectId: "project-a", decisions: [foreignDecision], runs: []
            ),
            .available
        )

        let priorRevision = consumedDecision(revision: 3, hash: "sha-3")
        XCTAssertEqual(
            GoalStartLedger().presentation(
                for: key, projectId: "project-a", decisions: [priorRevision], runs: []
            ),
            .available
        )
    }

    func testApproveAndStartOutputDecodesExistingDecisionRunAndCreatedFields() throws {
        let json = """
        {
          "success": true,
          "output": {
            "decision": {
              "approval_id": "approval-1",
              "plan_id": "plan-1",
              "revision": 4,
              "content_sha256": "sha-4",
              "state": "consumed",
              "binding": { "project_id": "project-a" },
              "consumed_by_goal_run_id": "run-1"
            },
            "goal_run": {
              "goal_run_id": "run-1",
              "plan_id": "plan-1",
              "revision": 4,
              "content_sha256": "sha-4",
              "project": "project-a",
              "mode": "chatgpt",
              "state": "queued"
            },
            "created": true
          }
        }
        """
        let envelope = try JSONDecoder.chadex.decode(
            PlanGoalEnvelope<GoalStartOutput>.self,
            from: Data(json.utf8)
        )
        let output = try XCTUnwrap(envelope.output)
        XCTAssertEqual(output.decision?.consumedByGoalRunId, "run-1")
        XCTAssertEqual(output.goalRun?.mode, "chatgpt")
        XCTAssertEqual(output.goalRun?.state, "queued")
        XCTAssertEqual(output.created, true)
    }
}
