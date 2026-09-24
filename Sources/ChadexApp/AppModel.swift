import AppKit
import Combine
import Foundation
import OSLog
import ServiceManagement

@MainActor
final class AppModel: ObservableObject {
    private static let startupLogger = Logger(subsystem: "app.chadex.Chadex", category: "startup")
    @Published private(set) var preferences: ChadexPreferences
    @Published private(set) var snapshot: BackendSnapshot = .initial
    @Published private(set) var activities: [ActivityEntry] = []
    @Published private(set) var performanceTraces: [McpPerformanceTraceEntry] = []
    @Published private(set) var lifecyclePerformanceTraces: [LifecyclePerformanceTraceEntry] = []
    @Published var activitySearch = ""
    @Published var activityFilter: ActivityFilter = .all
    @Published var presentedError: PresentedError?
    @Published var isAppActive = true
    @Published var showingConnectionSettings = false
    @Published private(set) var launchAtLoginEnabled = SMAppService.mainApp.status == .enabled
    @Published private(set) var isSwitchingProject = false
    @Published private(set) var switchingProjectName: String?
    @Published private(set) var hasStoredAPIKey = false
    @Published private(set) var connectionAction: ConnectionAction?
    @Published private(set) var connectionActionError: HelperErrorPayload?
    @Published private(set) var connectionCheckInFlight = false
    @Published private(set) var connectionCheckMessage: String?
    @Published private(set) var connectionCheckSucceeded: Bool?
    @Published private(set) var isBootstrapping = false

    enum ActivityFilter: String, CaseIterable, Identifiable {
        case all
        case warningsAndErrors
        var id: String { rawValue }
    }

    struct PresentedError: Identifiable, Equatable {
        let id = UUID()
        var title: String
        var message: String
        var recovery: String
        var code: String?
        var details: String?
    }

    private let helper: HelperClient
    private let keychain: KeychainStore
    private let store: ProjectStore
    private var cachedAPIKey: String?
    private var didLoadAPIKeyFromKeychain = false
    private let keychainACLVersionKey = "app.chadex.keychain-acl-version"
    private let currentKeychainACLVersion = 1
    private var pollingTask: Task<Void, Never>?
    private var refreshInFlight = false
    private var snapshotRequestGate = SnapshotRequestGate()
    private var activityRefreshGate = ActivityRefreshGate()
    private var snapshotFreshnessGate = SnapshotFreshnessGate()
    private var projectSwitchGate = ProjectSwitchGate()
    private var appPhaseTimings: [AppPhaseTimingSample] = []
    private let foregroundRefreshMaxAge: TimeInterval = 1.5

    init(
        helper: HelperClient = HelperClient(),
        keychain: KeychainStore = KeychainStore(),
        store: ProjectStore = ProjectStore(),
        autostart: Bool = false
    ) {
        self.helper = helper
        self.keychain = keychain
        self.store = store
        self.preferences = store.load()
        // Do not touch Keychain during model construction. Bootstrap performs
        // one lazy read and caches it for the lifetime of this AppModel.
        self.hasStoredAPIKey = false
        if autostart {
            Task { @MainActor [weak self] in
                self?.start()
            }
        }
    }

    var projects: [ProjectRecord] { preferences.projects }

    var selectedProject: ProjectRecord? {
        guard let id = preferences.selectedProjectID else { return preferences.projects.first }
        return preferences.projects.first(where: { $0.id == id }) ?? preferences.projects.first
    }

    var filteredActivities: [ActivityEntry] {
        activities.filter { entry in
            let levelMatches = activityFilter == .all || entry.level == .warning || entry.level == .error
            let query = activitySearch.trimmingCharacters(in: .whitespacesAndNewlines)
            let searchMatches = query.isEmpty
                || entry.message.localizedCaseInsensitiveContains(query)
                || entry.source.localizedCaseInsensitiveContains(query)
                || entry.eventKind.localizedCaseInsensitiveContains(query)
            return levelMatches && searchMatches
        }
        .sorted { $0.sequence > $1.sequence }
    }

    var connectionPresentationPhase: ConnectionPhase {
        ConnectionPresentation.phase(
            for: snapshot,
            isBootstrapping: isBootstrapping,
            isSwitchingProject: isSwitchingProject
        )
    }

    var connectionError: HelperErrorPayload? {
        ConnectionPresentation.error(
            for: snapshot,
            actionError: connectionActionError,
            isBootstrapping: isBootstrapping,
            isSwitchingProject: isSwitchingProject
        )
    }

    var connectionActionInFlight: Bool {
        connectionAction != nil
    }

    var connectionActionStatusText: String? {
        switch connectionAction {
        case .connecting:
            return L10n.string("status.connecting")
        case .disconnecting:
            return L10n.string("status.disconnecting")
        case nil:
            return nil
        }
    }

