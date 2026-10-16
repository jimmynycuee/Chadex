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
    /// Complete acceptance array for `propose_acceptance`.
    var acceptance: JSONValue? = nil
    /// Complete read-only source manifest for `propose_acceptance`.
    var workspaceRequirements: JSONValue? = nil
    /// Complete dependency provenance manifest for `propose_acceptance`.
    var dependencies: JSONValue? = nil
    /// Stale-write fence for `propose_acceptance` and `approve_acceptance`.
    var expectedAcceptanceRevision: Int? = nil
    /// Server-issued hash shown in the human approval comparison.
    var acceptanceSha256: String? = nil
    /// Proposal identity fence for `approve_acceptance`.
    var proposalId: String? = nil
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
    var lane: String?
    var kind: String?
    var loopbackPorts: [UInt16]?
    var containerImage: String?
    var dependsOn: [String]?
    var externalGate: PlanExternalGate?

    var commandLine: String { (argv ?? []).joined(separator: " ") }
    var effectiveLane: String { lane ?? "offline_sandbox" }
    var effectiveKind: String { kind ?? "test" }
}

struct PlanExternalGate: Decodable, Equatable, Hashable {
    var owner: String?
    var action: String?
    var evidence: String?
    var nextStep: String?
}

struct PlanBudget: Decodable, Equatable, Hashable {
    var maxIterations: Int?
    var maxMinutes: Int?
    var agentTimeoutSecs: Int?
}

struct PlanWorkspaceRequirement: Decodable, Equatable, Hashable {
    var sourcePath: String
    var sourceSha256: String
    var destinationPath: String
    var permission: String
}

struct PlanWorkspaceDependency: Decodable, Equatable, Hashable {
    var path: String
    var provenance: String
}

struct PlanSpec: Decodable, Equatable, Hashable {
    var acceptance: [PlanAcceptanceCheck]?
    var budget: PlanBudget?
    var workspaceRequirements: [PlanWorkspaceRequirement]?
    var dependencies: [PlanWorkspaceDependency]?
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

/// Result of the app-only approve-and-start action. The backend response keeps
/// its existing decision/goal_run/created fields; the panel uses the run to
/// render its actual mode and state immediately.
struct GoalStartOutput: Decodable, Equatable {
    var decision: PlanDecision?
    var goalRun: GoalRun?
    var created: Bool?
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
    var committedRounds: Int?
    var completedAcceptanceRounds: Int?
    /// Kept opaque until the Store defines the attempt record shape.
    var currentAttempt: JSONValue?
    var checkpointSha: String?
    var acceptanceRevision: Int?

    var id: String { goalRunId }
    var isChatgptMode: Bool { mode == "chatgpt" }
    var isWaitingForChatgpt: Bool { isChatgptMode && (waitingForChatgpt ?? false) }
    var isChatgptIdle: Bool { isChatgptMode && state == "paused" && (stateReason ?? "").hasPrefix("chatgpt_idle") }
    var canPause: Bool { state == "running" || state == "queued" }
    /// Only an explicitly paused run uses the general resume operation.
    var canResume: Bool { state == "paused" && !isOutcomeUnknown }
    var isOutcomeUnknown: Bool {
        ["outcome_unknown", "unresolved", "cleanup_pending"].contains { (stateReason ?? "").localizedCaseInsensitiveContains($0) }
    }
    /// A retry is available only from a checkpoint Git object ID reported by
    /// the trusted operator. Unknown outcomes are never replayed automatically.
    var canRetryAcceptance: Bool {
        state == "needs_attention"
            && !isOutcomeUnknown
            && GoalWireHash.isGitObjectID(checkpointSha)
    }
    /// Legacy committed work with no usable Git object ID needs a human-
    /// confirmed metadata adoption before retry or acceptance editing.
    var canAdoptCheckpoint: Bool {
        state == "needs_attention"
            && !isOutcomeUnknown
            && (committedRounds ?? 0) > 0
            && !GoalWireHash.isGitObjectID(checkpointSha)
    }
    var canCancel: Bool {
        !["succeeded", "failed", "cancelled", "stopped"].contains(state)
    }
    var isFinished: Bool { !canCancel }
    var hasCheckpointProgressMetrics: Bool {
        committedRounds != nil
            || completedAcceptanceRounds != nil
            || currentAttempt != nil
            || !(checkpointSha ?? "").isEmpty
            || acceptanceRevision != nil
    }
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
    var workspaceRequirements: [JSONValue]?
    var dependencies: [JSONValue]?
    var pendingAcceptance: GoalPendingAcceptance?
    var acceptanceRevisions: [JSONValue]?
    var blockingCause: JSONValue?
    var operations: [GoalOperation]?
    var events: [GoalEvent]?
    var emergencyStopEngaged: Bool?

