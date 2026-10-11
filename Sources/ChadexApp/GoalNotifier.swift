import Foundation
import UserNotifications

/// Posts a macOS notification when a Goal run finishes or stops, and when a
/// direct Goal request is waiting for Start. ChatGPT cannot be told when a
/// Goal finishes, so this is how the person learns about it.
@MainActor
final class GoalNotifier {
    enum Event: Equatable {
        case runSettled(GoalRun)
        case requestWaiting(PlanSummary)
    }

    static let interval: TimeInterval = 10
    private static let settledStates: Set<String> = [
        "succeeded", "failed", "cancelled", "stopped", "needs_attention",
    ]

    private weak var app: AppModel?
    private var task: Task<Void, Never>?
    private var runStates: [String: String]?
    private var waitingRequests: Set<String>?
    private var authorizationRequested = false

    func start(app: AppModel) {
        guard task == nil else { return }
        self.app = app
        task = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: UInt64(Self.interval * 1_000_000_000))
                await self?.poll()
            }
        }
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    /// One poll now, outside the timer (before the board is cleared).
    func pollNow() async {
        guard task != nil else { return }
        await poll()
    }

    private func poll() async {
        guard let app, app.selectedProject != nil else { return }
        let result = await app.planGoalOperator(
            tool: "goal_operator",
            arguments: PlanGoalArguments(action: "overview"),
            as: GoalOverviewOutput.self
        )
        guard case .success(let overview) = result else { return }
        let (events, states, requests) = Self.events(
            overview: overview, previousRuns: runStates, previousRequests: waitingRequests
        )
        runStates = states
        waitingRequests = requests
        // Request titles come from the model: at most one request
        // notification per poll, so a burst of requests cannot flood.
        var postedRequest = false
        for event in events {
            if case .requestWaiting = event {
                if postedRequest { continue }
                postedRequest = true
            }
            post(event)
        }
    }

    /// Pure diff of two overviews. The first observation only seeds state, so
    /// launching the app never replays old results.
    static func events(
        overview: GoalOverviewOutput,
        previousRuns: [String: String]?,
        previousRequests: Set<String>?
    ) -> ([Event], [String: String], Set<String>) {
        let runs = overview.goalRuns ?? []
        let states = Dictionary(runs.map { ($0.goalRunId, $0.state) }, uniquingKeysWith: { first, _ in first })
        let waiting = (overview.plans ?? []).filter { $0.isGoalRequest && $0.state == "drafting" }
        let requestIDs = Set(waiting.map(\.planId))
        var events: [Event] = []
        if let previousRuns {
            for run in runs where settledStates.contains(run.state) && previousRuns[run.goalRunId] != run.state {
                // A run first seen already finished, e.g. after a restart, is old news.
                if previousRuns[run.goalRunId] != nil {
                    events.append(.runSettled(run))
                }
            }
        }
        if let previousRequests {
            for plan in waiting where !previousRequests.contains(plan.planId) {
                events.append(.requestWaiting(plan))
            }
        }
        return (events, states, requestIDs)
    }

    private func post(_ event: Event) {
        // Notifications need a real app bundle (not `swift test`).
        guard Bundle.main.bundleURL.pathExtension == "app" else { return }
        let center = UNUserNotificationCenter.current()
        if !authorizationRequested {
            authorizationRequested = true
            center.requestAuthorization(options: [.alert, .sound]) { _, _ in }
        }
        let content = UNMutableNotificationContent()
        switch event {
        case .runSettled(let run):
            content.title = L10n.string("planGoal.notify.runTitle", stateTitle(run.state))
            content.body = [run.stateReason, run.resultBranch].compactMap { $0 }.joined(separator: "\n")
        case .requestWaiting(let plan):
            content.title = L10n.string("planGoal.notify.requestTitle")
            content.body = L10n.string("planGoal.notify.requestBody", plan.title)
        }
        content.sound = .default
        center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }

    private func stateTitle(_ state: String) -> String {
        let key = "planGoal.runState.\(state)"
        let text = L10n.string(key)
        return text == key ? state : text
    }
}
