import Foundation

// Wire models for the "planGoalOperator" helper bridge (Plan Mode review and
// Goal Mode supervision). They are decoded with `JSONDecoder.chadex`
// (convertFromSnakeCase), so properties are camelCase without CodingKeys.
// Everything except the identifying fields is optional so that a missing or
// null field, or an extra one, never breaks decoding.

// MARK: - Request

struct PlanGoalOperatorParams: Encodable, Equatable {
    var path: String
    var tool: String
    var arguments: PlanGoalArguments
}

/// Union of the `plan_review` and `goal_operator` arguments. Nil fields are
/// not encoded. Encoded with `JSONEncoder.chadex` (convertToSnakeCase).
struct PlanGoalArguments: Encodable, Equatable {
    var action: String
    var planId: String?
    var revision: Int?
    var contentSha256: String?
    var reason: String?
    var approvalId: String?
    var ttlMinutes: Int?
    var goalRunId: String?
    /// Goal execution mode for `approve_and_start`: "codex" or "chatgpt".
    var mode: String?
}

// MARK: - Envelope

struct PlanGoalFailureOutput: Decodable, Equatable {
    var message: String?
    var errorKind: String?
}

/// `{ "success": Bool, "output": {...}, "error": String? }`
struct PlanGoalEnvelope<Output: Decodable>: Decodable {
    var success: Bool
    var output: Output?
    var error: String?
    var failure: PlanGoalFailureOutput?

    private enum CodingKeys: String, CodingKey { case success, output, error }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        success = try container.decodeIfPresent(Bool.self, forKey: .success) ?? false
        error = try container.decodeIfPresent(String.self, forKey: .error)
        if success {
            output = try container.decodeIfPresent(Output.self, forKey: .output)
            failure = nil
        } else {
            output = nil
            failure = try? container.decodeIfPresent(PlanGoalFailureOutput.self, forKey: .output)
        }
    }
}

enum PlanGoalResult<Output> {
    case success(Output)
    case failure(message: String, kind: String?)
}

// MARK: - Plans

struct PlanSummary: Decodable, Equatable, Identifiable, Hashable {
    var planId: String
    var projectId: String?
    var title: String
    var state: String
    var latestRevision: Int
    var planModeActive: Bool?
    var planModeExpiresAtUnixMs: Int?
    var createdAtUnixMs: Int?
    var updatedAtUnixMs: Int?
    var closedReason: String?
    /// "plan" (Plan Mode) or "goal" (a direct Goal request awaiting Start).
    var origin: String?

    var id: String { planId }
    var isPlanModeActive: Bool { planModeActive ?? false }
    var isGoalRequest: Bool { origin == "goal" }
}

struct PlanAcceptanceCheck: Decodable, Equatable, Hashable {
    var name: String?
    var argv: [String]?
    var cwd: String?
    var timeoutSecs: Int?

    var commandLine: String { (argv ?? []).joined(separator: " ") }
}

struct PlanBudget: Decodable, Equatable, Hashable {
    var maxIterations: Int?
    var maxMinutes: Int?
    var agentTimeoutSecs: Int?
}

struct PlanSpec: Decodable, Equatable, Hashable {
    var acceptance: [PlanAcceptanceCheck]?
    var budget: PlanBudget?
}

struct PlanRevision: Decodable, Equatable, Hashable {
    var revision: Int
    var content: String?
    var spec: PlanSpec?
    var contentSha256: String
    var createdAtUnixMs: Int?

    var shortHash: String { PlanGoalFormat.shortHash(contentSha256) }
}

struct PlanRevisionInfo: Decodable, Equatable, Hashable, Identifiable {
    var revision: Int
    var contentSha256: String?
    var createdAtUnixMs: Int?

    var id: Int { revision }
}

struct PlanDecisionBinding: Decodable, Equatable, Hashable {
    var projectId: String?
    var runnerClientId: String?
    var projectRoot: String?
    var baseCommit: String?
}

struct PlanDecision: Decodable, Equatable, Hashable, Identifiable {
    var approvalId: String
    var planId: String?
    var revision: Int
    var contentSha256: String?
    var state: String
    var binding: PlanDecisionBinding?
    var approver: String?
    var channel: String?
    var reason: String?
    var createdAtUnixMs: Int?
    var expiresAtUnixMs: Int?
    var consumedByGoalRunId: String?

    var id: String { approvalId }
    var isActive: Bool { state == "active" }
}

struct PlanDiff: Decodable, Equatable, Hashable {
    var changedLines: Int?
    var lines: [String]?
    var truncated: Bool?
}

struct PlanListOutput: Decodable, Equatable {
    var plans: [PlanSummary]?
}

struct PlanInspectOutput: Decodable, Equatable {
    var plan: PlanSummary
    var revision: PlanRevision?
    var revisions: [PlanRevisionInfo]?
    var decisions: [PlanDecision]?
    var diff: PlanDiff?
    /// Nil when Chadex could not tell (Runner offline, git failed).
    var gitReadiness: GitReadiness?
}

