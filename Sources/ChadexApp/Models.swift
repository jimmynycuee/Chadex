import Foundation

enum ConnectionPhase: String, CaseIterable, Sendable {
    case unconfigured
    case preparing
    case waitingForChatGPTVerification = "waiting_for_chatgpt_verification"
    case verified
    case stopped
    case error
}

enum ConnectionAction: Equatable, Sendable {
    case connecting
    case disconnecting
}

enum ComputerControlMode: String, Codable, CaseIterable, Identifiable, Sendable {
    case readOnly = "read_only"
    case askBeforeControl = "ask_before_control"
    case allowSession = "allow_session"
    case alwaysAllow = "always_allow"

    var id: String { rawValue }
}

struct ComputerApproval: Codable, Equatable, Identifiable, Sendable {
    var approvalId: String
    var action: String
    var createdAtMs: UInt64
    var id: String { approvalId }
}

struct ComputerSafetyAuditEvent: Codable, Equatable, Identifiable, Sendable {
    var sequence: UInt64
    var timestampMs: UInt64
    var event: String
    var action: String?
    var reason: String?
    var id: UInt64 { sequence }
}

struct ComputerSafetyStatus: Codable, Equatable, Sendable {
    var mode: ComputerControlMode
    var stopped: Bool
    var generation: UInt64
    var pendingApprovals: [ComputerApproval]
    var audit: [ComputerSafetyAuditEvent]

    static let initial = ComputerSafetyStatus(
        mode: .askBeforeControl,
        stopped: false,
        generation: 0,
        pendingApprovals: [],
        audit: []
    )
}

extension ConnectionPhase: Codable {
    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        let value = try container.decode(String.self)
        if value == "waiting_for_chat_gpt_verification" {
            self = .waitingForChatGPTVerification
            return
        }
        guard let phase = ConnectionPhase(rawValue: value) else {
            throw DecodingError.dataCorruptedError(
                in: container,
                debugDescription: "Unknown Chadex connection phase: \(value)"
            )
        }
        self = phase
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(rawValue)
    }
}

struct SnapshotRequestGate: Sendable {
    private(set) var nextSequence: UInt64 = 0
    private(set) var latestAppliedSequence: UInt64 = 0

    mutating func issue() -> UInt64 {
        nextSequence &+= 1
        return nextSequence
    }

    mutating func shouldApply(_ sequence: UInt64) -> Bool {
        guard sequence >= latestAppliedSequence else { return false }
        latestAppliedSequence = sequence
        return true
    }

    mutating func shouldApply(
        _ sequence: UInt64,
        candidateRevision: UInt64,
        currentRevision: UInt64
    ) -> Bool {
        if shouldApply(sequence) {
            return true
        }
        return candidateRevision > currentRevision
    }
}

struct ActivityRefreshGate: Sendable {
    private(set) var loadedSequence: UInt64?

    func shouldRefresh(for sequence: UInt64) -> Bool {
        loadedSequence != sequence
    }

    mutating func markLoaded(_ sequence: UInt64) {
        loadedSequence = sequence
    }

    mutating func reset() {
        loadedSequence = nil
    }
}

struct SnapshotFreshnessGate: Sendable {
    private(set) var lastAppliedUptime: TimeInterval?

    mutating func markApplied(at uptime: TimeInterval) {
        lastAppliedUptime = uptime
    }

    func shouldRefresh(at uptime: TimeInterval, maxAge: TimeInterval) -> Bool {
        guard let lastAppliedUptime else { return true }
        let age = uptime - lastAppliedUptime
        return age < 0 || age > maxAge
    }
}

struct ProjectSwitchGate: Sendable {
    private(set) var activeProjectID: UUID?

    mutating func begin(_ projectID: UUID) -> Bool {
        guard activeProjectID == nil else { return false }
        activeProjectID = projectID
        return true
    }

    mutating func finish(_ projectID: UUID) {
        guard activeProjectID == projectID else { return }
        activeProjectID = nil
    }
}

enum ActivityLevel: String, Codable, Sendable {
    case info
    case warning
    case error
}

