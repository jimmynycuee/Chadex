import Foundation

/// State and actions of the Plan & Goal operator panel. This app is the only
/// trusted approval path: every approval sends the exact revision and hash the
/// person was looking at, and the backend rejects anything stale.
@MainActor
final class PlanGoalPanelModel: ObservableObject {
    enum Selection: Equatable, Hashable {
        case plan(String)
        case run(String)
    }

    static let pollInterval: TimeInterval = 5

    @Published private(set) var overview: GoalOverviewOutput?
    @Published private(set) var overviewError: String?
    @Published private(set) var loading = false
    @Published private(set) var selection: Selection?
    @Published private(set) var planDetail: PlanInspectOutput?
    @Published private(set) var runDetail: GoalStatusOutput?
    @Published private(set) var detailError: String?
    @Published private(set) var actionError: String?
    @Published private(set) var actionInFlight = false
    @Published private(set) var goalStartLedger = GoalStartLedger()
    @Published private(set) var emergencyInFlight = false
    /// Revision the person picked; nil follows the latest one.
    @Published private(set) var viewedRevision: Int?

    private weak var app: AppModel?
    private var boundProjectID: UUID?
    private var refreshInFlight = false
    private var refreshPending = false

    var emergencyStopEngaged: Bool { overview?.emergencyStopEngaged ?? false }

    func attach(_ app: AppModel) {
        self.app = app
        app.attachPlanGoalPanel(self)
    }

    /// Re-read the durable board after runtime readiness or a connection-state
    /// transition. The stored plans and approvals remain authoritative.
    func reloadExistingData() async {
        await refresh()
    }

    /// Binds the panel to a project; switching projects drops all state so a
    /// late response for the previous project can never be shown.
    func bind(projectID: UUID) {
        guard boundProjectID != projectID else { return }
        boundProjectID = projectID
        overview = nil
        overviewError = nil
        selection = nil
        planDetail = nil
        runDetail = nil
        detailError = nil
        actionError = nil
        viewedRevision = nil
        var ledger = goalStartLedger
        ledger.resetForProjectChange()
        goalStartLedger = ledger
    }

    func select(_ newSelection: Selection?) async {
        guard selection != newSelection else { return }
        selection = newSelection
        planDetail = nil
        runDetail = nil
        detailError = nil
        actionError = nil
        viewedRevision = nil
        await refresh()
    }

    /// Tapping the open card again closes its detail.
    func toggle(_ item: Selection) async {
        await select(selection == item ? nil : item)
    }

    func selectRevision(_ revision: Int?) async {
        guard viewedRevision != revision else { return }
        viewedRevision = revision
        await refresh()
    }

    // MARK: - Refresh

    /// Overview plus the open detail. Never overlaps: a call during a running
    /// refresh schedules exactly one more pass so a just-made change is read.
    func refresh() async {
        if refreshInFlight {
            refreshPending = true
            return
        }
        guard let app, let projectID = app.selectedProject?.id, boundProjectID == projectID else { return }
        refreshInFlight = true
        loading = true
        defer {
            refreshInFlight = false
            loading = false
        }
        repeat {
            refreshPending = false
            await refreshOnce(app: app, projectID: projectID)
        } while refreshPending && !Task.isCancelled && app.selectedProject?.id == projectID
    }

    private func isCurrent(_ projectID: UUID, _ app: AppModel) -> Bool {
        boundProjectID == projectID && app.selectedProject?.id == projectID
    }