/// Whether the project can be the starting point of a Goal: a Goal always
/// starts from a commit, and only sees committed content.
struct GitReadiness: Decodable, Equatable {
    var isGitRepo: Bool?
    var hasCommit: Bool?
    var headCommit: String?
    var uncommittedFiles: Int?
    var blocksGoalStart: Bool?
    var projectRoot: String?
    var message: String?
    var howToCommit: [String]?
    var tips: [String]?

    var blocks: Bool { blocksGoalStart ?? false }
    var hasUncommittedWork: Bool { !blocks && (uncommittedFiles ?? 0) > 0 }
}

struct PlanDecisionOutput: Decodable, Equatable {
    var decision: PlanDecision?
}

// MARK: - Goal runs

struct GoalRun: Decodable, Equatable, Identifiable {
    var goalRunId: String
    var planId: String?
    var revision: Int?
    var contentSha256: String?
    var project: JSONValue?
    var state: String
    var stateReason: String?
    var controlRequest: String?
    var iteration: Int?
    var maxIterations: Int?
    var deadlineAtUnixMs: Int?
    var provider: String?
    var workspacePath: String?
    var resultBranch: String?
    var tokensUsed: Int?
    var cost: String?
    var createdAtUnixMs: Int?
    var updatedAtUnixMs: Int?
    var finishedAtUnixMs: Int?
    /// "codex" or "chatgpt"; a missing value means codex.
    var mode: String?
    var workspaceProject: String?
    var waitingForChatgpt: Bool?
    /// English hint written for the model, not shown to the person.
    var nextAction: String?

    var id: String { goalRunId }
    var isChatgptMode: Bool { mode == "chatgpt" }
    var isWaitingForChatgpt: Bool { isChatgptMode && (waitingForChatgpt ?? false) }
    var isChatgptIdle: Bool { isChatgptMode && state == "paused" && (stateReason ?? "").hasPrefix("chatgpt_idle") }
    var canPause: Bool { state == "running" || state == "queued" }
    var canResume: Bool { state == "paused" || state == "needs_attention" }
    var canCancel: Bool {
        !["succeeded", "failed", "cancelled", "stopped"].contains(state)
    }
    var isFinished: Bool { !canCancel }
}

struct GoalOperation: Decodable, Equatable, Identifiable {
    var opKey: String
    var kind: String?
    var iteration: Int?
    var state: String?
    var jobId: String?
    var result: JSONValue?
    var startedAtUnixMs: Int?
    var finishedAtUnixMs: Int?

    var id: String { opKey }
}

struct GoalEvent: Decodable, Equatable, Identifiable {
    var seq: Int
    var atUnixMs: Int?
    var kind: String
    var detail: JSONValue?

    var id: Int { seq }
}

struct GoalOverviewOutput: Decodable, Equatable {
    var plans: [PlanSummary]?
    var goalRuns: [GoalRun]?
    var emergencyStopEngaged: Bool?
    var planModeLocks: Int?
    var supervisorRunning: Bool?
}

struct GoalStatusOutput: Decodable, Equatable {
    var goalRun: GoalRun
    var acceptance: [JSONValue]?
    var operations: [GoalOperation]?
    var events: [GoalEvent]?
    var emergencyStopEngaged: Bool?
}

struct GoalRunOutput: Decodable, Equatable {
    var goalRun: GoalRun?
}

struct GoalEmergencyStopOutput: Decodable, Equatable {
    var emergencyStopEngaged: Bool?
    var affectedRuns: Int?
}

// MARK: - Formatting

enum PlanGoalFormat {
    static func shortHash(_ hash: String?) -> String {
        guard let hash else { return "" }
        return String(hash.prefix(12))
    }

    static func date(_ unixMs: Int?) -> String? {
        guard let unixMs else { return nil }
        let date = Date(timeIntervalSince1970: Double(unixMs) / 1000)
        return date.formatted(date: .abbreviated, time: .standard)
    }

    /// Compact one-line rendering of an event detail object.
    static func compact(_ value: JSONValue?) -> String {
        guard let value else { return "" }
        switch value {
        case .null: return ""
        case .string(let text): return text
        case .bool(let flag): return flag ? "true" : "false"
        case .number(let number):
            return number.rounded() == number && abs(number) < 1e15
                ? String(Int(number)) : String(number)
        case .array(let items):
            return items.map { compact($0) }.joined(separator: ", ")
        case .object(let fields):
            return fields.keys.sorted().compactMap { key -> String? in
                let rendered = compact(fields[key])
                if rendered.isEmpty { return nil }
                let clipped = rendered.count > 80 ? String(rendered.prefix(80)) + "…" : rendered
                return "\(key)=\(clipped)"
            }.joined(separator: " ")
        }
    }
}