struct ProjectRecord: Codable, Hashable, Identifiable, Sendable {
    var id: UUID
    var name: String
    var path: String
    var addedAt: Date

    init(id: UUID = UUID(), name: String, path: String, addedAt: Date = Date()) {
        self.id = id
        self.name = name
        self.path = path
        self.addedAt = addedAt
    }
}

struct ProjectInspection: Codable, Equatable, Sendable {
    var path: String
    var allowedRoot: String
    var isGitRepository: Bool
    var readable: Bool
    var writable: Bool
}

struct SkillDescriptor: Codable, Equatable, Identifiable, Sendable {
    var skillId: String
    var name: String
    var description: String
    var definitionRevision: String
    var packageRevision: String?
    var sourceScope: String
    var trust: String
    var nameConflict: Bool
    /// Absent from older helpers; nil means the helper did not report a policy.
    var scriptsAllowed: Bool?

    var id: String { skillId }
    var isProjectSkill: Bool { sourceScope == "project" }
    var isManagedSkill: Bool { trust == "operator_installed_guidance" }
    var isConfiguredSkill: Bool { trust == "operator_configured_guidance" }
}

struct SkillCatalogInspection: Codable, Equatable, Sendable {
    var project: String
    var catalogRevision: String
    var totalCount: Int
    var returnedCount: Int
    var skills: [SkillDescriptor]
    var invalidCount: Int
    var diagnostics: [JSONValue]
    var discoveryTruncated: Bool

    var revisionsBySkillID: [String: String] {
        Dictionary(uniqueKeysWithValues: skills.map {
            ($0.skillId, $0.definitionRevision + "|" + ($0.packageRevision ?? "live"))
        })
    }
}

struct ManagedSkillInventoryEntry: Codable, Equatable, Identifiable, Sendable {
    var skillId: String
    var skillKey: String
    var stateRevision: String
    var activePackageRevision: String?
    var preferredPackageRevision: String
    var definitionRevision: String
    var name: String
    var description: String
    var totalVersions: Int

    var id: String { skillId }
    var isActive: Bool { activePackageRevision != nil }
    var revisionIdentity: String {
        definitionRevision + "|" + preferredPackageRevision + "|" + (activePackageRevision ?? "inactive")
    }
}

struct SkillInventoryInspection: Codable, Equatable, Sendable {
    var project: String
    var totalCount: Int
    var skills: [ManagedSkillInventoryEntry]
}

struct SkillDefinitionPreview: Codable, Equatable, Sendable {
    var skillId: String
    var definitionRevision: String
    var packageRevision: String?
    var text: String
    var hasMore: Bool
}

struct SkillOperationResult: Codable, Equatable, Sendable {}

struct ExternalSkillPackage: Codable, Equatable, Identifiable, Sendable {
    var package: String
    var state: String
    var name: String?
    var description: String?
    var hasScripts: Bool
    var linkTargetRoot: String?
    var invalidReason: String?
    var nameConflict: Bool

    var id: String { package }
}

struct ExternalSkillSource: Codable, Equatable, Identifiable, Sendable {
    var kind: String
    var path: String
    var canonicalPath: String?
    var status: String
    var rootIsLink: Bool
    var sameAs: String?
    var validCount: Int
    var symlinkCount: Int
    var invalidCount: Int
    var scriptCount: Int
    var truncated: Bool
    var providedBy: [String]
    var packages: [ExternalSkillPackage]

    var id: String { kind + "|" + path }
}

struct ExternalSkillSourceDiscovery: Codable, Equatable, Sendable {
    var format: String
    var sources: [ExternalSkillSource]
    var recommendedRoots: [String]
}

/// Whether the user already has somewhere to pick Skills from. Mirrors the Windows `hasValidSource`.
enum ExternalSkillAvailability: Equatable {
    /// Discovery has not finished (or failed), so "nothing found" must not be shown yet.
    case unknown
    case none
    case available

    static func isProvidedElsewhere(_ source: ExternalSkillSource) -> Bool {
        source.status == "duplicate_source" || (!source.providedBy.isEmpty && source.validCount == 0)
    }