    var hasUnknownOutcome: Bool {
        goalRun.isOutcomeUnknown
            || Self.containsUnknownOutcome(blockingCause)
            || (events ?? []).contains { $0.kind.localizedCaseInsensitiveContains("outcome_unknown") }
    }

    private static func containsUnknownOutcome(_ value: JSONValue?) -> Bool {
        guard let value else { return false }
        switch value {
        case .string(let text):
            return text.localizedCaseInsensitiveContains("outcome_unknown")
        case .array(let values):
            return values.contains { containsUnknownOutcome($0) }
        case .object(let fields):
            return fields.values.contains { containsUnknownOutcome($0) }
        default:
            return false
        }
    }

    /// A preflight rejection before ANY agent/staging operation is safe to
    /// prepare again after explicit local human confirmation. It is not a
    /// generic Resume for unknown or partly executed work.
    var canRetryPreparation: Bool {
        goalRun.state == "needs_attention"
            && !hasUnknownOutcome
            && (goalRun.committedRounds ?? 0) == 0
            && goalRun.checkpointSha == nil
            && (goalRun.stateReason ?? "").hasPrefix("toolchain_preflight_")
            && !(operations ?? []).contains { $0.kind == "agent" || $0.kind == "commit" }
    }

    var canRetryAcceptance: Bool {
        goalRun.canRetryAcceptance && !hasUnknownOutcome
    }

    var canAdoptCheckpoint: Bool {
        goalRun.canAdoptCheckpoint && !hasUnknownOutcome
    }

    var canOpenAcceptanceEditor: Bool {
        goalRun.state == "needs_attention"
            && !hasUnknownOutcome
            && GoalWireHash.isGitObjectID(goalRun.checkpointSha)
            && (goalRun.acceptanceRevision ?? -1) >= 0
            && acceptance != nil
            && workspaceRequirements != nil
            && dependencies != nil
    }
}

struct GoalAcceptanceSpec: Decodable, Equatable {
    var acceptance: [JSONValue]?
    var workspaceRequirements: [JSONValue]?
    var dependencies: [JSONValue]?

    var jsonValue: JSONValue? {
        guard let acceptance, let workspaceRequirements, let dependencies else { return nil }
        return .object([
            "acceptance": .array(acceptance),
            "workspace_requirements": .array(workspaceRequirements),
            "dependencies": .array(dependencies)
        ])
    }
}

struct GoalPendingAcceptance: Decodable, Equatable {
    var proposalId: String?
    var revision: Int?
    var specSha256: String?
    var spec: GoalAcceptanceSpec?
    var state: String?
}

/// Safe blocker fields come from the direct structured status payload first,
/// with event detail filling any fields that the payload omits.
struct GoalBlockingEvidence: Equatable {
    var blockingCheck: JSONValue?
    var executable: JSONValue?
    var repair: JSONValue?
    var cause: JSONValue?

    init?(blockingCause: JSONValue? = nil, events: [GoalEvent]?) {
        let direct = Self.parse(blockingCause)
        let event = (events ?? []).sorted { $0.seq < $1.seq }.reversed().first { event in
            event.kind.localizedCaseInsensitiveContains("block")
                || event.kind.localizedCaseInsensitiveContains("fail")
                || event.kind.localizedCaseInsensitiveContains("outcome_unknown")
                || Self.containsKnownField(event.detail)
        }
        let eventEvidence = Self.parse(event?.detail)
        guard direct != nil || eventEvidence != nil else { return nil }
        blockingCheck = direct?.blockingCheck ?? eventEvidence?.blockingCheck
        executable = direct?.executable ?? eventEvidence?.executable
        repair = direct?.repair ?? eventEvidence?.repair
        cause = direct?.cause ?? eventEvidence?.cause
    }