    private func refreshOnce(app: AppModel, projectID: UUID) async {
        let overviewResult = await app.planGoalOperator(
            tool: "goal_operator",
            arguments: PlanGoalArguments(action: "overview"),
            as: GoalOverviewOutput.self
        )
        guard isCurrent(projectID, app) else { return }
        var overviewReadSucceeded = false
        var refreshedRuns: [GoalRun] = []
        switch overviewResult {
        case .success(let value):
            overviewReadSucceeded = true
            refreshedRuns = value.goalRuns ?? []
            var ledger = goalStartLedger
            ledger.reconcileUnknownRuns(projectScopeID: projectID, runs: refreshedRuns)
            goalStartLedger = ledger
            overview = value
            overviewError = nil
        case .failure(let message, _):
            overviewError = message
        }

        guard let requested = selection else { return }
        let requestedRevision = viewedRevision
        switch requested {
        case .plan(let planId):
            let result = await app.planGoalOperator(
                tool: "plan_review",
                arguments: PlanGoalArguments(action: "inspect", planId: planId, revision: requestedRevision),
                as: PlanInspectOutput.self
            )
            guard isCurrent(projectID, app), selection == requested, viewedRevision == requestedRevision else { return }
            switch result {
            case .success(let value):
                if overviewReadSucceeded,
                   let revision = value.revision,
                   requestedRevision == nil || requestedRevision == revision.revision {
                    let key = GoalStartKey(
                        projectScopeID: projectID,
                        projectId: value.plan.projectId,
                        planId: value.plan.planId,
                        revision: revision.revision,
                        contentSha256: revision.contentSha256
                    )
                    var ledger = goalStartLedger
                    ledger.reconcileUnknown(
                        key,
                        projectId: value.plan.projectId,
                        decisions: value.decisions ?? [],
                        runs: refreshedRuns,
                        authoritative: true
                    )
                    goalStartLedger = ledger
                }
                planDetail = value
                detailError = nil
            case .failure(let message, _):
                detailError = message
            }
        case .run(let runId):
            let result = await app.planGoalOperator(
                tool: "goal_operator",
                arguments: PlanGoalArguments(action: "status", goalRunId: runId),
                as: GoalStatusOutput.self
            )
            guard isCurrent(projectID, app), selection == requested else { return }
            switch result {
            case .success(let value):
                runDetail = value
                detailError = nil
            case .failure(let message, _):
                detailError = message
            }
        }
    }

    // MARK: - Plan actions

    /// Sends exactly the revision and hash that were displayed and confirmed.
    @discardableResult
    func approve(planId: String, revision: Int, contentSha256: String) async -> Bool {
        await act(tool: "plan_review", PlanGoalArguments(
            action: "approve", planId: planId, revision: revision, contentSha256: contentSha256
        ))
    }

    /// Approves the displayed revision and starts its Goal in one step.
    @discardableResult
    func approveAndStart(planId: String, revision: Int, contentSha256: String, mode: String) async -> Bool {
        guard !actionInFlight,
              let app,
              let projectID = app.selectedProject?.id,
              boundProjectID == projectID else { return false }
        var ledger = goalStartLedger
        guard let key = ledger.pendingKey(
            projectScopeID: projectID,
            planId: planId,
            revision: revision,
            contentSha256: contentSha256
        ) else { return false }
        guard ledger.beginSubmission(key) else { return false }
        goalStartLedger = ledger

        actionInFlight = true
        actionError = nil
        defer { actionInFlight = false }
        let result = await app.planGoalOperator(
            tool: "plan_review",
            arguments: PlanGoalArguments(
                action: "approve_and_start", planId: planId, revision: revision,
                contentSha256: contentSha256, mode: mode
            ),
            as: GoalStartOutput.self
        )
        guard isCurrent(projectID, app) else { return false }

        var succeeded = false
        switch result {
        case .success(let output):
            let run = output.goalRun.flatMap {
                GoalStartLedger.matches($0, key: key, projectId: planDetail?.plan.projectId) ? $0 : nil
            }
            ledger = goalStartLedger
            ledger.complete(key, run: run, runId: run?.goalRunId ?? output.decision?.consumedByGoalRunId)
            goalStartLedger = ledger
            if let run { recordStartedRun(run) }
            succeeded = true
        case .failure(let message, let kind):
            ledger = goalStartLedger
            ledger.recordFailure(key, kind: kind)
            goalStartLedger = ledger
            actionError = message
        }
        await refresh()
        return succeeded
    }

    func goalStartPresentation(
        planId: String,
        revision: Int,
        contentSha256: String,
        projectId: String?,
        decisions: [PlanDecision]
    ) -> GoalStartPresentation {
        guard let projectScopeID = boundProjectID else { return .available }
        return goalStartLedger.presentation(
            for: GoalStartKey(
                projectScopeID: projectScopeID,
                projectId: projectId,
                planId: planId,
                revision: revision,
                contentSha256: contentSha256
            ),
            projectId: projectId,
            decisions: decisions,
            runs: overview?.goalRuns ?? []
        )
    }

    /// Reserves the revision synchronously on button tap, before the
    /// confirmation alert is shown, so either mode blocks the other at once.
    func reserveGoalStart(
        planId: String,
        revision: Int,
        contentSha256: String,
        projectId: String?,
        decisions: [PlanDecision]
    ) -> Bool {
        guard !actionInFlight,
              let app,
              let selectedProjectID = app.selectedProject?.id,
              boundProjectID == selectedProjectID else { return false }
        let key = GoalStartKey(
            projectScopeID: selectedProjectID,
            projectId: projectId,
            planId: planId,
            revision: revision,
            contentSha256: contentSha256
        )
        var ledger = goalStartLedger
        guard ledger.reserve(key, projectId: projectId, decisions: decisions, runs: overview?.goalRuns ?? []) else {
            return false
        }
        goalStartLedger = ledger
        return true
    }