    static func isConnectable(_ source: ExternalSkillSource) -> Bool {
        source.status == "available" && source.canonicalPath != nil && !isProvidedElsewhere(source)
    }

    /// A usable source is a connectable source holding at least one Skill, or any connected root.
    static func evaluate(
        discovery: ExternalSkillSourceDiscovery?,
        roots: ExternalSkillRootsState?,
        loading: Bool
    ) -> ExternalSkillAvailability {
        if let roots, !roots.roots.isEmpty { return .available }
        if let discovery, discovery.sources.contains(where: { isConnectable($0) && $0.validCount > 0 }) {
            return .available
        }
        guard !loading, discovery != nil, roots != nil else { return .unknown }
        return .none
    }
}

/// User-facing text for helper error codes that the Skills and Project Memory pages can hit.
enum SkillManagementErrorMessage {
    static func message(forCode code: String) -> String? {
        switch code {
        case "skill_management_requires_local_runtime": return L10n.string("skills.error.requiresLocalRuntime")
        case "skill_management_credential_unavailable": return L10n.string("skills.error.credentialUnavailable")
        case "skill_management_credential_rejected": return L10n.string("skills.error.credentialRejected")
        case "runner_config_path_is_link": return L10n.string("skills.error.runnerConfigPathIsLink")
        // Skill package problems reported by the runtime while installing a ZIP.
        case "skill_definition_missing": return L10n.string("skills.error.definitionMissing")
        case "skill_frontmatter_missing", "skill_frontmatter_unclosed", "skill_frontmatter_duplicate_field",
             "skill_frontmatter_scalar_invalid", "skill_name_missing", "skill_name_invalid",
             "skill_description_missing", "skill_description_invalid":
            return L10n.string("skills.error.definitionInvalid")
        case "skill_definition_too_large": return L10n.string("skills.error.definitionTooLarge")
        case "skill_definition_invalid_utf8": return L10n.string("skills.error.definitionEncoding")
        case "skill_install_archive_malformed", "skill_install_archive_size_mismatch":
            return L10n.string("skills.error.archiveMalformed")
        case "skill_install_archive_path_invalid", "skill_install_archive_special_entry", "skill_resource_path_invalid":
            return L10n.string("skills.error.archivePathInvalid")
        case "skill_install_duplicate_path": return L10n.string("skills.error.archiveDuplicatePath")
        case "skill_install_file_count_exceeded", "skill_install_file_too_large",
             "skill_install_total_too_large", "skill_install_archive_too_large":
            return L10n.string("skills.error.archiveTooLarge")
        case "skill_artifact_invalid", "skill_install_artifact_not_found", "skill_install_artifact_path_invalid",
             "skill_install_artifact_unavailable", "skill_install_artifact_changed":
            return L10n.string("skills.error.artifactUnavailable")
        case "skill_archive_flatten_failed": return L10n.string("skills.error.archiveFlattenFailed")
        case "skill_install_source_project_unavailable", "skill_install_source_project_forbidden":
            return L10n.string("skills.error.projectForbidden")
        case "skill_key_invalid", "skill_install_invalid_arguments": return L10n.string("skills.error.keyInvalid")
        case "skill_store_skill_limit_exceeded", "skill_store_revision_limit_exceeded":
            return L10n.string("skills.error.limitReached")
        case "skill_state_changed", "skill_expected_state_required", "skill_install_reconcile_required":
            return L10n.string("skills.error.stateChanged")
        case "skill_store_capability_unavailable", "skill_store_unavailable", "skill_store_lock_unavailable":
            return L10n.string("skills.error.storeUnavailable")
        default: return nil
        }
    }

    /// The helper's fixed fallback text for `installSkill`; it carries no reason on its own.
    static let genericInstallHelperMessage = "Chadex could not install the Skill"

    /// Install failures always say why: a known code gets its own message, a specific
    /// helper message is kept, and the helper's generic text is replaced by one naming the code.
    static func installMessage(forCode code: String, helperMessage: String? = nil) -> String {
        if let known = message(forCode: code) { return known }
        if let helperMessage, !helperMessage.isEmpty, helperMessage != genericInstallHelperMessage {
            return helperMessage
        }
        return L10n.string("skills.error.installFailed", code)
    }
}

