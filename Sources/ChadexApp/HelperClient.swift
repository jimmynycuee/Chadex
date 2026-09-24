import Foundation

enum HelperClientError: LocalizedError {
    case helperMissing(String)
    case launchFailed(String)
    case notRunning
    case protocolMismatch(expected: Int, received: Int)
    case malformedResponse
    case backend(HelperErrorPayload)
    case disconnected
    case timedOut(String)
    case cancelled
    case tooManyPendingRequests

    var errorDescription: String? {
        switch self {
        case .helperMissing(let path): return "Chadex helper was not found at \(path)."
        case .launchFailed(let message): return "Could not launch Chadex helper: \(message)"
        case .notRunning: return "Chadex helper is not running."
        case .protocolMismatch(let expected, let received): return "Helper protocol mismatch (expected \(expected), got \(received))."
        case .malformedResponse: return "Chadex helper returned an invalid response."
        case .backend(let error): return error.message
        case .disconnected: return "Chadex helper disconnected."
        case .timedOut(let method): return "Chadex helper request \(method) timed out."
        case .cancelled: return "Chadex helper request was cancelled."
        case .tooManyPendingRequests: return "Chadex helper has too many requests in flight."
        }
    }
}

final class HelperClient: @unchecked Sendable {
    static let protocolVersion = 1
    private static let maxPendingRequests = 64
    private static let shutdownTotalTimeout: TimeInterval = 5
    private static let shutdownRPCTimeout: TimeInterval = 1.5

    private struct PendingRequest {
        let generation: UInt64
        let continuation: CheckedContinuation<HelperResponse, Error>
    }

    private final class CancellationState: @unchecked Sendable {
        private let lock = NSLock()
        private var cancelled = false

        func cancel() {
            lock.lock()
            cancelled = true
            lock.unlock()
        }

        func isCancelled() -> Bool {
            lock.lock()
            defer { lock.unlock() }
            return cancelled
        }
    }

    private let lock = NSLock()
    private let writeLock = NSLock()
    private let executableOverride: URL?
    private var process: Process?
    private var processGeneration: UInt64 = 0
    private var stdinPipe: Pipe?
    private var stdoutPipe: Pipe?
    private var stderrPipe: Pipe?
    private var pending: [String: PendingRequest] = [:]
    private var stdoutBuffer = Data()
    private var latencySamples: [HelperLatencySample] = []
    private(set) var stderrTail = ""

    init(executableURL: URL? = nil) {
        self.executableOverride = executableURL
    }

    deinit {
        shutdownImmediately()
    }

    func startIfNeededAsync() async throws {
        try await Task.detached(priority: .userInitiated) { [self] in
            try startIfNeeded()
        }.value
    }