    func cancelGoalStartConfirmation(planId: String, revision: Int, contentSha256: String) {
        guard let projectScopeID = boundProjectID else { return }
        var ledger = goalStartLedger
        ledger.cancelConfirmation(
            projectScopeID: projectScopeID,
            planId: planId,
            revision: revision,
            contentSha256: contentSha256
        )
        goalStartLedger = ledger
    }

    private func recordStartedRun(_ run: GoalRun) {
        var current = overview ?? GoalOverviewOutput(
            plans: nil, goalRuns: nil, emergencyStopEngaged: nil, planModeLocks: nil, supervisorRunning: nil
        )
        var runs = current.goalRuns ?? []
        runs.removeAll { $0.goalRunId == run.goalRunId }
        runs.insert(run, at: 0)
        current.goalRuns = runs
        overview = current
    }

    @discardableResult
    func requestChanges(planId: String, revision: Int, contentSha256: String, reason: String) async -> Bool {
        await act(tool: "plan_review", PlanGoalArguments(
            action: "reject", planId: planId, revision: revision, contentSha256: contentSha256, reason: reason
        ))
    }

    @discardableResult
    func revoke(approvalId: String) async -> Bool {
        await act(tool: "plan_review", PlanGoalArguments(action: "revoke", approvalId: approvalId))
    }

    @discardableResult
    func closePlan(planId: String, reason: String? = nil) async -> Bool {
        await act(tool: "plan_review", PlanGoalArguments(action: "close", planId: planId, reason: reason))
    }

    /// Removes the plan from the lists (it is closed; its record is kept).
    @discardableResult
    func deletePlan(planId: String) async -> Bool {
        let deleted = await closePlan(planId: planId, reason: "deleted in Chadex app")
        if deleted, selection == .plan(planId) { await select(nil) }
        return deleted
    }

    // MARK: - Goal actions

    /// Proposes all three approval-bearing Goal spec arrays. The returned
    /// server record is shown for full comparison before separate approval.
    @discardableResult
    func proposeAcceptance(
        goalRunId: String,
        expectedAcceptanceRevision: Int,
        acceptance: [JSONValue],
        workspaceRequirements: [JSONValue],
        dependencies: [JSONValue]
    ) async -> GoalAcceptanceProposal? {
        guard !actionInFlight,
              expectedAcceptanceRevision >= 0,
              expectedAcceptanceRevision < Int.max,
              let app,
              let projectID = app.selectedProject?.id,
              boundProjectID == projectID else { return nil }

        actionInFlight = true
        actionError = nil
        defer { actionInFlight = false }
        let result = await app.planGoalOperator(
            tool: "goal_operator",
            arguments: PlanGoalArguments(
                action: "propose_acceptance",
                goalRunId: goalRunId,
                acceptance: .array(acceptance),
                workspaceRequirements: .array(workspaceRequirements),
                dependencies: .array(dependencies),
                expectedAcceptanceRevision: expectedAcceptanceRevision
            ),
            as: GoalAcceptanceProposal.self
        )
        guard isCurrent(projectID, app) else { return nil }

        let proposal: GoalAcceptanceProposal?
        switch result {
        case .success(let output):
            let isBoundToRun = output.goalRunId == goalRunId
            let hasServerEvidence = !output.proposalId.isEmpty
                && GoalWireHash.isSHA256(output.proposedAcceptanceSha256)
                && GoalWireHash.isGitObjectID(output.checkpointSha)
                && GoalWireHash.isSHA256(output.sourceAcceptanceSha256)
                && output.sourceAcceptanceRevision >= 0
                && output.sourceAcceptanceRevision == expectedAcceptanceRevision
                && output.proposedAtUnixMs >= 0
            proposal = isBoundToRun && hasServerEvidence ? output : nil
            if proposal == nil {
                actionError = L10n.string("planGoal.acceptanceEditor.invalidProposal")
            }
        case .failure(let message, _):
            proposal = nil
            actionError = message
        }
        await refresh()
        return proposal
    }