struct ExternalSkillRootsState: Codable, Equatable, Sendable {
    var format: String
    var roots: [String]
    var scriptRoots: [String]
    var revision: String
    var generation: Int?
}

struct SetExternalSkillRootsParams: Codable, Sendable {
    var roots: [String]
    var scriptRoots: [String]
    var expectedRevision: String
    var verifyProjectPath: String?
}

struct SkillCenterItem: Equatable, Identifiable, Sendable {
    var skillId: String
    var name: String
    var description: String
    var definitionRevision: String
    var packageRevision: String?
    var sourceScope: String
    var trust: String
    var nameConflict: Bool
    var scriptsAllowed: Bool?
    var managed: ManagedSkillInventoryEntry?

    var id: String { skillId }
    var isManaged: Bool { managed != nil }
    var isActive: Bool { managed?.isActive ?? true }
    var canLoadDefinition: Bool { isActive }

    var sourceLabelKey: String {
        if isManaged { return "skills.source.installed" }
        if trust == "operator_configured_guidance" { return "skills.source.configured" }
        return "skills.source.project"
    }
}

enum ProjectMemoryCategory: String, CaseIterable, Identifiable, Sendable {
    case architecture
    case decisions
    case workflow
    case other

    var id: String { rawValue }
}

struct ProjectMemoryDescriptor: Codable, Equatable, Identifiable, Sendable {
    var memoryId: String
    var memoryKey: String
    var summary: String
    var priority: String
    var bootstrap: Bool
    var tags: [String]
    var revision: String
    var matchedFields: [String]?

    var id: String { memoryId }
    var category: ProjectMemoryCategory {
        let normalized = Set(tags.map { $0.lowercased() })
        if !normalized.isDisjoint(with: ["architecture", "architectural"]) { return .architecture }
        if !normalized.isDisjoint(with: ["decision", "decisions", "adr"]) { return .decisions }
        if !normalized.isDisjoint(with: ["workflow", "process", "procedure"]) { return .workflow }
        return .other
    }
}

struct ProjectMemoryCatalog: Codable, Equatable, Sendable {
    var project: String
    var catalogRevision: String
    var totalCount: Int
    var returnedCount: Int
    var memories: [ProjectMemoryDescriptor]
}

struct ProjectMemoryProvenance: Codable, Equatable, Sendable {
    var createdByKind: String
    var updatedByKind: String
}

struct ProjectMemoryRecord: Codable, Equatable, Identifiable, Sendable {
    var project: String
    var memoryId: String
    var memoryKey: String
    var summary: String
    var body: String
    var priority: String
    var bootstrap: Bool
    var tags: [String]
    var revision: String
    var createdAtUnixMs: Int64
    var updatedAtUnixMs: Int64
    var provenance: ProjectMemoryProvenance

    var id: String { memoryId }
}

struct ProjectMemorySetResult: Codable, Equatable, Sendable {
    var project: String
    var memoryId: String
    var memoryKey: String
    var oldRevision: String?
    var revision: String
    var created: Bool
    var stateChanged: Bool
}

struct ProjectMemoryDeleteResult: Codable, Equatable, Sendable {
    var project: String
    var memoryId: String?
    var memoryKey: String
    var revision: String?
    var deleted: Bool
    var stateChanged: Bool
}

struct ActivityEntry: Codable, Identifiable, Equatable, Sendable {
    var sequence: UInt64
    var timestampMs: UInt64
    var source: String
    var level: ActivityLevel
    var eventKind: String
    var message: String

    var id: UInt64 { sequence }
    var date: Date { Date(timeIntervalSince1970: Double(timestampMs) / 1000.0) }
}

