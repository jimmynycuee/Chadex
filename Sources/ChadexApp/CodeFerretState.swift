import Foundation

enum FerretState: String, CaseIterable, Identifiable {
    case idle, listening, thinking, searching, coding, testing, waiting, longTask, success, error, sleep
    var id: String { rawValue }
    var title: String { L10n.string("ferret.state.\(rawValue)") }
    var isWorking: Bool { [.thinking, .searching, .coding, .testing, .longTask].contains(self) }
    var symbol: String {
        switch self {
        case .idle: return "circle"
        case .listening: return "waveform"
        case .thinking: return "ellipsis"
        case .searching: return "folder.badge.magnifyingglass"
        case .coding: return "chevron.left.forwardslash.chevron.right"
        case .testing: return "checklist"
        case .waiting: return "hourglass"
        case .longTask: return "clock"
        case .success: return "checkmark"
        case .error: return "exclamationmark.triangle"
        case .sleep: return "moon.zzz"
        }
    }
}

struct FerretPresentation: Equatable {
    var state: FerretState = .idle
    var activity: FerretState? = nil
    var progress: Double? = nil
    var since: Date = .distantPast
}

/// A deterministic, clock-injected projection of observed runtime facts. It never
/// invents model token activity, test results, or a percentage for unknown work.
struct FerretController {
    private(set) var presentation = FerretPresentation()
    private var projectPath: String?
    private var initialized = false
    private var traceFloor: UInt64 = 0
    private var seenFinished: Set<UInt64> = []
    private var lastActivitySequence: UInt64 = 0
    private var lastTaskID: String?
    private var lastTaskStatus: String?
    private var busyID: String?
    private var receivedAt: Date = .distantPast
    private var lastActivityAt: Date = .distantPast
    private var seenTerminalJobs: Set<String> = []
    private var lastObservedActiveJobs = false
    private var reaction: (state: FerretState, until: Date)?

    mutating func update(snapshot: BackendSnapshot, traces: [McpPerformanceTraceEntry],
                         activities: [ActivityEntry], preparing: Bool = false,
                         foreground: Bool = true, now: Date = Date()) {
        let path = snapshot.selectedProject?.path
        if !initialized || path != projectPath {
            // Do not replay history or carry a reaction into another project.
            traceFloor = initialized ? traces.map(\.sequence).max() ?? 0 : 0
            seenFinished = Set(traces.filter { $0.finishedAtMs != nil }.map(\.sequence))
            lastActivitySequence = activities.map(\.sequence).max() ?? 0
            lastTaskID = snapshot.taskProgress?.taskId
            lastTaskStatus = snapshot.taskProgress?.status
            projectPath = path
            initialized = true
            lastActivityAt = now
            reaction = nil
            seenTerminalJobs = Set((snapshot.mascotJobs ?? []).filter { !$0.isActive }.map(\.jobId))
            lastObservedActiveJobs = false
            busyID = nil
        }

        let newEvents = activities.filter { $0.sequence > lastActivitySequence }
        if let newest = newEvents.max(by: { $0.sequence < $1.sequence }) {
            lastActivitySequence = newest.sequence
            lastActivityAt = now
            if newEvents.contains(where: { $0.level == .error }) {
                reaction = (.error, now.addingTimeInterval(5))
            }
        }
        // Start and finish share a sequence. Detect finishes separately so a
        // response can end an observed request without producing a second start.
        let finished = traces.filter { !seenFinished.contains($0.sequence) && $0.finishedAtMs != nil && $0.sequence > traceFloor && $0.toolNames.contains(where: Self.isMeaningfulTool) }
        if let newest = finished.max(by: { $0.sequence < $1.sequence }) {
            seenFinished.formUnion(finished.map(\.sequence))
            seenFinished.formIntersection(Set(traces.map(\.sequence)))
            if let end = newest.finishedAtMs, age(end, now) < 8, newest.toolNames.contains(where: Self.isMeaningfulTool) {
                lastActivityAt = now
                let failed = newest.toolFailed == true || (newest.statusCode ?? 200) >= 400 || newest.completion != "completed"
                reaction = (failed ? .error : .waiting, now.addingTimeInterval(failed ? 5 : 3))
            }
        }
        let task = snapshot.taskProgress
        if let task, task.taskId != lastTaskID || task.status != lastTaskStatus {
            lastTaskID = task.taskId
            lastTaskStatus = task.status
            lastActivityAt = now
            if task.status == "completed", task.validation.status != "failed" {
                reaction = (.success, now.addingTimeInterval(3))
            } else if ["failed", "failed_validation"].contains(task.status) || task.validation.status == "failed" {
                reaction = (.error, now.addingTimeInterval(5))
            }
        }

        let jobs = snapshot.mascotJobs
        let activeJobs = (jobs ?? []).filter(\.isActive)
        if let jobs {
            for job in jobs where !job.isActive && !seenTerminalJobs.contains(job.jobId) {
                seenTerminalJobs.insert(job.jobId)
                if let finished = job.finishedAtMs, age(finished, now) < 8 {
                    lastActivityAt = now
                    if ["failed", "lost", "timeout", "timed_out", "process_lost"].contains(job.status) || (job.exitCode.map { $0 != 0 } ?? false) {
                        reaction = (.error, now.addingTimeInterval(5))
                    } else if ["completed", "exited"].contains(job.status), job.exitCode == 0 {
                        reaction = (.success, now.addingTimeInterval(3))
                    } else {
                        reaction = (.waiting, now.addingTimeInterval(3))
                    }
                }
            }
            seenTerminalJobs.formIntersection(Set(jobs.map(\.jobId)))
            lastObservedActiveJobs = !activeJobs.isEmpty
        }
        let live = traces.filter { $0.sequence > traceFloor && $0.finishedAtMs == nil && $0.toolNames.contains(where: Self.isMeaningfulTool) }
        let activeTask = task.map { Self.taskIsActive($0.status) } ?? false
        let id = activeTask ? "task:\(task!.taskId)" : activeJobs.first.map { "job:\($0.jobId)" } ?? live.first.map { "tool:\($0.sequence)" }
        if let id, id != busyID {
            busyID = id
            receivedAt = now
            lastActivityAt = now
        } else if id == nil {
            busyID = nil
        }

        var state: FerretState = .idle
        var activity: FerretState?
        var progress: Double?
        if let task, activeTask {
            activity = Self.taskState(task)
            if task.totalSteps > 0 {
                progress = min(1, max(0, Double(task.completedSteps) / Double(task.totalSteps)))
            }
            state = activity!
            if age(task.startedAtMs, now) >= 60 && state != .waiting { state = .longTask }
            if now.timeIntervalSince(receivedAt) < 0.65 { state = .listening }
        } else if !activeJobs.isEmpty {
            activity = activeJobs.allSatisfy(\.isWaiting) ? .waiting : .thinking
            state = activity!
            if state != .waiting, let start = activeJobs.compactMap(\.startedAtMs).min(), age(start, now) >= 60 {
                state = .longTask
            }
            if now.timeIntervalSince(receivedAt) < 0.65 { state = .listening }
        } else if !live.isEmpty {
            activity = live.flatMap(\.toolNames).filter(Self.isMeaningfulTool).map(Self.toolState).max(by: { Self.priority($0) < Self.priority($1) })
            state = activity ?? .thinking
            if let start = live.map(\.startedAtMs).min(), age(start, now) >= 60 { state = .longTask }
            if now.timeIntervalSince(receivedAt) < 0.65 { state = .listening }
        } else if snapshot.error != nil {
            state = .error
        } else if task?.status == "blocked" || task?.status == "interrupted" || task?.status == "ready_to_apply" || task?.status == "unknown" {
            state = .waiting
        } else if (jobs ?? []).contains(where: { $0.status == "unknown" }) || (jobs == nil && lastObservedActiveJobs) {
            // Loss of observation is not terminal evidence. Await a fresh scoped snapshot.
            state = .waiting
        } else if let operation = snapshot.currentOperation {
            state = operation.phase == "cancelling" ? .waiting : .thinking
            if age(operation.startedAtMs, now) >= 60 { state = .longTask }
        } else if preparing {
            state = .thinking
        } else if let reaction, now < reaction.until {
            state = reaction.state
        } else if snapshot.phase == .waitingForChatGPTVerification {
            state = .waiting
        } else if now.timeIntervalSince(lastActivityAt) >= 180 || !foreground {
            state = .sleep
        }
        if state != presentation.state { presentation.since = now }
        presentation.state = state
        presentation.activity = activity
        presentation.progress = progress
    }