    private init?(fields: [String: JSONValue]) {
        let nestedCause = fields["cause"]
        let nestedFields: [String: JSONValue] = {
            if case .object(let object)? = nestedCause { return object }
            return [:]
        }()
        blockingCheck = Self.firstValue(["blocking_check", "blockingCheck", "blockingcheck"], in: fields)
            ?? Self.firstValue(["blocking_check", "blockingCheck", "blockingcheck"], in: nestedFields)
        executable = fields["executable"] ?? nestedFields["executable"]
        repair = fields["repair"] ?? nestedFields["repair"]
        cause = nestedCause
        guard blockingCheck != nil || executable != nil || repair != nil || cause != nil else { return nil }
    }

    private static func parse(_ value: JSONValue?) -> GoalBlockingEvidence? {
        guard case .object(let fields)? = value else { return nil }
        return GoalBlockingEvidence(fields: fields)
    }

    private static func containsKnownField(_ value: JSONValue?) -> Bool {
        guard case .object(let fields)? = value else { return false }
        return firstValue(["blocking_check", "blockingCheck", "blockingcheck", "executable", "repair"], in: fields) != nil
    }

    private static func firstValue(_ keys: [String], in fields: [String: JSONValue]) -> JSONValue? {
        keys.lazy.compactMap { fields[$0] }.first
    }
}

struct GoalAcceptanceProposal: Decodable, Equatable {
    var proposalId: String
    var goalRunId: String
    var sourceAcceptanceRevision: Int
    var checkpointSha: String
    var sourceAcceptanceSha256: String
    var proposedAcceptanceSha256: String
    var acceptance: [JSONValue]
    var workspaceRequirements: [JSONValue]
    var dependencies: [JSONValue]
    var proposedAtUnixMs: Int

    /// The Store's normalized pending record is defined as source revision + 1.
    var pendingAcceptance: GoalPendingAcceptance {
        GoalPendingAcceptance(
            proposalId: proposalId,
            revision: sourceAcceptanceRevision + 1,
            specSha256: proposedAcceptanceSha256,
            spec: GoalAcceptanceSpec(
                acceptance: acceptance,
                workspaceRequirements: workspaceRequirements,
                dependencies: dependencies
            ),
            state: "proposed"
        )
    }
}

struct GoalAcceptanceEditorSnapshot: Equatable {
    var acceptance: [JSONValue]
    var workspaceRequirements: [JSONValue]
    var dependencies: [JSONValue]
    var expectedAcceptanceRevision: Int
}

/// Local-only editor state. It deliberately carries no server revision or
/// approval fields; those are supplied by the Goal Store wire contract.
struct GoalAcceptanceEditorDraft: Equatable {
    let current: GoalAcceptanceSpec
    var text: String

    init(currentAcceptance: [JSONValue], workspaceRequirements: [JSONValue], dependencies: [JSONValue]) {
        current = GoalAcceptanceSpec(
            acceptance: currentAcceptance,
            workspaceRequirements: workspaceRequirements,
            dependencies: dependencies
        )
        text = PlanGoalFormat.json(current.jsonValue ?? .object([:]))
    }

    var currentText: String { PlanGoalFormat.json(current.jsonValue ?? .object([:])) }
    var parsedValue: JSONValue? {
        try? JSONDecoder().decode(JSONValue.self, from: Data(text.utf8))
    }
    var parsedSpec: GoalAcceptanceSpec? {
        guard case .object(let fields)? = parsedValue,
              Set(fields.keys) == Set(["acceptance", "workspace_requirements", "dependencies"]),
              case .array(let acceptance)? = fields["acceptance"],
              case .array(let workspaceRequirements)? = fields["workspace_requirements"],
              case .array(let dependencies)? = fields["dependencies"] else { return nil }
        return GoalAcceptanceSpec(
            acceptance: acceptance,
            workspaceRequirements: workspaceRequirements,
            dependencies: dependencies
        )
    }
    var isValid: Bool { parsedSpec != nil }
    var hasChanges: Bool {
        guard let parsedSpec else { return false }
        return parsedSpec != current
    }
    var canReview: Bool { isValid && hasChanges }
}

enum GoalWireHash {
    static func isGitObjectID(_ value: String?) -> Bool {
        guard let value, value.utf8.count == 40 || value.utf8.count == 64 else { return false }
        return value.utf8.allSatisfy(isASCIIHexDigit)
    }

    static func isSHA256(_ value: String?) -> Bool {
        guard let value, value.utf8.count == 64 else { return false }
        return value.utf8.allSatisfy(isASCIIHexDigit)
    }