struct McpPerformanceTraceEntry: Codable, Identifiable, Equatable, Sendable {
    var sequence: UInt64
    var startedAtMs: UInt64
    var finishedAtMs: UInt64?
    var requestIdHashes: [String]?
    var serverTraceId: String?
    var methods: [String]
    var toolNames: [String]
    var toolFailed: Bool? = nil
    var requestBytes: UInt64
    var responseBytes: UInt64
    var statusCode: UInt16?
    var ingressPreBackendUs: UInt64
    var backendHeadersUs: UInt64
    var responseStreamUs: UInt64
    var totalUs: UInt64
    var completion: String

    var id: UInt64 { sequence }
    var date: Date { Date(timeIntervalSince1970: Double(startedAtMs) / 1000.0) }
    var totalMilliseconds: Double { Double(totalUs) / 1_000.0 }
    var backendHeadersMilliseconds: Double { Double(backendHeadersUs) / 1_000.0 }
}

struct LifecyclePerformanceTraceEntry: Codable, Identifiable, Equatable, Sendable {
    var sequence: UInt64
    var startedAtMs: UInt64
    var operation: String
    var phase: String
    var totalUs: UInt64
    var completion: String

    var id: UInt64 { sequence }
    var date: Date { Date(timeIntervalSince1970: Double(startedAtMs) / 1000.0) }
    var totalMilliseconds: Double { Double(totalUs) / 1_000.0 }
}

struct AppPhaseTimingSample: Equatable, Sendable {
    var timestamp: Date
    var operation: String
    var phase: String
    var durationMilliseconds: Double
    var succeeded: Bool
}

struct HelperLatencySample: Equatable, Sendable {
    var timestamp: Date
    var method: String
    var durationMilliseconds: Double
    var succeeded: Bool
}

struct OperationSnapshot: Codable, Equatable, Sendable {
    var id: String
    var kind: String
    var phase: String
    var startedAtMs: UInt64
    var cancellable: Bool
}

struct TaskProgressStep: Codable, Equatable, Sendable, Identifiable {
    var index: Int
    var kind: String
    var status: String
    var durationMs: UInt64
    var attempts: Int
    var retries: Int

    var id: Int { index }
}

struct TaskValidationSummary: Codable, Equatable, Sendable {
    var status: String
    var checksPassed: UInt64
    var checksFailed: UInt64
    var failedCheck: UInt64?
}

struct TaskReviewSummary: Codable, Equatable, Sendable {
    var status: String
    var changedFileCount: UInt64?
}

struct TaskProgressSnapshot: Codable, Equatable, Sendable {
    var taskId: String
    var project: String
    var goal: String
    var status: String
    var currentStep: Int
    var totalSteps: Int
    var completedSteps: Int
    var plan: [String]
    var cancelRequested: Bool
    var startedAtMs: UInt64
    var finishedAtMs: UInt64?
    var durationMs: UInt64?
    var validation: TaskValidationSummary
    var review: TaskReviewSummary
    var steps: [TaskProgressStep]

    var isActive: Bool {
        ["queued", "preparing", "running", "integrating", "validating", "cancelling"].contains(status)
    }

    var canCancel: Bool {
        (status == "queued" || status == "running") && !cancelRequested
    }
}

struct HelperErrorPayload: Codable, Equatable, Error, Sendable {
    var code: String
    var message: String
    var recovery: String?
    var details: JSONValue?
}

struct GraphifyStatus: Codable, Equatable, Sendable {
    var available: Bool
    var path: String?
    var source: String
}

/// Bounded local-runtime evidence; never includes command text or process output.
struct FerretJobSnapshot: Codable, Equatable, Sendable {
    var jobId: String
    var status: String
    var startedAtMs: UInt64?
    var finishedAtMs: UInt64?
    var exitCode: Int?

    var isActive: Bool {
        ["queued", "agent_queued", "started", "running", "recovering", "stop_requested"].contains(status)
    }
    var isWaiting: Bool { ["queued", "agent_queued", "recovering", "stop_requested"].contains(status) }
}

struct BackendSnapshot: Codable, Equatable, Sendable {
    var phase: ConnectionPhase
    var graphify: GraphifyStatus?
    var selectedProject: ProjectInspection?
    var tunnelReady: Bool
    var chatGPTConnected: Bool
    var chatGPTVerifiedForSelectedProject: Bool
    var lastVerifiedAtMs: UInt64?
    var currentOperation: OperationSnapshot?
    var taskProgress: TaskProgressSnapshot?
    var mascotJobs: [FerretJobSnapshot]?
    var error: HelperErrorPayload?
    var activitySequence: UInt64
    var stateRevision: UInt64