    static func taskIsActive(_ status: String) -> Bool {
        ["queued", "preparing", "running", "integrating", "validating", "cancelling"].contains(status)
    }

    static func taskState(_ task: TaskProgressSnapshot) -> FerretState {
        if task.status == "queued" || task.status == "cancelling" || task.cancelRequested { return .waiting }
        if task.status == "validating" { return .testing }
        if task.status == "integrating" { return .coding }
        guard task.plan.indices.contains(task.currentStep) else { return .thinking }
        switch task.plan[task.currentStep] {
        case "search", "read": return .searching
        case "edit": return .coding
        case "validate": return .testing
        case "run_process": return .thinking // A process is not necessarily a test.
        default: return .thinking
        }
    }

    static func isMeaningfulTool(_ name: String) -> Bool {
        !["runtime_status", "list_tools", "list_runners", "list_projects", "tool_manifest",
          "read_tool_trace", "goal_plan_state", "work_result_state", "agent_wait_state",
          "agent_continuation_bind", "agent_continuation_recover_endpoint", "agent_continuation_state",
          "agent_continuation_wake_acquire", "agent_continuation_wake_prepare",
          "agent_continuation_wake_finish", "agent_continuation_unbind"].contains(name)
    }

    static func toolState(_ name: String) -> FerretState {
        switch name {
        case "search_project_texts", "read_project_files", "read_files", "read_file", "search_codebase", "search_code", "list_directory", "list_project_files", "get_project_context", "project_map", "search_symbols": return .searching
        case "apply_project_edits", "apply_edits", "apply_patch", "apply_text_edits", "write_file", "write_files": return .coding
        case "run_validation", "validate_project", "validate_task", "cargo_test", "cargo_check": return .testing
        case "task_status", "job_status", "poll_job", "await_job", "observe_jobs", "wait_for_agent_events": return .waiting
        default: return .thinking
        }
    }

    private static func priority(_ state: FerretState) -> Int {
        switch state { case .testing: return 4; case .coding: return 3; case .searching: return 2; case .thinking: return 1; default: return 0 }
    }
    private func age(_ milliseconds: UInt64, _ now: Date) -> TimeInterval {
        max(0, now.timeIntervalSince1970 - Double(milliseconds) / 1000)
    }
}

/// Explicit opt-in for isolated visual review, never enabled in a normal launch.
enum FerretReviewMode {
    static let enabled = CommandLine.arguments.contains("--ferret-review")
        || ProcessInfo.processInfo.environment["CHADEX_FERRET_REVIEW"] == "1"
}