    var connectionActionExplanationText: String? {
        switch connectionAction {
        case .connecting:
            return L10n.string("connection.explainConnecting")
        case .disconnecting:
            return L10n.string("connection.explainDisconnecting")
        case nil:
            return nil
        }
    }

    func start() {
        guard pollingTask == nil else { return }
        isBootstrapping = true
        pollingTask = Task { [weak self] in
            guard let self else { return }
            await self.bootstrap()
            while !Task.isCancelled {
                let nanos = UInt64(self.pollInterval * 1_000_000_000)
                try? await Task.sleep(nanoseconds: nanos)
                guard !Task.isCancelled else { break }
                await self.refreshStatus()
            }
        }
    }

    func applicationBecameActive() {
        isAppActive = true
        // A status error presented on the main window while a configuration
        // sheet is open can create a hidden macOS modal session that steals
        // keyboard and pointer events from the visible sheet.
        guard !showingConnectionSettings else { return }
        let now = ProcessInfo.processInfo.systemUptime
        guard snapshotFreshnessGate.shouldRefresh(at: now, maxAge: foregroundRefreshMaxAge) else {
            return
        }
        Task { await refreshStatus(force: true) }
    }

    func applicationResignedActive() {
        isAppActive = false
    }

    func shutdown() async {
        pollingTask?.cancel()
        await helper.shutdown()
    }