    private enum CodingKeys: String, CodingKey {
        case phase
        case graphify
        case selectedProject
        case tunnelReady
        case chatGptConnected
        case chatGptVerifiedForSelectedProject
        case lastVerifiedAtMs
        case currentOperation
        case taskProgress
        case mascotJobs
        case error
        case activitySequence
        case stateRevision
    }

    init(
        phase: ConnectionPhase,
        graphify: GraphifyStatus? = nil,
        selectedProject: ProjectInspection?,
        tunnelReady: Bool,
        chatGPTConnected: Bool,
        chatGPTVerifiedForSelectedProject: Bool,
        lastVerifiedAtMs: UInt64?,
        currentOperation: OperationSnapshot?,
        taskProgress: TaskProgressSnapshot? = nil,
        mascotJobs: [FerretJobSnapshot]? = nil,
        error: HelperErrorPayload?,
        activitySequence: UInt64,
        stateRevision: UInt64 = 0
    ) {
        self.phase = phase
        self.graphify = graphify
        self.selectedProject = selectedProject
        self.tunnelReady = tunnelReady
        self.chatGPTConnected = chatGPTConnected
        self.chatGPTVerifiedForSelectedProject = chatGPTVerifiedForSelectedProject
        self.lastVerifiedAtMs = lastVerifiedAtMs
        self.currentOperation = currentOperation
        self.taskProgress = taskProgress
        self.mascotJobs = mascotJobs
        self.error = error
        self.activitySequence = activitySequence
        self.stateRevision = stateRevision
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        phase = try container.decode(ConnectionPhase.self, forKey: .phase)
        graphify = try container.decodeIfPresent(GraphifyStatus.self, forKey: .graphify)
        selectedProject = try container.decodeIfPresent(ProjectInspection.self, forKey: .selectedProject)
        tunnelReady = try container.decode(Bool.self, forKey: .tunnelReady)
        chatGPTConnected = try container.decodeIfPresent(Bool.self, forKey: .chatGptConnected) ?? false
        chatGPTVerifiedForSelectedProject = try container.decode(Bool.self, forKey: .chatGptVerifiedForSelectedProject)
        lastVerifiedAtMs = try container.decodeIfPresent(UInt64.self, forKey: .lastVerifiedAtMs)
        currentOperation = try container.decodeIfPresent(OperationSnapshot.self, forKey: .currentOperation)
        taskProgress = try container.decodeIfPresent(TaskProgressSnapshot.self, forKey: .taskProgress)
        mascotJobs = try container.decodeIfPresent([FerretJobSnapshot].self, forKey: .mascotJobs)
        error = try container.decodeIfPresent(HelperErrorPayload.self, forKey: .error)
        activitySequence = try container.decode(UInt64.self, forKey: .activitySequence)
        stateRevision = try container.decodeIfPresent(UInt64.self, forKey: .stateRevision) ?? 0
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(phase, forKey: .phase)
        try container.encodeIfPresent(graphify, forKey: .graphify)
        try container.encodeIfPresent(selectedProject, forKey: .selectedProject)
        try container.encode(tunnelReady, forKey: .tunnelReady)
        try container.encode(chatGPTConnected, forKey: .chatGptConnected)
        try container.encode(chatGPTVerifiedForSelectedProject, forKey: .chatGptVerifiedForSelectedProject)
        try container.encodeIfPresent(lastVerifiedAtMs, forKey: .lastVerifiedAtMs)
        try container.encodeIfPresent(currentOperation, forKey: .currentOperation)
        try container.encodeIfPresent(taskProgress, forKey: .taskProgress)
        try container.encodeIfPresent(mascotJobs, forKey: .mascotJobs)
        try container.encodeIfPresent(error, forKey: .error)
        try container.encode(activitySequence, forKey: .activitySequence)
        try container.encode(stateRevision, forKey: .stateRevision)
    }