    func startIfNeeded() throws {
        lock.lock()
        defer { lock.unlock() }
        if process?.isRunning == true { return }

        let executableURL = try executableOverride ?? Self.resolveExecutableURL()
        guard FileManager.default.isExecutableFile(atPath: executableURL.path) else {
            throw HelperClientError.helperMissing(executableURL.path)
        }

        let process = Process()
        let stdinPipe = Pipe()
        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()
        process.executableURL = executableURL
        process.standardInput = stdinPipe
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe
        process.environment = Self.environmentForHelper()
        let generation = processGeneration &+ 1
        process.terminationHandler = { [weak self] _ in
            self?.handleProcessTermination(generation: generation)
        }

        stdoutPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            self?.consumeStdout(handle.availableData)
        }
        stderrPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            self?.consumeStderr(handle.availableData)
        }

        do {
            try process.run()
            self.process = process
            self.processGeneration = generation
            self.stdinPipe = stdinPipe
            self.stdoutPipe = stdoutPipe
            self.stderrPipe = stderrPipe
            stdoutBuffer.removeAll(keepingCapacity: true)
        } catch {
            throw HelperClientError.launchFailed(error.localizedDescription)
        }
    }

    func request<Params: Encodable, Result: Decodable>(
        method: String,
        params: Params,
        as resultType: Result.Type = Result.self,
        timeout: TimeInterval? = nil
    ) async throws -> Result {
        let startedAt = ProcessInfo.processInfo.systemUptime
        var succeeded = false
        defer {
            recordLatency(
                method: method,
                durationMilliseconds: (ProcessInfo.processInfo.systemUptime - startedAt) * 1_000,
                succeeded: succeeded
            )
        }
        try startIfNeeded()
        let requestId = UUID().uuidString
        let paramsValue = try Self.encodeJSONValue(params)
        let request = HelperRequest(requestId: requestId, method: method, params: paramsValue)
        var frame = try JSONEncoder.chadex.encode(request)
        frame.append(0x0A)
        let timeout = max(0.05, timeout ?? Self.requestTimeout(for: method))
        let cancellationState = CancellationState()

        let response: HelperResponse = try await withTaskCancellationHandler(operation: {
            try await withCheckedThrowingContinuation { continuation in
                if cancellationState.isCancelled() || Task.isCancelled {
                    continuation.resume(throwing: HelperClientError.cancelled)
                    return
                }

                lock.lock()
                guard process?.isRunning == true,
                      let writer = stdinPipe?.fileHandleForWriting else {
                    lock.unlock()
                    continuation.resume(throwing: HelperClientError.notRunning)
                    return
                }
                guard pending.count < Self.maxPendingRequests else {
                    lock.unlock()
                    continuation.resume(throwing: HelperClientError.tooManyPendingRequests)
                    return
                }
                let generation = processGeneration
                pending[requestId] = PendingRequest(
                    generation: generation,
                    continuation: continuation
                )
                lock.unlock()

                if cancellationState.isCancelled() || Task.isCancelled {
                    resolve(
                        requestId: requestId,
                        generation: generation,
                        result: .failure(HelperClientError.cancelled)
                    )
                    return
                }

                Task.detached(priority: .utility) { [weak self] in
                    let nanos = UInt64(min(timeout, 3_600) * 1_000_000_000)
                    try? await Task.sleep(nanoseconds: nanos)
                    self?.resolve(
                        requestId: requestId,
                        generation: generation,
                        result: .failure(HelperClientError.timedOut(method))
                    )
                }

                do {
                    writeLock.lock()
                    defer { writeLock.unlock() }
                    try writer.write(contentsOf: frame)
                } catch {
                    resolve(
                        requestId: requestId,
                        generation: generation,
                        result: .failure(error)
                    )
                }
            }
        }, onCancel: { [weak self] in
            cancellationState.cancel()
            self?.resolve(
                requestId: requestId,
                generation: nil,
                result: .failure(HelperClientError.cancelled)
            )
        })

        guard response.protocolVersion == Self.protocolVersion else {
            throw HelperClientError.protocolMismatch(expected: Self.protocolVersion, received: response.protocolVersion)
        }
        if let error = response.error {
            throw HelperClientError.backend(error)
        }
        guard let value = response.result else {
            throw HelperClientError.malformedResponse
        }
        let decoded = try Self.decode(Result.self, from: value)
        succeeded = true
        return decoded
    }

    func performanceSamples(limit: Int = 50) -> [HelperLatencySample] {
        lock.lock()
        defer { lock.unlock() }
        let boundedLimit = max(1, min(limit, 100))
        return Array(latencySamples.suffix(boundedLimit))
    }

    func sendCredential(tunnelID: String, apiKey: String) async throws -> BackendSnapshot {
        try await request(
            method: "provideCredential",
            params: CredentialParams(tunnelId: tunnelID, apiKey: apiKey)
        )
    }

    func shutdown() async {
        let startedAt = ProcessInfo.processInfo.systemUptime
        let (process, writer, generation) = processContext()
        guard let process, process.isRunning else { return }

        let remainingForRPC = max(
            0.05,
            min(
                Self.shutdownRPCTimeout,
                Self.shutdownTotalTimeout - (ProcessInfo.processInfo.systemUptime - startedAt)
            )
        )
        let _: BackendSnapshot? = try? await request(
            method: "shutdown",
            params: EmptyParams(),
            timeout: remainingForRPC
        )
        try? writer?.close()

        while process.isRunning
            && ProcessInfo.processInfo.systemUptime - startedAt < Self.shutdownTotalTimeout {
            try? await Task.sleep(for: .milliseconds(100))
        }
        if process.isRunning {
            process.terminate()
        }
        failPending(generation: generation, with: HelperClientError.disconnected)
        if !process.isRunning {
            cleanupAfterExit(process, generation: generation)
        }
    }

    private func processContext() -> (Process?, FileHandle?, UInt64) {
        lock.lock()
        defer { lock.unlock() }
        return (process, stdinPipe?.fileHandleForWriting, processGeneration)
    }

    private func shutdownImmediately() {
        lock.lock()
        let process = self.process
        let writer = stdinPipe?.fileHandleForWriting
        stdoutPipe?.fileHandleForReading.readabilityHandler = nil
        stderrPipe?.fileHandleForReading.readabilityHandler = nil
        lock.unlock()
        try? writer?.close()
        if let process, process.isRunning {
            process.terminate()
        }
    }

    private func cleanupAfterExit(_ exitedProcess: Process, generation: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        guard processGeneration == generation, process === exitedProcess else { return }
        stdoutPipe?.fileHandleForReading.readabilityHandler = nil
        stderrPipe?.fileHandleForReading.readabilityHandler = nil
        process = nil
        stdinPipe = nil
        stdoutPipe = nil
        stderrPipe = nil
        stdoutBuffer.removeAll(keepingCapacity: true)
    }

    private func handleProcessTermination(generation: UInt64) {
        failPending(generation: generation, with: HelperClientError.disconnected)
        lock.lock()
        guard processGeneration == generation else {
            lock.unlock()
            return
        }
        let exitedProcess = process
        lock.unlock()
        if let exitedProcess {
            cleanupAfterExit(exitedProcess, generation: generation)
        }
    }

    private func consumeStdout(_ data: Data) {
        guard !data.isEmpty else { return }
        lock.lock()
        stdoutBuffer.append(data)
        var lines: [Data] = []
        while let range = stdoutBuffer.firstRange(of: Data([0x0A])) {
            lines.append(stdoutBuffer.subdata(in: 0..<range.lowerBound))
            stdoutBuffer.removeSubrange(0...range.lowerBound)
        }
        lock.unlock()

        for line in lines where !line.isEmpty {
            guard let response = try? JSONDecoder.chadex.decode(HelperResponse.self, from: line) else {
                failAllPending(with: HelperClientError.malformedResponse)
                continue
            }
            resolve(requestId: response.requestId, result: .success(response))
        }
    }

    private func consumeStderr(_ data: Data) {
        guard !data.isEmpty, let text = String(data: data, encoding: .utf8) else { return }
        lock.lock()
        stderrTail = String((stderrTail + text).suffix(8_192))
        lock.unlock()
    }

    private func recordLatency(method: String, durationMilliseconds: Double, succeeded: Bool) {
        lock.lock()
        latencySamples.append(
            HelperLatencySample(
                timestamp: Date(),
                method: method,
                durationMilliseconds: durationMilliseconds,
                succeeded: succeeded
            )
        )
        if latencySamples.count > 100 {
            latencySamples.removeFirst(latencySamples.count - 100)
        }
        lock.unlock()
    }

    private func resolve(
        requestId: String,
        generation: UInt64? = nil,
        result: Result<HelperResponse, Error>
    ) {
        lock.lock()
        let pendingRequest = pending[requestId]
        if let generation,
           let pendingRequest,
           pendingRequest.generation != generation {
            lock.unlock()
            return
        }
        let continuation = pending.removeValue(forKey: requestId)?.continuation
        lock.unlock()
        continuation?.resume(with: result)
    }

    private func failPending(generation: UInt64, with error: Error) {
        lock.lock()
        let requestIds = pending.compactMap { key, value in
            value.generation == generation ? key : nil
        }
        let continuations = requestIds.compactMap { pending.removeValue(forKey: $0)?.continuation }
        lock.unlock()
        for continuation in continuations {
            continuation.resume(throwing: error)
        }
    }

    private func failAllPending(with error: Error) {
        lock.lock()
        let continuations = pending.values.map(\.continuation)
        pending.removeAll()
        lock.unlock()
        for continuation in continuations {
            continuation.resume(throwing: error)
        }
    }

    private static func requestTimeout(for method: String) -> TimeInterval {
        switch method {
        case "connectChatGPT", "startTunnel", "configureLocalSetup", "resumeService":
            return 120
        case "switchLocalProject", "activateProject", "stopLocalService", "disconnectAI", "stopTunnel":
            return 30
        case "shutdown":
            return shutdownRPCTimeout
        default:
            return 10
        }
    }

    private static func resolveExecutableURL() throws -> URL {
        #if DEBUG
        if let override = ProcessInfo.processInfo.environment["CHADEX_HELPER_PATH"], !override.isEmpty {
            return URL(fileURLWithPath: override)
        }
        #endif
        return Bundle.main.bundleURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("Helpers", isDirectory: true)
            .appendingPathComponent("chadex-helper", isDirectory: false)
    }

    static func environmentForHelper(
        baseEnvironment: [String: String] = ProcessInfo.processInfo.environment,
        homeDirectory: URL = FileManager.default.homeDirectoryForCurrentUser,
        isExecutable: (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) -> [String: String] {
        guard let graphifyPath = resolveGraphifyExecutable(
            environment: baseEnvironment,
            homeDirectory: homeDirectory,
            isExecutable: isExecutable
        ) else {
            return baseEnvironment
        }

        var environment = baseEnvironment
        environment["CHADEX_GRAPHIFY_BIN"] = graphifyPath
        let graphifyDirectory = URL(fileURLWithPath: graphifyPath)
            .deletingLastPathComponent()
            .path
        environment["PATH"] = prependPath(graphifyDirectory, to: baseEnvironment["PATH"])
        return environment
    }

    static func resolveGraphifyExecutable(
        environment: [String: String],
        homeDirectory: URL,
        isExecutable: (String) -> Bool
    ) -> String? {
        var candidates: [String] = []
        if let override = environment["CHADEX_GRAPHIFY_BIN"], !override.isEmpty {
            candidates.append(override)
        }
        if let path = environment["PATH"] {
            candidates.append(contentsOf: path
                .split(separator: ":", omittingEmptySubsequences: true)
                .map { URL(fileURLWithPath: String($0)).appendingPathComponent("graphify").path })
        }

        candidates.append(homeDirectory
            .appendingPathComponent(".local/bin/graphify", isDirectory: false)
            .path)

        let pythonRoot = homeDirectory.appendingPathComponent("Library/Python", isDirectory: true)
        if let versions = try? FileManager.default.contentsOfDirectory(
            at: pythonRoot,
            includingPropertiesForKeys: [.isDirectoryKey],
            options: [.skipsHiddenFiles]
        ) {
            candidates.append(contentsOf: versions
                .filter(\.hasDirectoryPath)
                .sorted { $0.path > $1.path }
                .map { $0.appendingPathComponent("bin/graphify", isDirectory: false).path })
        }

        candidates.append(contentsOf: [
            "/opt/homebrew/bin/graphify",
            "/usr/local/bin/graphify"
        ])

        var seen = Set<String>()
        return candidates.first { candidate in
            seen.insert(candidate).inserted && isExecutable(candidate)
        }
    }

    static func prependPath(_ directory: String, to path: String?) -> String {
        var entries = (path ?? "")
            .split(separator: ":", omittingEmptySubsequences: true)
            .map(String.init)
        entries.removeAll { $0 == directory }
        entries.insert(directory, at: 0)
        return entries.joined(separator: ":")
    }

    private static func encodeJSONValue<T: Encodable>(_ value: T) throws -> JSONValue {
        let data = try JSONEncoder.chadex.encode(value)
        return try JSONDecoder.chadex.decode(JSONValue.self, from: data)
    }

    private static func decode<T: Decodable>(_ type: T.Type, from value: JSONValue) throws -> T {
        let data = try JSONEncoder.chadex.encode(value)
        return try JSONDecoder.chadex.decode(T.self, from: data)
    }
}

extension JSONEncoder {
    static var chadex: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        return encoder
    }
}

extension JSONDecoder {
    static var chadex: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }
}