    /// Approves only the displayed proposal, fenced by its original revision.
    @discardableResult
    func approveAcceptance(
        goalRunId: String,
        expectedAcceptanceRevision: Int,
        acceptanceSha256: String,
        proposalId: String
    ) async -> Bool {
        guard expectedAcceptanceRevision >= 0,
              expectedAcceptanceRevision < Int.max,
              GoalWireHash.isSHA256(acceptanceSha256),
              !proposalId.isEmpty else { return false }
        return await act(tool: "goal_operator", PlanGoalArguments(
            action: "approve_acceptance",
            goalRunId: goalRunId,
            expectedAcceptanceRevision: expectedAcceptanceRevision,
            acceptanceSha256: acceptanceSha256,
            proposalId: proposalId
        ))
    }

    /// Removes a finished run from the lists; its record and branch are kept.
    @discardableResult
    func deleteRun(goalRunId: String) async -> Bool {
        let deleted = await act(tool: "goal_operator", PlanGoalArguments(action: "dismiss", goalRunId: goalRunId))
        if deleted, selection == .run(goalRunId) { await select(nil) }
        return deleted
    }

    /// Explicitly clears the board for the selected project. The helper resolves
    /// the selected path to its exact full runtime project ID before dispatch.
    @discardableResult
    func clearBoard() async -> Bool {
        guard let app, let projectID = app.selectedProject?.id,
              boundProjectID == projectID else { return false }
        return await act(tool: "goal_operator", PlanGoalArguments(action: "clear_board"))
    }

    @discardableResult
    func pause(goalRunId: String) async -> Bool {
        await act(tool: "goal_operator", PlanGoalArguments(action: "pause", goalRunId: goalRunId))
    }

    @discardableResult
    func resume(goalRunId: String) async -> Bool {
        await act(tool: "goal_operator", PlanGoalArguments(action: "resume", goalRunId: goalRunId))
    }

    /// The view exposes this only for a proved before-work preflight blocker.
    @discardableResult
    func retryPreparation(goalRunId: String) async -> Bool {
        await resume(goalRunId: goalRunId)
    }

    /// Retries acceptance from the existing checkpoint only after the person
    /// confirms. The trusted operator validates that the blocker is recoverable.
    @discardableResult
    func retryAcceptance(goalRunId: String) async -> Bool {
        await act(tool: "goal_operator", PlanGoalArguments(
            action: "retry_acceptance", goalRunId: goalRunId
        ))
    }

    /// Adopts metadata for an existing committed checkpoint after the person
    /// confirms. The trusted operator validates HEAD, journal, and diff; this
    /// action does not start or resume the Goal.
    @discardableResult
    func adoptCheckpoint(goalRunId: String) async -> Bool {
        await act(tool: "goal_operator", PlanGoalArguments(
            action: "adopt_checkpoint", goalRunId: goalRunId
        ))
    }

    @discardableResult
    func cancel(goalRunId: String) async -> Bool {
        await act(tool: "goal_operator", PlanGoalArguments(action: "cancel", goalRunId: goalRunId))
    }

    @discardableResult
    func emergencyStop(reason: String?) async -> Bool {
        await emergency(PlanGoalArguments(action: "emergency_stop", reason: reason))
    }

    @discardableResult
    func releaseEmergencyStop() async -> Bool {
        await emergency(PlanGoalArguments(action: "release_emergency_stop"))
    }

    private func act(tool: String, _ arguments: PlanGoalArguments) async -> Bool {
        guard !actionInFlight, let app, let projectID = app.selectedProject?.id, boundProjectID == projectID else {
            return false
        }
        actionInFlight = true
        actionError = nil
        defer { actionInFlight = false }
        let result = await app.planGoalOperator(tool: tool, arguments: arguments, as: JSONValue.self)
        guard isCurrent(projectID, app) else { return false }
        var succeeded = false
        switch result {
        case .success:
            succeeded = true
        case .failure(let message, _):
            actionError = message
        }
        // Re-inspect and refresh the overview even after a failure, since a
        // stale-revision rejection means the displayed plan has changed.
        await refresh()
        return succeeded
    }

    private func emergency(_ arguments: PlanGoalArguments) async -> Bool {
        guard !emergencyInFlight, let app, let projectID = app.selectedProject?.id, boundProjectID == projectID else {
            return false
        }
        emergencyInFlight = true
        actionError = nil
        defer { emergencyInFlight = false }
        let result = await app.planGoalOperator(
            tool: "goal_operator", arguments: arguments, as: GoalEmergencyStopOutput.self
        )
        guard isCurrent(projectID, app) else { return false }
        var succeeded = false
        switch result {
        case .success(let value):
            succeeded = true
            if var current = overview {
                current.emergencyStopEngaged = value.emergencyStopEngaged ?? current.emergencyStopEngaged
                overview = current
            }
        case .failure(let message, _):
            actionError = message
        }
        await refresh()
        return succeeded
    }
}
