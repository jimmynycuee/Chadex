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
        switch overviewResult {
        case .success(let value):
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

    // MARK: - Goal actions

    @discardableResult
    func pause(goalRunId: String) async -> Bool {
        await act(tool: "goal_operator", PlanGoalArguments(action: "pause", goalRunId: goalRunId))
    }

    @discardableResult
    func resume(goalRunId: String) async -> Bool {
        await act(tool: "goal_operator", PlanGoalArguments(action: "resume", goalRunId: goalRunId))
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