    private static func isASCIIHexDigit(_ byte: UInt8) -> Bool {
        (48...57).contains(byte) || (65...70).contains(byte) || (97...102).contains(byte)
    }
}

struct GoalRunOutput: Decodable, Equatable {
    var goalRun: GoalRun?
}

struct GoalStartKey: Equatable, Hashable {
    var projectScopeID: UUID
    var projectId: String?
    var planId: String
    var revision: Int
    var contentSha256: String
}

enum GoalStartPresentation: Equatable {
    case available
    case starting
    case checkingStatus
    case existingRun(GoalRun)
    case existingRunID(String?)
}

/// Client-side single-flight state for one exact plan revision. Durable
/// decisions and runs restore the completed state after a panel reload.
struct GoalStartLedger: Equatable {
    private enum PendingPhase: Equatable {
        case awaitingConfirmation
        case submitting
        case outcomeUnknown
    }

    private var pending: [GoalStartKey: PendingPhase] = [:]
    private(set) var completed: Set<GoalStartKey> = []
    private(set) var acceptedRuns: [GoalStartKey: GoalRun] = [:]
    private(set) var acceptedRunIDs: [GoalStartKey: String] = [:]

    mutating func reserve(
        _ key: GoalStartKey,
        projectId: String?,
        decisions: [PlanDecision],
        runs: [GoalRun]
    ) -> Bool {
        guard case .available = presentation(
            for: key, projectId: projectId, decisions: decisions, runs: runs
        ) else { return false }
        pending[key] = .awaitingConfirmation
        return true
    }

    mutating func beginSubmission(_ key: GoalStartKey) -> Bool {
        guard pending[key] == .awaitingConfirmation else { return false }
        pending[key] = .submitting
        return true
    }

    func pendingKey(
        projectScopeID: UUID,
        planId: String,
        revision: Int,
        contentSha256: String
    ) -> GoalStartKey? {
        pending.keys.first {
            $0.projectScopeID == projectScopeID && $0.planId == planId &&
                $0.revision == revision && $0.contentSha256 == contentSha256
        }
    }

    mutating func cancelConfirmation(_ key: GoalStartKey) {
        guard pending[key] == .awaitingConfirmation else { return }
        pending.removeValue(forKey: key)
    }

    mutating func cancelConfirmation(
        projectScopeID: UUID,
        planId: String,
        revision: Int,
        contentSha256: String
    ) {
        guard let key = pendingKey(
            projectScopeID: projectScopeID,
            planId: planId,
            revision: revision,
            contentSha256: contentSha256
        ) else { return }
        cancelConfirmation(key)
    }

    mutating func fail(_ key: GoalStartKey) {
        pending.removeValue(forKey: key)
    }

    mutating func markOutcomeUnknown(_ key: GoalStartKey) {
        guard pending[key] == .submitting else { return }
        pending[key] = .outcomeUnknown
    }

    mutating func recordFailure(_ key: GoalStartKey, kind: String?) {
        if kind == nil || kind == "outcome_unknown" {
            markOutcomeUnknown(key)
        } else {
            fail(key)
        }
    }

    /// Drops project-local UI state when switching workspaces but preserves
    /// unresolved submissions until that workspace's durable state is read.
    mutating func resetForProjectChange() {
        pending = pending.reduce(into: [:]) { result, item in
            switch item.value {
            case .outcomeUnknown, .submitting:
                result[item.key] = .outcomeUnknown
            case .awaitingConfirmation:
                break
            }
        }
        completed.removeAll()
        acceptedRuns.removeAll()
        acceptedRunIDs.removeAll()
    }

    mutating func reconcileUnknownRuns(projectScopeID: UUID, runs: [GoalRun]) {
        let unknownKeys = pending.compactMap { key, phase in
            key.projectScopeID == projectScopeID && phase == .outcomeUnknown ? key : nil
        }
        for key in unknownKeys {
            guard let run = Self.newestMatchingRun(
                key: key, projectId: key.projectId, runs: runs
            ) else { continue }
            complete(key, run: run, runId: run.goalRunId)
        }
    }