    func isAtLeastAsFresh(as current: BackendSnapshot) -> Bool {
        stateRevision >= current.stateRevision
    }

    static let initial = BackendSnapshot(
        phase: .unconfigured,
        graphify: nil,
        selectedProject: nil,
        tunnelReady: false,
        chatGPTConnected: false,
        chatGPTVerifiedForSelectedProject: false,
        lastVerifiedAtMs: nil,
        currentOperation: nil,
        taskProgress: nil,
        error: nil,
        activitySequence: 0,
        stateRevision: 0
    )
}

enum ConnectionPresentation {
    static func phase(
        for snapshot: BackendSnapshot,
        isBootstrapping: Bool,
        isSwitchingProject: Bool = false,
        isConnecting: Bool = false
    ) -> ConnectionPhase {
        if isSwitchingProject || isConnecting {
            return .preparing
        }
        if isBootstrapping && snapshot.phase == .error {
            return .preparing
        }
        return snapshot.phase
    }

    static func error(
        for snapshot: BackendSnapshot,
        actionError: HelperErrorPayload?,
        isBootstrapping: Bool,
        isSwitchingProject: Bool = false,
        isConnecting: Bool = false
    ) -> HelperErrorPayload? {
        if isSwitchingProject || isConnecting {
            return nil
        }
        if isBootstrapping && snapshot.phase == .error {
            return actionError
        }
        return snapshot.error ?? actionError
    }
}

enum JSONValue: Codable, Equatable, Sendable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([String: JSONValue].self) {
            self = .object(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unsupported JSON value")
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .string(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .bool(let value): try container.encode(value)
        case .object(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .null: try container.encodeNil()
        }
    }
}

struct HelperRequest: Encodable, Sendable {
    var protocolVersion: Int = 1
    var requestId: String
    var method: String
    var params: JSONValue
}

struct HelperResponse: Decodable, Sendable {
    var protocolVersion: Int
    var requestId: String
    var result: JSONValue?
    var error: HelperErrorPayload?
}

struct EmptyParams: Codable, Sendable {}

struct ComputerControlModeParams: Codable, Sendable {
    var mode: ComputerControlMode
}

struct ComputerApprovalParams: Codable, Sendable {
    var approvalId: String
}

struct InspectProjectParams: Codable, Sendable {
    var path: String
}

struct ActivateProjectParams: Codable, Sendable {
    var path: String
}

struct SkillDefinitionParams: Codable, Sendable {
    var path: String
    var skillId: String
    var definitionRevision: String
    var packageRevision: String?
}

struct CreateProjectSkillParams: Codable, Sendable {
    var path: String
    var skillKey: String
    var content: String
}

struct InstallSkillParams: Codable, Sendable {
    var path: String
    var skillKey: String
    var artifactPath: String
}

struct ActivateSkillParams: Codable, Sendable {
    var path: String
    var skillKey: String
    var packageRevision: String
    var stateRevision: String
}

struct DeactivateSkillParams: Codable, Sendable {
    var path: String
    var skillKey: String
    var stateRevision: String
}

struct ProjectMemoryReadParams: Codable, Sendable {
    var path: String
    var memoryKey: String
    var expectedRevision: String?
}

struct ProjectMemorySetParams: Codable, Sendable {
    var path: String
    var memoryKey: String
    var summary: String
    var body: String
    var priority: String
    var bootstrap: Bool
    var tags: [String]
    var expectedRevision: String?
}

struct ProjectMemoryDeleteParams: Codable, Sendable {
    var path: String
    var memoryKey: String
    var expectedRevision: String
}

struct CredentialParams: Codable, Sendable {
    var tunnelId: String
    var apiKey: String
}

struct CancelOperationParams: Codable, Sendable {
    var operationId: String
}

struct CancelTaskParams: Codable, Sendable {
    var project: String
    var taskId: String
}

struct ActivityQueryParams: Codable, Sendable {
    var limit: Int
}

struct PerformanceTraceQueryParams: Codable, Sendable {
    var limit: Int
}
