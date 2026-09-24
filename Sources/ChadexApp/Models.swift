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
        status == "queued" || status == "running" || status == "cancelling"
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
        isSwitchingProject: Bool = false
    ) -> ConnectionPhase {
        if isSwitchingProject {
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
        isSwitchingProject: Bool = false
    ) -> HelperErrorPayload? {
        if isSwitchingProject {
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

struct InspectProjectParams: Codable, Sendable {
    var path: String
}

struct ActivateProjectParams: Codable, Sendable {
    var path: String
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