    /// Reconciles an uncertain transport result against a successfully read
    /// overview and the exact inspected revision. An incomplete read leaves
    /// the reservation in place.
    mutating func reconcileUnknown(
        _ key: GoalStartKey,
        projectId: String?,
        decisions: [PlanDecision],
        runs: [GoalRun],
        authoritative: Bool
    ) {
        guard pending[key] == .outcomeUnknown, authoritative else { return }
        if let run = Self.newestMatchingRun(key: key, projectId: projectId, runs: runs) {
            complete(key, run: run, runId: run.goalRunId)
        } else if let decision = Self.consumedDecision(
            key: key, projectId: projectId, decisions: decisions
        ) {
            complete(key, run: nil, runId: decision.consumedByGoalRunId)
        } else {
            pending.removeValue(forKey: key)
        }
    }

    mutating func complete(_ key: GoalStartKey, run: GoalRun?, runId: String?) {
        pending.removeValue(forKey: key)
        completed.insert(key)
        if let run { acceptedRuns[key] = run }
        if let runId, !runId.isEmpty { acceptedRunIDs[key] = runId }
    }

    func presentation(
        for key: GoalStartKey,
        projectId: String?,
        decisions: [PlanDecision],
        runs: [GoalRun]
    ) -> GoalStartPresentation {
        let durableRun = Self.newestMatchingRun(key: key, projectId: projectId, runs: runs)
        let acceptedRun = acceptedRuns[key].flatMap {
            Self.matches($0, key: key, projectId: projectId) ? $0 : nil
        }
        if let run = Self.preferredRun(acceptedRun, durableRun) {
            return .existingRun(run)
        }
        if completed.contains(key) {
            let consumedRunId = Self.consumedDecision(
                key: key, projectId: projectId, decisions: decisions
            )?.consumedByGoalRunId
            return .existingRunID(acceptedRunIDs[key] ?? consumedRunId)
        }
        if let decision = Self.consumedDecision(key: key, projectId: projectId, decisions: decisions) {
            return .existingRunID(decision.consumedByGoalRunId)
        }
        switch pending[key] {
        case .awaitingConfirmation, .submitting: return .starting
        case .outcomeUnknown: return .checkingStatus
        case nil: break
        }
        return .available
    }

    private static func newestMatchingRun(
        key: GoalStartKey,
        projectId: String?,
        runs: [GoalRun]
    ) -> GoalRun? {
        runs.filter { matches($0, key: key, projectId: projectId) }
            .max { ($0.updatedAtUnixMs ?? Int.min) < ($1.updatedAtUnixMs ?? Int.min) }
    }

    private static func preferredRun(_ cached: GoalRun?, _ durable: GoalRun?) -> GoalRun? {
        guard let cached else { return durable }
        guard let durable else { return cached }
        guard let cachedUpdated = cached.updatedAtUnixMs,
              let durableUpdated = durable.updatedAtUnixMs else { return durable }
        return cachedUpdated > durableUpdated ? cached : durable
    }

    static func matches(_ run: GoalRun, key: GoalStartKey, projectId: String?) -> Bool {
        guard run.planId == key.planId,
              run.revision == key.revision,
              run.contentSha256 == key.contentSha256 else { return false }
        guard let projectId else { return true }
        guard case .string(let runProjectId)? = run.project else { return false }
        return runProjectId == projectId
    }

    private static func consumedDecision(
        key: GoalStartKey,
        projectId: String?,
        decisions: [PlanDecision]
    ) -> PlanDecision? {
        decisions.first { decision in
            guard decision.planId == key.planId,
                  decision.revision == key.revision,
                  decision.contentSha256 == key.contentSha256,
                  (decision.state == "consumed" || decision.consumedByGoalRunId != nil) else {
                return false
            }
            if let projectId {
                guard decision.binding?.projectId == projectId else { return false }
            }
            return true
        }
    }
}

struct GoalEmergencyStopOutput: Decodable, Equatable {
    var emergencyStopEngaged: Bool?
    var affectedRuns: Int?
}

// MARK: - Formatting

enum PlanGoalFormat {
    static func json(_ value: JSONValue) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes]
        guard let data = try? encoder.encode(value),
              let text = String(data: data, encoding: .utf8) else { return "null" }
        return text
    }

    static func shortHash(_ hash: String?) -> String {
        guard let hash else { return "" }
        return String(hash.prefix(12))
    }

    static func acceptanceChecks(_ acceptance: [JSONValue]?) -> [PlanAcceptanceCheck]? {
        guard let acceptance,
              let data = try? JSONEncoder().encode(JSONValue.array(acceptance)) else { return nil }
        return try? JSONDecoder.chadex.decode([PlanAcceptanceCheck].self, from: data)
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