    func addProjectFromPanel() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.prompt = L10n.string("project.choose")
        guard panel.runModal() == .OK, let url = panel.url else { return }
        Task { await addProject(url: url) }
    }

    @discardableResult
    func addProject(url: URL) async -> Bool {
        presentedError = nil
        do {
            let inspection: ProjectInspection = try await helper.request(
                method: "inspectProject",
                params: InspectProjectParams(path: url.path)
            )
            guard inspection.readable else {
                throw HelperErrorPayload(code: "project_not_readable", message: L10n.string("error.projectUnreadable"), recovery: L10n.string("error.chooseAnotherProject"), details: nil)
            }
            if !preferences.projects.contains(where: { $0.path == inspection.path }) {
                preferences.projects.append(ProjectRecord(name: url.lastPathComponent, path: inspection.path))
                try persist()
            }
            guard let id = preferences.projects.first(where: { $0.path == inspection.path })?.id else {
                return false
            }
            let switched = await selectProject(id)
            return switched
        } catch {
            present(error)
            return false
        }
    }

    func inspectProject(url: URL) async throws -> ProjectInspection {
        try await helper.request(method: "inspectProject", params: InspectProjectParams(path: url.path))
    }

    @discardableResult
    func removeProject(_ id: UUID) async -> Bool {
        guard let project = preferences.projects.first(where: { $0.id == id }) else { return true }
        let remaining = preferences.projects.filter { $0.id != id }

        if selectedProject?.id == project.id {
            if let fallback = remaining.first {
                guard await selectProject(fallback.id) else { return false }
            } else {
                do {
                    _ = try await requestSnapshot(method: "stopLocalService", params: EmptyParams())
                    await refreshActivities()
                } catch {
                    present(error)
                    await refreshStatus(force: true)
                    return false
                }
            }
        }

        preferences.projects = remaining
        if remaining.isEmpty {
            preferences.selectedProjectID = nil
        } else if preferences.selectedProjectID == id {
            preferences.selectedProjectID = remaining.first?.id
        }

        do {
            try persist()
            return true
        } catch {
            present(error)
            return false
        }
    }

    @discardableResult
    func selectProject(_ id: UUID) async -> Bool {
        guard projectSwitchGate.begin(id) else { return false }
        defer { projectSwitchGate.finish(id) }
        guard id != preferences.selectedProjectID else { return true }
        guard let project = preferences.projects.first(where: { $0.id == id }) else { return false }
        isSwitchingProject = true
        switchingProjectName = project.name
        defer {
            isSwitchingProject = false
            switchingProjectName = nil
        }
        presentedError = nil
        let switchStarted = ProcessInfo.processInfo.systemUptime
        let switchTimestamp = Date()
        do {
            _ = try await requestSnapshot(
                method: "switchLocalProject",
                params: ActivateProjectParams(path: project.path)
            )
            recordAppPhase(
                timestamp: switchTimestamp,
                operation: "project_switch",
                phase: "activation",
                startedUptime: switchStarted,
                succeeded: true
            )
            preferences.selectedProjectID = id
            try persist()
            await refreshActivities()
            return true
        } catch {
            recordAppPhase(
                timestamp: switchTimestamp,
                operation: "project_switch",
                phase: "activation",
                startedUptime: switchStarted,
                succeeded: false
            )
            present(error)
            await refreshStatus(force: true)
            return false
        }
    }

    @discardableResult
    private func activateStoredSelection() async -> Bool {
        guard let selectedProject else { return true }
        do {
            _ = try await requestSnapshot(method: "activateProject", params: ActivateProjectParams(path: selectedProject.path))
            await refreshActivities()
            return true
        } catch {
            present(error)
            await refreshStatus(force: true)
            return false
        }
    }

    func refreshStatus(force: Bool = false) async {
        if connectionActionInFlight && !force { return }
        if refreshInFlight && !force { return }
        guard !refreshInFlight else { return }
        refreshInFlight = true
        defer { refreshInFlight = false }
        do {
            _ = try await requestSnapshot(method: "getStatus", params: EmptyParams())
            await refreshActivities()
        } catch {
            // Polling is background work. While the user is entering tunnel
            // credentials, surfacing this as a modal alert would interrupt the
            // active text field and can steal the sheet's event handling.
            if !showingConnectionSettings {
                present(error, quietlyIfAlreadyShown: true)
            }
        }
    }

    func refreshPerformanceTraces() async {
        do {
            let mcpTraces: [McpPerformanceTraceEntry] = try await helper.request(
                method: "queryPerformanceTraces",
                params: PerformanceTraceQueryParams(limit: 100)
            )
            let lifecycleTraces: [LifecyclePerformanceTraceEntry] = try await helper.request(
                method: "queryLifecyclePerformanceTraces",
                params: PerformanceTraceQueryParams(limit: 100)
            )
            performanceTraces = mcpTraces
            lifecyclePerformanceTraces = lifecycleTraces
        } catch {
            // Performance diagnostics are secondary and must never affect
            // the connection state or interrupt normal use.
        }
    }

    func refreshDiagnostics() async {
        await refreshStatus(force: true)
        await refreshPerformanceTraces()
    }

    func primaryAction() {
        guard !isSwitchingProject else { return }
        Task {
            switch snapshot.phase {
            case .unconfigured:
                showConnectionSettings()
            case .preparing:
                if let operation = snapshot.currentOperation, operation.cancellable {
                    await cancel(operation)
                }
            case .waitingForChatGPTVerification:
                await disconnectChatGPT()
            case .verified:
                await disconnectChatGPT()
            case .stopped:
                await connectChatGPT()
            case .error:
                // An error phase is actionable: the project and tunnel
                // credentials are already configured, but either the local
                // runtime or the secure tunnel needs recovery. A passive
                // refresh only re-observes the broken state and can trap the
                // Retry button in an endless error loop. Re-run the normal
                // connection path so it restores the runtime first and then
                // starts the tunnel.
                await connectChatGPT()
            }
        }
    }

    func showConnectionSettings() {
        presentedError = nil
        showingConnectionSettings = true
    }

    func clearPresentedError() {
        presentedError = nil
    }

    func cancel(_ operation: OperationSnapshot) async {
        do {
            _ = try await requestSnapshot(method: "cancelOperation", params: CancelOperationParams(operationId: operation.id))
            await refreshStatus(force: true)
        } catch { present(error) }
    }

    func updateTunnelID(_ tunnelID: String) {
        preferences.tunnelID = tunnelID.trimmingCharacters(in: .whitespacesAndNewlines)
        do { try persist() } catch { present(error) }
    }

    @discardableResult
    func saveConnectionSettings(tunnelID: String, apiKey: String) async -> Bool {
        presentedError = nil
        let trimmedTunnelID = tunnelID.trimmingCharacters(in: .whitespacesAndNewlines)
        let trimmedAPIKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)

        if !trimmedAPIKey.isEmpty && trimmedTunnelID.isEmpty {
            present(HelperErrorPayload(
                code: "tunnel_id_missing",
                message: L10n.string("error.tunnelIDMissing"),
                recovery: L10n.string("error.enterTunnelID"),
                details: nil
            ))
            return false
        }
        if !trimmedTunnelID.isEmpty && !Self.isValidTunnelID(trimmedTunnelID) {
            present(HelperErrorPayload(
                code: "tunnel_config_invalid",
                message: L10n.string("error.tunnelIDInvalid"),
                recovery: L10n.string("error.enterValidTunnelID"),
                details: nil
            ))
            return false
        }
        if !trimmedAPIKey.isEmpty && !Self.isValidAPIKey(trimmedAPIKey) {
            present(HelperErrorPayload(
                code: "tunnel_config_invalid",
                message: L10n.string("error.apiKeyInvalid"),
                recovery: L10n.string("error.enterValidAPIKey"),
                details: nil
            ))
            return false
        }

        do {
            let keyToUse = trimmedAPIKey.isEmpty ? try loadAPIKeyOnce() : trimmedAPIKey
            let shouldReconnect = snapshot.tunnelReady
                || snapshot.chatGPTConnected
                || snapshot.chatGPTVerifiedForSelectedProject
                || snapshot.phase == .waitingForChatGPTVerification

            if !trimmedAPIKey.isEmpty {
                try keychain.saveAPIKey(trimmedAPIKey)
                cacheAPIKey(trimmedAPIKey)
            }

            preferences.tunnelID = trimmedTunnelID
            try persist()

            guard !trimmedTunnelID.isEmpty, let keyToUse, !keyToUse.isEmpty else {
                _ = try await requestSnapshot(method: "clearCredential", params: EmptyParams())
                preferences.restoreConnectionOnLaunch = false
                try persist()
                await refreshActivities()
                return true
            }

            _ = try await sendCredentialSnapshot(tunnelID: trimmedTunnelID, apiKey: keyToUse)
            if shouldReconnect {
                Task { [weak self] in
                    await self?.reconnectChatGPTAfterSettingsChange()
                }
                return true
            }
            await refreshActivities()
            return true
        } catch {
            present(error)
            await refreshStatus(force: true)
            return false
        }
    }

    private func reconnectChatGPTAfterSettingsChange() async {
        do {
            _ = try await requestSnapshot(method: "disconnectAI", params: EmptyParams())
            _ = await connectChatGPT()
        } catch {
            present(error)
            await refreshStatus(force: true)
        }
    }

    func saveAPIKey(_ apiKey: String) async -> Bool {
        let trimmed = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let tunnelID = preferences.tunnelID.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return false }
        guard !tunnelID.isEmpty else {
            present(HelperErrorPayload(
                code: "tunnel_id_missing",
                message: L10n.string("error.tunnelIDMissing"),
                recovery: L10n.string("error.enterTunnelID"),
                details: nil
            ))
            return false
        }
        do {
            try keychain.saveAPIKey(trimmed)
            cacheAPIKey(trimmed)
            _ = try await sendCredentialSnapshot(tunnelID: tunnelID, apiKey: trimmed)
            return true
        } catch {
            present(error)
            return false
        }
    }

    @discardableResult
    func clearAPIKey() async -> Bool {
        presentedError = nil
        do {
            _ = try await requestSnapshot(method: "clearCredential", params: EmptyParams())
            try keychain.deleteAPIKey()
            cacheAPIKey(nil)
            preferences.restoreConnectionOnLaunch = false
            try persist()
            await refreshActivities()
            return true
        } catch {
            present(error)
            await refreshStatus(force: true)
            return false
        }
    }

    @discardableResult
    func pushStoredCredentialIfAvailable() async -> Bool {
        do {
            if let key = try loadAPIKeyOnce(), !key.isEmpty, !preferences.tunnelID.isEmpty {
                _ = try await sendCredentialSnapshot(tunnelID: preferences.tunnelID, apiKey: key)
            }
            return true
        } catch {
            present(error, quietlyIfAlreadyShown: true)
            return false
        }
    }

    func setRestoreService(_ value: Bool) {
        preferences.restoreServiceOnLaunch = value
        do { try persist() } catch { present(error) }
    }

    func setRestoreConnection(_ value: Bool) {
        preferences.restoreConnectionOnLaunch = value
        do { try persist() } catch { present(error) }
    }

    func stopLocalService() {
        Task { await invoke(method: "stopLocalService") }
    }

    func checkConnection() {
        Task { await checkConnectionNow() }
    }

    func setLaunchAtLogin(_ value: Bool) {
        do {
            if value {
                try SMAppService.mainApp.register()
            } else {
                try SMAppService.mainApp.unregister()
            }
            launchAtLoginEnabled = SMAppService.mainApp.status == .enabled
        } catch {
            launchAtLoginEnabled = SMAppService.mainApp.status == .enabled
            present(error)
        }
    }

    func exportDiagnostics() {
        Task { await exportDiagnosticsReport() }
    }

    private func exportDiagnosticsReport() async {
        await refreshPerformanceTraces()
        let mcpTraces = performanceTraces
        let lifecycleTraces = lifecyclePerformanceTraces
        let appTimings = appPhaseTimings
        let helperSamples = helper.performanceSamples(limit: 50)

        let panel = NSSavePanel()
        panel.title = L10n.string("settings.exportDiagnostics")
        panel.prompt = L10n.string("settings.export")
        panel.nameFieldStringValue = "Chadex-Diagnostics-\(Self.diagnosticsFileTimestamp()).txt"
        panel.canCreateDirectories = true

        guard panel.runModal() == .OK, let url = panel.url else { return }

        do {
            try diagnosticsReport(
                mcpTraces: mcpTraces,
                lifecycleTraces: lifecycleTraces,
                appTimings: appTimings,
                helperSamples: helperSamples
            ).write(to: url, atomically: true, encoding: .utf8)
        } catch {
            present(error)
        }
    }

    private func diagnosticsReport(
        mcpTraces: [McpPerformanceTraceEntry],
        lifecycleTraces: [LifecyclePerformanceTraceEntry],
        appTimings: [AppPhaseTimingSample],
        helperSamples: [HelperLatencySample]
    ) -> String {
        let appVersion = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development"
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "—"
        let language = UserDefaults.standard.string(forKey: ChadexPreferenceKey.language) ?? ChadexLanguage.system.rawValue
        let interfaceSize = UserDefaults.standard.string(forKey: ChadexPreferenceKey.interfaceSize) ?? ChadexInterfaceSize.comfortable.rawValue
        let appearance = UserDefaults.standard.string(forKey: ChadexPreferenceKey.appearance) ?? ChadexAppearance.system.rawValue

        var lines: [String] = [
            "Chadex Diagnostics",
            "Generated: \(Date().ISO8601Format())",
            "",
            "App",
            "Version: \(appVersion) (\(build))",
            "macOS: \(ProcessInfo.processInfo.operatingSystemVersionString)",
            "Language: \(language)",
            "Interface size: \(interfaceSize)",
            "Appearance: \(appearance)",
            "",
            "Connection",
            "Phase: \(snapshot.phase.rawValue)",
            "Tunnel ready: \(snapshot.tunnelReady)",
            "ChatGPT connected: \(snapshot.chatGPTConnected)",
            "Project verified: \(snapshot.chatGPTVerifiedForSelectedProject)",
            "Tunnel ID: \(Self.abbreviatedTunnelID(preferences.tunnelID))",
            "API key stored: \(hasStoredAPIKey)",
            "API key value: EXCLUDED",
            "Selected project: \(selectedProject?.name ?? "—")",
            "Activity entries: \(activities.count)"
        ]

        if let graphify = snapshot.graphify {
            lines.append("Graphify available: \(graphify.available)")
            lines.append("Graphify path: \(graphify.path ?? "—")")
            lines.append("Graphify source: \(graphify.source)")
        }

        if let operation = snapshot.currentOperation {
            lines.append("Current operation: \(operation.kind) / \(operation.phase)")
        }

        lines.append("")
        lines.append("Performance")
        lines.append("Helper round trips: \(helperSamples.count)")
        lines.append("App lifecycle timings: \(appTimings.count)")
        lines.append("Helper lifecycle traces: \(lifecycleTraces.count)")
        lines.append("MCP ingress traces: \(mcpTraces.count)")

        if !helperSamples.isEmpty {
            let average = helperSamples.map(\.durationMilliseconds).reduce(0, +) / Double(helperSamples.count)
            lines.append(String(format: "Helper average: %.2f ms", average))
            lines.append("Recent helper round trips")
            for sample in helperSamples.suffix(20) {
                lines.append(
                    String(
                        format: "[%@] %@ %.2f ms %@",
                        sample.timestamp.ISO8601Format(),
                        sample.method,
                        sample.durationMilliseconds,
                        sample.succeeded ? "ok" : "error"
                    )
                )
            }
        }

        if !appTimings.isEmpty {
            lines.append("Recent app lifecycle timings")
            for timing in appTimings.suffix(30) {
                lines.append(
                    String(
                        format: "[%@] operation=%@ phase=%@ total=%.2fms %@",
                        timing.timestamp.ISO8601Format(),
                        timing.operation,
                        timing.phase,
                        timing.durationMilliseconds,
                        timing.succeeded ? "ok" : "error"
                    )
                )
            }
        }

        if !lifecycleTraces.isEmpty {
            lines.append("Recent helper lifecycle traces")
            for trace in lifecycleTraces.suffix(50) {
                lines.append(
                    String(
                        format: "[%@] operation=%@ phase=%@ total=%.2fms completion=%@",
                        trace.date.ISO8601Format(),
                        trace.operation,
                        trace.phase,
                        trace.totalMilliseconds,
                        trace.completion
                    )
                )
            }
        }

        if !mcpTraces.isEmpty {
            let averageUs = mcpTraces.map(\.totalUs).reduce(UInt64(0), &+)
                / UInt64(mcpTraces.count)
            lines.append(String(format: "MCP average: %.2f ms", Double(averageUs) / 1_000.0))
            lines.append("Recent MCP ingress traces")
            for trace in mcpTraces.suffix(50) {
                let methods = trace.methods.isEmpty ? "—" : trace.methods.joined(separator: ",")
                let tools = trace.toolNames.isEmpty ? "—" : trace.toolNames.joined(separator: ",")
                let status = trace.statusCode.map(String.init) ?? "—"
                lines.append(
                    String(
                        format: "[%@] methods=%@ tools=%@ total=%.2fms pre_backend=%.2fms backend_headers=%.2fms response_stream=%.2fms request=%lluB response=%lluB status=%@ completion=%@",
                        trace.date.ISO8601Format(Date.ISO8601FormatStyle(includingFractionalSeconds: true)),
                        methods,
                        tools,
                        Double(trace.totalUs) / 1_000.0,
                        Double(trace.ingressPreBackendUs) / 1_000.0,
                        Double(trace.backendHeadersUs) / 1_000.0,
                        Double(trace.responseStreamUs) / 1_000.0,
                        trace.requestBytes,
                        trace.responseBytes,
                        status,
                        trace.completion
                    )
                )
                lines.append("  started_at_ms=\(trace.startedAtMs) finished_at_ms=\(trace.finishedAtMs.map(String.init) ?? "—") server_trace_id=\(trace.serverTraceId ?? "—") request_id_hashes=\((trace.requestIdHashes ?? []).joined(separator: ","))")
            }
        }

        lines.append("")
        lines.append("Recent Activity")

        // Diagnostics must never trigger Keychain authentication UI. The
        // credential was already loaded during bootstrap if it exists.
        let storedSecret = cachedAPIKey

        for entry in activities.suffix(50) {
            var message = entry.message
            if let storedSecret, !storedSecret.isEmpty {
                message = message.replacingOccurrences(of: storedSecret, with: "[REDACTED]")
            }
            lines.append(
                "[\(entry.date.ISO8601Format())] [\(entry.level.rawValue)] [\(entry.source)] [\(entry.eventKind)] \(message)"
            )
        }

        return lines.joined(separator: "\n") + "\n"
    }

    private static func abbreviatedTunnelID(_ value: String) -> String {
        guard !value.isEmpty else { return "—" }
        guard value.count > 16 else { return value }
        return "\(value.prefix(8))…\(value.suffix(5))"
    }

    private static func diagnosticsFileTimestamp() -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyyMMdd-HHmmss"
        return formatter.string(from: Date())
    }

    func showBackgroundCloseHintIfNeeded() {
        guard !preferences.backgroundCloseHintShown else { return }
        preferences.backgroundCloseHintShown = true
        try? persist()
        let alert = NSAlert()
        alert.messageText = L10n.string("background.title")
        alert.informativeText = L10n.string("background.message")
        alert.alertStyle = .informational
        alert.addButton(withTitle: L10n.string("common.ok"))
        alert.runModal()
    }

    private var pollInterval: TimeInterval {
        if snapshot.taskProgress?.isActive == true { return 1 }
        if snapshot.currentOperation != nil { return 1 }
        if snapshot.tunnelReady && !snapshot.chatGPTConnected { return isAppActive ? 2 : 10 }
        return isAppActive ? 5 : 15
    }

    private func cacheAPIKey(_ value: String?) {
        cachedAPIKey = value
        didLoadAPIKeyFromKeychain = true
        hasStoredAPIKey = value?.isEmpty == false
    }

    private func loadAPIKeyOnce() throws -> String? {
        if didLoadAPIKeyFromKeychain {
            return cachedAPIKey
        }
        let value = try keychain.loadAPIKey()
        if let value, !value.isEmpty {
            try migrateKeychainACLIfNeeded(value)
        }
        cacheAPIKey(value)
        return value
    }

    private func migrateKeychainACLIfNeeded(_ value: String) throws {
        let defaults = UserDefaults.standard
        guard defaults.integer(forKey: keychainACLVersionKey) < currentKeychainACLVersion else {
            return
        }
        // Credentials created by the old ad-hoc signed builds can keep an ACL
        // tied to the previous CDHash. Recreate the exact item once under the
        // stable Apple Development requirement so later launches do not need
        // repeated authorization. No secret is persisted outside Keychain.
        try keychain.recreateAPIKeyForCurrentSigner(value)
        defaults.set(currentKeychainACLVersion, forKey: keychainACLVersionKey)
    }

    private func bootstrap() async {
        let bootstrapTimestamp = Date()
        let bootstrapStarted = ProcessInfo.processInfo.systemUptime
        var bootstrapSucceeded = false
        defer {
            isBootstrapping = false
            recordAppPhase(
                timestamp: bootstrapTimestamp,
                operation: "bootstrap",
                phase: "total",
                startedUptime: bootstrapStarted,
                succeeded: bootstrapSucceeded
            )
        }

        do {
            let helperTimestamp = Date()
            let helperStarted = ProcessInfo.processInfo.systemUptime
            do {
                try await helper.startIfNeededAsync()
                recordAppPhase(
                    timestamp: helperTimestamp,
                    operation: "bootstrap",
                    phase: "helper_start",
                    startedUptime: helperStarted,
                    succeeded: true
                )
            } catch {
                recordAppPhase(
                    timestamp: helperTimestamp,
                    operation: "bootstrap",
                    phase: "helper_start",
                    startedUptime: helperStarted,
                    succeeded: false
                )
                throw error
            }

            let credentialTimestamp = Date()
            let credentialStarted = ProcessInfo.processInfo.systemUptime
            let credentialSucceeded = await pushStoredCredentialIfAvailable()
            recordAppPhase(
                timestamp: credentialTimestamp,
                operation: "bootstrap",
                phase: "credential_push",
                startedUptime: credentialStarted,
                succeeded: credentialSucceeded
            )

            if selectedProject != nil {
                let projectTimestamp = Date()
                let projectStarted = ProcessInfo.processInfo.systemUptime
                let projectSucceeded = await activateStoredSelection()
                recordAppPhase(
                    timestamp: projectTimestamp,
                    operation: "bootstrap",
                    phase: "project_activation",
                    startedUptime: projectStarted,
                    succeeded: projectSucceeded
                )
                if preferences.restoreConnectionOnLaunch {
                    let connectionTimestamp = Date()
                    let connectionStarted = ProcessInfo.processInfo.systemUptime
                    let connectionSucceeded = await connectChatGPT()
                    recordAppPhase(
                        timestamp: connectionTimestamp,
                        operation: "bootstrap",
                        phase: "connection_restore",
                        startedUptime: connectionStarted,
                        succeeded: connectionSucceeded
                    )
                } else if preferences.restoreServiceOnLaunch {
                    let serviceTimestamp = Date()
                    let serviceStarted = ProcessInfo.processInfo.systemUptime
                    await invoke(method: "resumeService")
                    recordAppPhase(
                        timestamp: serviceTimestamp,
                        operation: "bootstrap",
                        phase: "service_restore",
                        startedUptime: serviceStarted,
                        succeeded: snapshot.error == nil
                    )
                }
            } else {
                await refreshStatus(force: true)
            }
            bootstrapSucceeded = true
        } catch {
            present(error)
        }
    }

    private func recordAppPhase(
        timestamp: Date = Date(),
        operation: String,
        phase: String,
        startedUptime: TimeInterval,
        succeeded: Bool
    ) {
        let durationMilliseconds = max(
            0,
            (ProcessInfo.processInfo.systemUptime - startedUptime) * 1_000
        )
        appPhaseTimings.append(
            AppPhaseTimingSample(
                timestamp: timestamp,
                operation: operation,
                phase: phase,
                durationMilliseconds: durationMilliseconds,
                succeeded: succeeded
            )
        )
        Self.startupLogger.notice(
            "operation=\(operation, privacy: .public) phase=\(phase, privacy: .public) duration_ms=\(durationMilliseconds, privacy: .public) succeeded=\(succeeded, privacy: .public)"
        )
        if appPhaseTimings.count > 100 {
            appPhaseTimings.removeFirst(appPhaseTimings.count - 100)
        }
    }

    private func refreshActivities(force: Bool = false) async {
        let targetSequence = snapshot.activitySequence
        guard force || activityRefreshGate.shouldRefresh(for: targetSequence) else { return }
        do {
            let result: [ActivityEntry] = try await helper.request(method: "queryActivities", params: ActivityQueryParams(limit: 200))
            activities = Array(result.suffix(200))
            activityRefreshGate.markLoaded(targetSequence)
        } catch {
            // Status remains authoritative even when the secondary activity query fails.
            // Leave the gate unchanged so the next refresh can retry.
        }
    }

    func cancelTask(_ task: TaskProgressSnapshot) async {
        guard task.canCancel else { return }
        do {
            _ = try await requestSnapshot(
                method: "cancelTask",
                params: CancelTaskParams(project: task.project, taskId: task.taskId)
            )
        } catch {
            present(error)
            await refreshStatus(force: true)
        }
    }

    private func invoke(method: String) async {
        if method == "disconnectAI" || method == "stopTunnel" || method == "stopLocalService" {
            connectionCheckMessage = nil
            connectionCheckSucceeded = nil
        }
        do {
            _ = try await requestSnapshot(method: method, params: EmptyParams())
            await refreshActivities()
        } catch {
            present(error)
            await refreshStatus(force: true)
        }
    }

    @discardableResult
    private func connectChatGPT() async -> Bool {
        guard !connectionActionInFlight else { return false }
        connectionActionError = nil
        connectionCheckMessage = nil
        connectionCheckSucceeded = nil
        connectionAction = .connecting
        defer { connectionAction = nil }
        do {
            _ = try await requestSnapshot(method: "connectChatGPT", params: EmptyParams())
            if !snapshot.tunnelReady && snapshot.phase == .stopped {
                let payload = HelperErrorPayload(
                    code: "connection_state_inconsistent",
                    message: L10n.string("error.connectionNotConfirmed"),
                    recovery: L10n.string("error.connectionNotConfirmedRecovery"),
                    details: nil
                )
                connectionActionError = payload
                present(payload)
                await refreshStatus(force: true)
                return false
            }
            await refreshActivities()
            return true
        } catch {
            connectionActionError = connectionPayload(from: error)
            present(error)
            await refreshStatus(force: true)
            return false
        }
    }

    @discardableResult
    private func disconnectChatGPT() async -> Bool {
        guard !connectionActionInFlight else { return false }
        connectionActionError = nil
        connectionCheckMessage = nil
        connectionCheckSucceeded = nil
        connectionAction = .disconnecting
        defer { connectionAction = nil }
        do {
            _ = try await requestSnapshot(method: "disconnectAI", params: EmptyParams())
            await refreshActivities()
            return true
        } catch {
            connectionActionError = connectionPayload(from: error)
            present(error)
            await refreshStatus(force: true)
            return false
        }
    }

    private func checkConnectionNow() async {
        guard !connectionActionInFlight, !connectionCheckInFlight else { return }
        connectionCheckInFlight = true
        connectionCheckMessage = nil
        connectionCheckSucceeded = nil
        defer { connectionCheckInFlight = false }

        do {
            _ = try await requestSnapshot(method: "getStatus", params: EmptyParams())
            await refreshActivities()

            if snapshot.chatGPTVerifiedForSelectedProject {
                connectionCheckSucceeded = true
                connectionCheckMessage = L10n.string("connection.checkVerified")
            } else if snapshot.chatGPTConnected {
                connectionCheckSucceeded = true
                connectionCheckMessage = L10n.string("connection.checkChatGPTSeen")
            } else if snapshot.tunnelReady {
                connectionCheckSucceeded = true
                connectionCheckMessage = L10n.string("connection.checkTunnelReady")
            } else {
                connectionCheckSucceeded = false
                connectionCheckMessage = L10n.string("connection.checkNotReady")
            }
        } catch {
            connectionCheckSucceeded = false
            connectionCheckMessage = L10n.string("connection.checkFailed")
            present(error)
        }
    }

    @discardableResult
    private func applySnapshot(_ candidate: BackendSnapshot, requestSequence: UInt64? = nil) -> Bool {
        if let requestSequence {
            guard snapshotRequestGate.shouldApply(requestSequence) else { return false }
        } else if candidate.stateRevision < snapshot.stateRevision {
            return false
        }
        snapshot = candidate
        snapshotFreshnessGate.markApplied(at: ProcessInfo.processInfo.systemUptime)
        if candidate.tunnelReady || candidate.chatGPTConnected || candidate.phase == .verified {
            connectionActionError = nil
        }
        if !candidate.tunnelReady {
            connectionCheckMessage = nil
            connectionCheckSucceeded = nil
        }
        return true
    }

    private func beginSnapshotRequest() -> UInt64 {
        snapshotRequestGate.issue()
    }

    @discardableResult
    private func requestSnapshot<Params: Encodable & Sendable>(
        method: String,
        params: Params
    ) async throws -> BackendSnapshot {
        let requestSequence = beginSnapshotRequest()
        let candidate: BackendSnapshot = try await helper.request(method: method, params: params)
        applySnapshot(candidate, requestSequence: requestSequence)
        return candidate
    }

    @discardableResult
    private func sendCredentialSnapshot(tunnelID: String, apiKey: String) async throws -> BackendSnapshot {
        let requestSequence = beginSnapshotRequest()
        let candidate = try await helper.sendCredential(tunnelID: tunnelID, apiKey: apiKey)
        applySnapshot(candidate, requestSequence: requestSequence)
        return candidate
    }

    private func connectionPayload(from error: Error) -> HelperErrorPayload {
        if let helperError = error as? HelperClientError, case .backend(let payload) = helperError {
            return payload
        }
        if let payload = error as? HelperErrorPayload {
            return payload
        }
        return HelperErrorPayload(
            code: "connection_failed",
            message: error.localizedDescription,
            recovery: L10n.string("error.retry"),
            details: nil
        )
    }

    private func persist() throws {
        try store.save(preferences)
        objectWillChange.send()
    }

    private static func isValidTunnelID(_ value: String) -> Bool {
        let bytes = Array(value.utf8)
        guard !bytes.isEmpty, bytes.count <= 256 else { return false }
        return bytes.allSatisfy { byte in
            (byte >= 48 && byte <= 57)
                || (byte >= 65 && byte <= 90)
                || (byte >= 97 && byte <= 122)
                || byte == 95
                || byte == 45
        }
    }

    private static func isValidAPIKey(_ value: String) -> Bool {
        let bytes = Array(value.utf8)
        guard !bytes.isEmpty, bytes.count <= 8192 else { return false }
        return bytes.allSatisfy { $0 >= 0x21 && $0 <= 0x7E }
    }

    private func present(_ error: Error, quietlyIfAlreadyShown: Bool = false) {
        if quietlyIfAlreadyShown, presentedError != nil { return }
        if let helperError = (error as? HelperClientError), case .backend(let payload) = helperError {
            presentedError = PresentedError(
                title: L10n.string("error.title"),
                message: payload.message,
                recovery: payload.recovery ?? L10n.string("error.retry"),
                code: payload.code,
                details: payload.details.map(String.init(describing:))
            )
            return
        }
        if let payload = error as? HelperErrorPayload {
            presentedError = PresentedError(title: L10n.string("error.title"), message: payload.message, recovery: payload.recovery ?? L10n.string("error.retry"), code: payload.code, details: nil)
            return
        }
        presentedError = PresentedError(
            title: L10n.string("error.title"),
            message: error.localizedDescription,
            recovery: L10n.string("error.retry"),
            code: nil,
            details: nil
        )
    }
}
