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
    @Published private(set) var mascotTraces: [McpPerformanceTraceEntry] = []
    @Published private(set) var performanceTraces: [McpPerformanceTraceEntry] = []
    @Published private(set) var lifecyclePerformanceTraces: [LifecyclePerformanceTraceEntry] = []
    @Published private(set) var computerSafety: ComputerSafetyStatus = .initial
    @Published private(set) var computerSafetyError: String?
    @Published private(set) var computerSafetyMutationInFlight = false
    @Published var activitySearch = ""
    @Published var activityFilter: ActivityFilter = .all
    @Published var presentedError: PresentedError?
    @Published var isAppActive = true
    @Published var showingConnectionSettings = false
    @Published private(set) var launchAtLoginEnabled = SMAppService.mainApp.status == .enabled
    @Published private(set) var isSwitchingProject = false
    /// Re-activating the selected project in a runtime that serves another
    /// one. Counts as connection work so Connect, Disconnect and project
    /// switches wait for it instead of contending for the runtime.
    @Published private(set) var isRealigningRuntimeProject = false
    @Published private(set) var switchingProjectName: String?
    @Published private(set) var hasStoredAPIKey = false
    @Published private(set) var connectionAction: ConnectionAction?
    @Published private(set) var connectionActionError: HelperErrorPayload?
    @Published private(set) var connectionCheckInFlight = false
    @Published private(set) var connectionCheckMessage: String?
    @Published private(set) var connectionCheckSucceeded: Bool?
    @Published private(set) var isBootstrapping = false
    @Published private(set) var globalInstructions = ""
    @Published private(set) var globalInstructionsError: String?
    @Published private(set) var globalInstructionsSaving = false
    @Published private(set) var skillCatalog: SkillCatalogInspection?
    @Published private(set) var skillInventory: SkillInventoryInspection?
    @Published private(set) var skillDefinitions: [String: SkillDefinitionPreview] = [:]
    @Published private(set) var skillsLoading = false
    @Published private(set) var skillsError: String?
    @Published private(set) var skillDefinitionLoadingIDs: Set<String> = []
    @Published private(set) var skillMutationInFlightIDs: Set<String> = []
    @Published private(set) var projectSkillWriteInFlight = false
    @Published private(set) var skillInstallInFlight = false
    @Published private(set) var externalSkillSources: ExternalSkillSourceDiscovery?
    @Published private(set) var externalSkillRoots: ExternalSkillRootsState?
    @Published private(set) var externalSkillsLoading = false
    @Published private(set) var externalSkillsApplying = false
    @Published private(set) var externalSkillsError: String?
    @Published private(set) var projectMemoryCatalog: ProjectMemoryCatalog?
    @Published private(set) var projectMemoryRecords: [String: ProjectMemoryRecord] = [:]
    @Published private(set) var projectMemoryLoading = false
    @Published private(set) var projectMemoryError: String?
    @Published private(set) var projectMemoryReadLoadingKeys: Set<String> = []
    @Published private(set) var projectMemoryMutationInFlightKeys: Set<String> = []

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
    private(set) var runtimePrewarmTask: Task<Void, Never>?
    /// Code of the last Skill catalog or inventory failure, used to recognise a
    /// runtime that serves another project than the selected one.
    private var skillsInventoryErrorCode: String?
    /// The selection a runtime/project realignment was already attempted for
    /// (one automatic attempt per selection, so a failure cannot loop).
    private(set) var runtimeProjectRealignAttemptedID: UUID?
    private(set) var isShuttingDown = false
    private let pollWaker = PollWaker()
    private var refreshInFlight = false
    /// The model-owned Skills load in flight, shared by every caller. It is not
    /// a child of the caller's task, so leaving the Skills page (which cancels
    /// the page's `.task`) cannot cancel the load or leave it half-applied.
    private var skillsLoadTask: Task<Void, Never>?
    private var skillsLoadToken: UUID?
    private var skillsLoadProjectID: UUID?
    /// A refresh was requested while a load for the same project was already
    /// running (for example the runtime became ready meanwhile): the running
    /// load reads once more instead of dropping the request.
    private var skillsReloadRequested = false
    private var skillsLastRefreshUptime: TimeInterval?
    private var projectMemoryRefreshProjectID: UUID?
    private var projectMemoryLastRefreshUptime: TimeInterval?
    private var snapshotRequestGate = SnapshotRequestGate()
    private var activityRefreshGate = ActivityRefreshGate()
    private var snapshotFreshnessGate = SnapshotFreshnessGate()
    private var projectSwitchGate = ProjectSwitchGate()
    private var appPhaseTimings: [AppPhaseTimingSample] = []
    private let foregroundRefreshMaxAge: TimeInterval = 1.5
    // After the tunnel becomes ready, ChatGPT's first MCP request is what turns
    // the connection green. Users usually switch to ChatGPT at this point, so
    // keep a fast poll even in the background for a bounded window.
    private var awaitingChatGPTSinceUptime: TimeInterval?
    private let awaitingChatGPTFastPollWindow: TimeInterval = 120
    private let skillsRefreshMaxAge: TimeInterval = 5
    private let projectMemoryRefreshMaxAge: TimeInterval = 5

    init(
        helper: HelperClient = HelperClient(),
        // The explicit motion-review launch uses an isolated credential scope.
        keychain: KeychainStore = KeychainStore(service: FerretReviewMode.enabled
            ? "app.chadex.ferret-review.credentials" : "app.chadex.credentials"),
        store: ProjectStore = ProjectStore(),
        autostart: Bool = false
    ) {
        self.helper = helper
        self.keychain = keychain
        self.store = store
        self.preferences = store.load()
        do {
            self.globalInstructions = try store.loadGlobalInstructions()
        } catch {
            self.globalInstructions = ""
            self.globalInstructionsError = error.localizedDescription
        }
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

    var computerControlDefaultMode: ComputerControlMode {
        preferences.computerControlDefaultMode ?? .askBeforeControl
    }

    nonisolated static func shouldConnectAfterSavingConnectionSettings(
        hasSelectedProject: Bool
    ) -> Bool {
        hasSelectedProject
    }

    static let skillDraftTemplate = """
    ## Workflow
    1. Describe the reusable procedure this Skill should follow.
    2. Add concrete checks, commands, or decision rules.

    ## Boundaries
    - This Skill does not override Project Instructions or Chadex permissions.
    """

    var skillCenterItems: [SkillCenterItem] {
        let managedByID = Dictionary(uniqueKeysWithValues: (skillInventory?.skills ?? []).map { ($0.skillId, $0) })
        var items = (skillCatalog?.skills ?? []).map { descriptor in
            SkillCenterItem(
                skillId: descriptor.skillId,
                name: descriptor.name,
                description: descriptor.description,
                definitionRevision: descriptor.definitionRevision,
                packageRevision: descriptor.packageRevision,
                sourceScope: descriptor.sourceScope,
                trust: descriptor.trust,
                nameConflict: descriptor.nameConflict,
                scriptsAllowed: descriptor.scriptsAllowed,
                managed: managedByID[descriptor.skillId]
            )
        }
        let catalogIDs = Set(items.map(\.skillId))
        for managed in managedByID.values where !catalogIDs.contains(managed.skillId) {
            items.append(SkillCenterItem(
                skillId: managed.skillId,
                name: managed.name,
                description: managed.description,
                definitionRevision: managed.definitionRevision,
                packageRevision: managed.preferredPackageRevision,
                sourceScope: "runner",
                trust: "operator_installed_guidance",
                nameConflict: false,
                scriptsAllowed: true,
                managed: managed
            ))
        }
        return items.sorted {
            let ordered = $0.name.localizedCaseInsensitiveCompare($1.name)
            return ordered == .orderedSame ? $0.skillId < $1.skillId : ordered == .orderedAscending
        }
    }

    /// Newest first, ignoring the Activity page's search and filter.
    var recentActivities: [ActivityEntry] {
        activities.sorted { $0.sequence > $1.sequence }
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
            isSwitchingProject: isSwitchingProject,
            isConnecting: connectionAction == .connecting
        )
    }

    var connectionError: HelperErrorPayload? {
        ConnectionPresentation.error(
            for: snapshot,
            actionError: connectionActionError,
            isBootstrapping: isBootstrapping,
            isSwitchingProject: isSwitchingProject,
            isConnecting: connectionAction == .connecting
        )
    }

    var connectionActionInFlight: Bool {
        connectionAction != nil || isRealigningRuntimeProject
    }

    var hasUpdateBlockingWork: Bool {
        isBootstrapping ||
            isSwitchingProject ||
            connectionActionInFlight ||
            snapshot.currentOperation != nil ||
            snapshot.taskProgress?.isActive == true
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
            var lastPollUptime = ProcessInfo.processInfo.systemUptime
            while !Task.isCancelled {
                // Sleep exactly until the next poll is due. State changes that
                // shorten the interval wake this wait through `pollWaker`, so
                // there is no fixed-rate ticking while idle.
                let remaining = lastPollUptime + self.pollInterval
                    - ProcessInfo.processInfo.systemUptime
                if remaining > 0.01 {
                    await self.pollWaker.wait(seconds: remaining)
                    continue
                }
                guard !Task.isCancelled, !self.isShuttingDown else { break }
                await self.refreshStatus()
                lastPollUptime = ProcessInfo.processInfo.systemUptime
            }
        }
    }

    func applicationBecameActive() {
        isAppActive = true
        pollWaker.signal()
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
        // Stop everything that could talk to (or relaunch) the helper before
        // asking it to quit: the poll loop and any in-flight launch warm-up.
        isShuttingDown = true
        pollingTask?.cancel()
        runtimePrewarmTask?.cancel()
        pollWaker.signal()
        await helper.shutdown()
        await runtimePrewarmTask?.value
    }

    func addProjectFromPanel() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        panel.prompt = L10n.string("project.choose")
        guard panel.runModal() == .OK, let url = panel.url else { return }
        guard confirmBroadProjectAccess(url) else { return }
        Task { await addProject(url: url) }
    }

    /// A folder as broad as the home folder (or wider) lets ChatGPT reach
    /// every file under it: ask before adding it. Cancel is the default.
    private func confirmBroadProjectAccess(_ url: URL) -> Bool {
        let path = url.resolvingSymlinksInPath().standardizedFileURL.path
        let home = FileManager.default.homeDirectoryForCurrentUser
            .resolvingSymlinksInPath().standardizedFileURL.path
        guard ProjectPathBreadth.isBroad(path, homeDirectory: home) else { return true }
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = L10n.string("project.broadAccessTitle")
        alert.informativeText = L10n.string("project.broadAccessMessage", path)
        let cancel = alert.addButton(withTitle: L10n.string("common.cancel"))
        cancel.keyEquivalent = "\r"
        let addAnyway = alert.addButton(withTitle: L10n.string("project.broadAccessConfirm"))
        addAnyway.keyEquivalent = ""
        addAnyway.hasDestructiveAction = true
        return alert.runModal() == .alertSecondButtonReturn
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

        // A switch to this project is still running: removing it now would
        // leave the selection pointing at a project that is no longer listed
        // while the runtime serves it. Say so instead of doing nothing.
        if projectSwitchGate.activeProjectID == id {
            presentProjectRemovalBusy(project)
            return false
        }

        if selectedProject?.id == project.id {
            // Removing the current project switches to another one first;
            // while a switch is already running that cannot start, so tell
            // the user instead of silently keeping the project.
            if projectSwitchGate.activeProjectID != nil {
                presentProjectRemovalBusy(project)
                return false
            }
            if let fallback = remaining.first {
                guard await selectProject(fallback.id) else {
                    if let switchError = presentedError {
                        // Keep the project and say why: removing the current
                        // project needs a switch first, and that switch failed.
                        presentedError = PresentedError(
                            title: L10n.string("project.removeSwitchFailedTitle"),
                            message: L10n.string(
                                "project.removeSwitchFailedMessage",
                                project.name,
                                fallback.name,
                                switchError.message
                            ),
                            recovery: L10n.string("project.removeSwitchFailedRecovery", project.name),
                            code: switchError.code,
                            details: switchError.details
                        )
                    } else {
                        presentProjectRemovalBusy(project)
                    }
                    return false
                }
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

    private func presentProjectRemovalBusy(_ project: ProjectRecord) {
        presentedError = PresentedError(
            title: L10n.string("project.removeBusyTitle"),
            message: L10n.string("project.removeBusyMessage", project.name),
            recovery: L10n.string("project.removeBusyRecovery"),
            code: "project_switch_in_progress",
            details: nil
        )
    }

    /// Switches the runtime to the project. The switching state (spinner,
    /// disabled controls) covers only the activation itself: reloading the
    /// project's Skills, memory and activity, and on failure the status
    /// refresh, happen after it ends, so a slow secondary read can never keep
    /// the UI in "Switching to …" once the switch is decided.
    @discardableResult
    func selectProject(_ id: UUID) async -> Bool {
        // A background realignment owns the runtime briefly; an explicit
        // choice waits for it (bounded) instead of being silently dropped.
        guard await waitForRuntimeProjectRealign() else { return false }
        guard projectSwitchGate.begin(id) else { return false }
        if id == preferences.selectedProjectID {
            projectSwitchGate.finish(id)
            await refreshSkills()
            await refreshProjectMemory()
            return true
        }
        guard let project = preferences.projects.first(where: { $0.id == id }) else {
            projectSwitchGate.finish(id)
            return false
        }
        isSwitchingProject = true
        switchingProjectName = project.name
        presentedError = nil
        let switchStarted = ProcessInfo.processInfo.systemUptime
        let switchTimestamp = Date()
        var switchError: Error?
        do {
            _ = try await requestSnapshot(
                method: "switchLocalProject",
                params: ActivateProjectParams(path: project.path)
            )
        } catch {
            switchError = error
        }
        // The App can stop waiting (timeout) while the helper still completes
        // the switch. The helper's selection is authoritative: adopt it rather
        // than keep a selection the runtime no longer serves.
        if switchError != nil,
           let status = try? await requestSnapshot(
               method: "getStatus",
               params: MascotStatusParams(includeMascotJobs: false)
           ),
           status.selectedProject?.path == project.path {
            switchError = nil
        }
        recordAppPhase(
            timestamp: switchTimestamp,
            operation: "project_switch",
            phase: "activation",
            startedUptime: switchStarted,
            succeeded: switchError == nil
        )
        if let switchError {
            endProjectSwitch(id)
            present(switchError)
            await refreshStatus(force: true)
            return false
        }
        preferences.selectedProjectID = id
        do {
            try persist()
        } catch {
            endProjectSwitch(id)
            present(error)
            return false
        }
        clearSkills()
        clearProjectMemory()
        endProjectSwitch(id)
        await refreshActivities()
        await refreshSkills()
        await refreshProjectMemory()
        return true
    }

    private func endProjectSwitch(_ id: UUID) {
        isSwitchingProject = false
        switchingProjectName = nil
        projectSwitchGate.finish(id)
    }

    /// Waits until a background project realignment has finished. Returns
    /// false only if it did not finish in time or the app is shutting down.
    private func waitForRuntimeProjectRealign(timeout: TimeInterval = 50) async -> Bool {
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        while isRealigningRuntimeProject {
            if isShuttingDown || Task.isCancelled
                || ProcessInfo.processInfo.systemUptime >= deadline {
                return false
            }
            try? await Task.sleep(for: .milliseconds(25))
        }
        return !isShuttingDown
    }

    @discardableResult
    private func activateStoredSelection() async -> Bool {
        guard let selectedProject else { return true }
        do {
            _ = try await requestSnapshot(method: "activateProject", params: ActivateProjectParams(path: selectedProject.path))
            await refreshActivities()
            clearSkills()
            clearProjectMemory()
            await refreshSkills()
            await refreshProjectMemory()
            return true
        } catch {
            present(error)
            await refreshStatus(force: true)
            return false
        }
    }

    func refreshStatus(force: Bool = false) async {
        // After shutdown began a request would relaunch the helper that is
        // being stopped.
        guard !isShuttingDown else { return }
        if connectionActionInFlight && !force { return }
        if refreshInFlight && !force { return }
        guard !refreshInFlight else { return }
        refreshInFlight = true
        defer { refreshInFlight = false }
        do {
            _ = try await requestSnapshot(method: "getStatus", params: MascotStatusParams(
                includeMascotJobs: UserDefaults.standard.object(forKey: "ferret.visible") as? Bool != false
            ))
            await refreshComputerSafety()
            await refreshActivities()
            await refreshMascotTraces()
            let now = ProcessInfo.processInfo.systemUptime
            let skillObservationIsStale = skillsLastRefreshUptime
                .map { now - $0 >= skillsRefreshMaxAge }
                ?? true
            let memoryObservationIsStale = projectMemoryLastRefreshUptime
                .map { now - $0 >= projectMemoryRefreshMaxAge }
                ?? true
            if force || skillObservationIsStale {
                await refreshSkills()
            }
            if force || memoryObservationIsStale {
                await refreshProjectMemory()
            }
        } catch {
            // Polling is background work. While the user is entering tunnel
            // credentials, surfacing this as a modal alert would interrupt the
            // active text field and can steal the sheet's event handling.
            if !showingConnectionSettings {
                present(error, quietlyIfAlreadyShown: true)
            }
        }
    }


    func refreshComputerSafety() async {
        do {
            let status: ComputerSafetyStatus = try await helper.request(
                method: "getComputerSafety",
                params: EmptyParams()
            )
            computerSafety = status
            computerSafetyError = nil
        } catch {
            computerSafetyError = error.localizedDescription
        }
    }

    func setComputerControlMode(_ mode: ComputerControlMode) async {
        guard !computerSafetyMutationInFlight else { return }
        computerSafetyMutationInFlight = true
        defer { computerSafetyMutationInFlight = false }
        do {
            let status: ComputerSafetyStatus = try await helper.request(
                method: "setComputerControlMode",
                params: ComputerControlModeParams(mode: mode)
            )
            computerSafety = status
            computerSafetyError = nil
        } catch {
            computerSafetyError = error.localizedDescription
            present(error)
        }
    }

    func setComputerControlDefaultMode(_ mode: ComputerControlMode) async {
        guard mode != .allowSession else {
            await setComputerControlMode(mode)
            return
        }
        let previous = preferences.computerControlDefaultMode
        preferences.computerControlDefaultMode = mode
        do {
            try persist()
        } catch {
            preferences.computerControlDefaultMode = previous
            computerSafetyError = error.localizedDescription
            present(error)
            return
        }
        await setComputerControlMode(mode)
    }

    func resumeComputerControl() async {
        guard !computerSafetyMutationInFlight else { return }
        computerSafetyMutationInFlight = true
        defer { computerSafetyMutationInFlight = false }
        do {
            let status: ComputerSafetyStatus = try await helper.request(
                method: "resumeComputerControl",
                params: EmptyParams()
            )
            computerSafety = status
            computerSafetyError = nil
        } catch {
            computerSafetyError = error.localizedDescription
            present(error)
        }
    }

    func alwaysAllowComputerControl(_ approval: ComputerApproval) async {
        guard !computerSafetyMutationInFlight else { return }
        computerSafetyMutationInFlight = true
        defer { computerSafetyMutationInFlight = false }
        let previous = preferences.computerControlDefaultMode
        preferences.computerControlDefaultMode = .alwaysAllow
        do {
            try persist()
            let status: ComputerSafetyStatus = try await helper.request(
                method: "approveComputerControlAlways",
                params: ComputerApprovalParams(approvalId: approval.approvalId)
            )
            computerSafety = status
            computerSafetyError = nil
        } catch {
            preferences.computerControlDefaultMode = previous
            try? persist()
            computerSafetyError = error.localizedDescription
            present(error)
            await refreshComputerSafety()
        }
    }

    func approveComputerControl(_ approval: ComputerApproval) async {
        await resolveComputerApproval(approval, method: "approveComputerControl")
    }

    func denyComputerControl(_ approval: ComputerApproval) async {
        await resolveComputerApproval(approval, method: "denyComputerControl")
    }

    func stopComputerControl() async {
        guard !computerSafetyMutationInFlight else { return }
        computerSafetyMutationInFlight = true
        defer { computerSafetyMutationInFlight = false }
        do {
            let status: ComputerSafetyStatus = try await helper.request(
                method: "stopComputerControl",
                params: EmptyParams()
            )
            computerSafety = status
            computerSafetyError = nil
        } catch {
            computerSafetyError = error.localizedDescription
            present(error)
        }
    }

    private func resolveComputerApproval(_ approval: ComputerApproval, method: String) async {
        guard !computerSafetyMutationInFlight else { return }
        computerSafetyMutationInFlight = true
        defer { computerSafetyMutationInFlight = false }
        do {
            let status: ComputerSafetyStatus = try await helper.request(
                method: method,
                params: ComputerApprovalParams(approvalId: approval.approvalId)
            )
            computerSafety = status
            computerSafetyError = nil
        } catch {
            computerSafetyError = error.localizedDescription
            await refreshComputerSafety()
        }
    }

    @discardableResult
    func saveGlobalInstructions(_ content: String) -> Bool {
        guard !globalInstructionsSaving else { return false }
        globalInstructionsSaving = true
        defer { globalInstructionsSaving = false }
        do {
            try store.saveGlobalInstructions(content)
            globalInstructions = content
            globalInstructionsError = nil
            return true
        } catch {
            globalInstructionsError = error.localizedDescription
            return false
        }
    }

    func refreshSkills() async {
        await loadSkills()
        guard skillsInventoryErrorCode == "project_runtime_mismatch",
              await realignRuntimeProjectIfNeeded() else { return }
        await loadSkills()
    }

    /// After the runtime became usable in the background (launch warm-up),
    /// loads Skills again unless they are already loaded: a load made while the
    /// runtime was still starting only got an error, and nothing else would
    /// retry before the next stale poll.
    func reloadSkillsIfNotLoaded() async {
        guard selectedProject != nil, !isShuttingDown,
              skillCatalog == nil || skillInventory == nil else { return }
        await refreshSkills()
    }

    /// The local runtime is ready but serves another project than the
    /// selected one (for example after its saved project went stale). This is
    /// recoverable without user input: re-activate the selected project once
    /// per selection. Skipped while other work owns the runtime (launch,
    /// warm-up — which aligns the project itself — switching, connecting) and
    /// while the runtime is not ready, which does not use up the attempt. The
    /// helper's `realignLocalProject` only activates: it never touches the
    /// tunnel or the selection, so a failure leaves the connection as it was.
    @discardableResult
    func realignRuntimeProjectIfNeeded() async -> Bool {
        guard let selectedProject,
              runtimeProjectRealignAttemptedID != selectedProject.id,
              snapshot.runtimeReady == true,
              !isBootstrapping,
              !isSwitchingProject,
              !connectionActionInFlight,
              runtimePrewarmTask == nil,
              !isShuttingDown else { return false }
        runtimeProjectRealignAttemptedID = selectedProject.id
        isRealigningRuntimeProject = true
        defer { isRealigningRuntimeProject = false }
        do {
            let realigned = try await requestSnapshot(
                method: "realignLocalProject",
                params: ActivateProjectParams(path: selectedProject.path)
            )
            if realigned.runtimeReady != true {
                // Nothing was attempted: keep the chance for a ready runtime.
                runtimeProjectRealignAttemptedID = nil
                return false
            }
            await refreshActivities()
            return self.selectedProject?.id == selectedProject.id
        } catch {
            return false
        }
    }

    /// Loads the selected project's Skills. Concurrent callers share one
    /// model-owned load (a caller arriving mid-load makes it read once more and
    /// waits for the result), and `skillsLoading` is cleared when that load
    /// ends, whatever happens to the callers' tasks.
    private func loadSkills() async {
        guard let selectedProject else {
            clearSkills()
            return
        }
        let projectID = selectedProject.id
        if let running = skillsLoadTask, skillsLoadProjectID == projectID {
            skillsReloadRequested = true
            await running.value
            return
        }
        let token = UUID()
        skillsLoadToken = token
        skillsLoadProjectID = projectID
        skillsReloadRequested = false
        skillsLoading = true
        let task = Task { [weak self] in
            guard let self else { return }
            repeat {
                self.skillsReloadRequested = false
                await self.performSkillsLoad(projectID: projectID)
            } while self.skillsReloadRequested
                && self.skillsLoadToken == token
                && self.selectedProject?.id == projectID
                && !self.isShuttingDown
            if self.skillsLoadToken == token {
                self.skillsLoadTask = nil
                self.skillsLoadToken = nil
                self.skillsLoadProjectID = nil
                self.skillsReloadRequested = false
                self.skillsLoading = false
            }
        }
        skillsLoadTask = task
        await task.value
    }

    private func performSkillsLoad(projectID requestedProjectID: UUID) async {
        guard let selectedProject, selectedProject.id == requestedProjectID else { return }
        defer {
            if self.selectedProject?.id == requestedProjectID {
                skillsLastRefreshUptime = ProcessInfo.processInfo.systemUptime
            }
        }
        do {
            let catalog: SkillCatalogInspection = try await helper.request(
                method: "getSkillCatalog",
                params: InspectProjectParams(path: selectedProject.path)
            )
            guard self.selectedProject?.id == requestedProjectID else { return }

            var inventory: SkillInventoryInspection?
            var inventoryWarning: String?
            var inventoryErrorCode: String?
            do {
                inventory = try await helper.request(
                    method: "getSkillInventory",
                    params: InspectProjectParams(path: selectedProject.path)
                )
            } catch {
                inventoryWarning = helperErrorMessage(error)
                inventoryErrorCode = helperErrorPayload(error)?.code
            }
            guard self.selectedProject?.id == requestedProjectID else { return }
            skillsInventoryErrorCode = inventoryErrorCode

            skillCatalog = catalog
            skillInventory = inventory
            skillsError = inventoryWarning

            let currentByID = Dictionary(uniqueKeysWithValues: skillCenterItems.map { ($0.skillId, $0) })
            skillDefinitions = skillDefinitions.filter { skillID, preview in
                guard let item = currentByID[skillID], item.isActive else { return false }
                return preview.definitionRevision == item.definitionRevision
                    && preview.packageRevision == item.packageRevision
            }
        } catch {
            guard self.selectedProject?.id == requestedProjectID else { return }
            skillCatalog = nil
            skillInventory = nil
            skillDefinitions.removeAll()
            // The catalog checks the runtime project first, so a runtime that
            // serves another project usually fails here, not in the inventory.
            skillsInventoryErrorCode = helperErrorPayload(error)?.code
            skillsError = helperErrorMessage(error)
        }
    }

    func loadSkillDefinition(_ item: SkillCenterItem) async {
        guard item.canLoadDefinition, let selectedProject else { return }
        if let preview = skillDefinitions[item.skillId],
           preview.definitionRevision == item.definitionRevision,
           preview.packageRevision == item.packageRevision {
            return
        }
        guard !skillDefinitionLoadingIDs.contains(item.skillId) else { return }
        let requestedProjectID = selectedProject.id
        skillDefinitionLoadingIDs.insert(item.skillId)
        defer { skillDefinitionLoadingIDs.remove(item.skillId) }
        do {
            let preview: SkillDefinitionPreview = try await helper.request(
                method: "getSkillDefinition",
                params: SkillDefinitionParams(
                    path: selectedProject.path,
                    skillId: item.skillId,
                    definitionRevision: item.definitionRevision,
                    packageRevision: item.packageRevision
                )
            )
            guard self.selectedProject?.id == requestedProjectID else { return }
            guard let current = skillCenterItems.first(where: { $0.skillId == item.skillId }),
                  current.isActive,
                  current.definitionRevision == preview.definitionRevision,
                  current.packageRevision == preview.packageRevision else {
                await refreshSkills()
                return
            }
            skillDefinitions[item.skillId] = preview
            skillsError = nil
        } catch {
            guard self.selectedProject?.id == requestedProjectID else { return }
            skillsError = helperErrorMessage(error)
            await refreshSkills()
        }
    }

    @discardableResult
    func createProjectSkill(skillKey: String, description: String, instructions: String) async -> Bool {
        guard !projectSkillWriteInFlight, let selectedProject else { return false }
        let key = skillKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let summary = description.trimmingCharacters(in: .whitespacesAndNewlines)
        let body = instructions.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty, !summary.isEmpty, !body.isEmpty else { return false }
        projectSkillWriteInFlight = true
        defer { projectSkillWriteInFlight = false }
        let content = """
        ---
        name: \(key)
        description: \(Self.yamlQuoted(summary))
        ---

        \(body)
        """
        do {
            let _: SkillOperationResult = try await helper.request(
                method: "createProjectSkill",
                params: CreateProjectSkillParams(path: selectedProject.path, skillKey: key, content: content)
            )
            skillsError = nil
            await refreshSkills()
            return true
        } catch {
            let writeError = helperErrorMessage(error)
            await refreshSkills()
            skillsError = writeError
            return false
        }
    }

    /// Installs a Skill ZIP from any location. The runtime only reads project-relative artifacts,
    /// so the ZIP is copied into `.chadex/skill-imports/` for the call and removed afterwards,
    /// whether the install succeeds or fails.
    @discardableResult
    func installSkill(skillKey: String, archiveURL: URL) async -> Bool {
        guard !skillInstallInFlight, let selectedProject else { return false }
        let key = skillKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else { return false }
        skillInstallInFlight = true
        defer { skillInstallInFlight = false }
        let staged: StagedSkillArchive
        do {
            // Off the main actor: the source may be a slow volume or a not-yet-downloaded iCloud file.
            let projectRoot = URL(fileURLWithPath: selectedProject.path, isDirectory: true)
            staged = try await Task.detached(priority: .userInitiated) {
                try SkillArchiveStaging.stage(source: archiveURL, projectRoot: projectRoot)
            }.value
        } catch {
            skillsError = error.localizedDescription
            return false
        }
        defer { staged.remove() }
        do {
            let _: SkillOperationResult = try await helper.request(
                method: "installSkill",
                params: InstallSkillParams(path: selectedProject.path, skillKey: key, artifactPath: staged.relativePath)
            )
            skillsError = nil
            await refreshSkills()
            return true
        } catch {
            let installError = helperErrorPayload(error)
                .map { SkillManagementErrorMessage.installMessage(forCode: $0.code, helperMessage: $0.message) }
                ?? error.localizedDescription
            await refreshSkills()
            skillsError = installError
            return false
        }
    }

    func refreshExternalSkillSources() async {
        guard !externalSkillsLoading, !externalSkillsApplying else { return }
        externalSkillsLoading = true
        defer { externalSkillsLoading = false }
        do {
            externalSkillSources = try await helper.request(
                method: "discoverExternalSkillSources",
                params: EmptyParams()
            )
            externalSkillsError = nil
        } catch {
            externalSkillSources = nil
            externalSkillsError = externalSkillErrorMessage(error)
        }
        do {
            externalSkillRoots = try await helper.request(
                method: "getExternalSkillRoots",
                params: EmptyParams()
            )
        } catch {
            externalSkillRoots = nil
            externalSkillsError = externalSkillErrorMessage(error)
        }
    }

    /// Replaces the configured external roots with the full desired lists.
    @discardableResult
    func applyExternalSkillRoots(roots: [String], scriptRoots: [String]) async -> Bool {
        guard !externalSkillsApplying, let current = externalSkillRoots else { return false }
        externalSkillsApplying = true
        defer { externalSkillsApplying = false }
        let rootSet = Set(roots)
        let params = SetExternalSkillRootsParams(
            roots: roots,
            scriptRoots: scriptRoots.filter { rootSet.contains($0) },
            expectedRevision: current.revision,
            verifyProjectPath: selectedProject?.path
        )
        do {
            externalSkillRoots = try await helper.request(method: "setExternalSkillRoots", params: params)
            externalSkillsError = nil
            skillsLastRefreshUptime = nil
            await refreshSkills()
            return true
        } catch {
            let message = externalSkillErrorMessage(error)
            let code = helperErrorPayload(error)?.code
            if code == "external_skill_roots_conflict" || code == "external_skill_roots_state_unknown" {
                if let fresh: ExternalSkillRootsState = try? await helper.request(
                    method: "getExternalSkillRoots",
                    params: EmptyParams()
                ) {
                    externalSkillRoots = fresh
                }
            }
            externalSkillsError = message
            return false
        }
    }

    private func externalSkillErrorMessage(_ error: Error) -> String {
        guard let payload = helperErrorPayload(error) else { return error.localizedDescription }
        var path = ""
        if case .object(let details)? = payload.details, case .string(let value)? = details["path"] {
            path = value
        }
        switch payload.code {
        case "external_skill_roots_conflict": return L10n.string("skills.external.error.conflict")
        case "skill_root_invalid", "skill_root_not_found", "skill_root_is_link",
             "skill_root_not_directory", "skill_root_not_canonical":
            return L10n.string("skills.external.error.rootInvalid", path)
        case "skill_root_sensitive": return L10n.string("skills.external.error.rootSensitive", path)
        case "runner_config_rejected": return L10n.string("skills.external.error.rejected")
        case "runner_config_reload_failed", "runner_config_reload_unsupported", "runtime_unreachable":
            return L10n.string("skills.external.error.reload")
        case "runner_config_restart_required": return L10n.string("skills.external.error.restart")
        case "runtime_not_ready": return L10n.string("skills.external.error.notReady")
        case "external_skill_roots_unverified": return L10n.string("skills.external.error.unverified")
        case "external_skill_roots_state_unknown": return L10n.string("skills.external.error.stateUnknown")
        default: return SkillManagementErrorMessage.message(forCode: payload.code) ?? payload.message
        }
    }

    func setManagedSkillEnabled(_ item: SkillCenterItem, enabled: Bool) async {
        guard let selectedProject, let managed = item.managed else { return }
        guard !skillMutationInFlightIDs.contains(item.skillId) else { return }
        skillMutationInFlightIDs.insert(item.skillId)
        defer { skillMutationInFlightIDs.remove(item.skillId) }
        do {
            if enabled {
                let _: SkillOperationResult = try await helper.request(
                    method: "activateSkill",
                    params: ActivateSkillParams(
                        path: selectedProject.path,
                        skillKey: managed.skillKey,
                        packageRevision: managed.preferredPackageRevision,
                        stateRevision: managed.stateRevision
                    )
                )
            } else {
                let _: SkillOperationResult = try await helper.request(
                    method: "deactivateSkill",
                    params: DeactivateSkillParams(
                        path: selectedProject.path,
                        skillKey: managed.skillKey,
                        stateRevision: managed.stateRevision
                    )
                )
                skillDefinitions.removeValue(forKey: item.skillId)
            }
            skillsError = nil
            await refreshSkills()
        } catch {
            // Refresh first: it resets `skillsError`, which would hide this failure.
            let toggleError = helperErrorMessage(error)
            await refreshSkills()
            skillsError = toggleError
        }
    }

    /// Removes every stored version of an installed Skill. Project and external Skills have no
    /// `managed` entry and are never removed. The helper disables an enabled Skill first.
    @discardableResult
    func removeManagedSkill(_ item: SkillCenterItem) async -> Bool {
        guard let selectedProject, let managed = item.managed else { return false }
        guard !skillMutationInFlightIDs.contains(item.skillId) else { return false }
        skillMutationInFlightIDs.insert(item.skillId)
        defer { skillMutationInFlightIDs.remove(item.skillId) }
        do {
            let _: SkillRemoveResult = try await helper.request(
                method: "removeSkill",
                params: RemoveSkillParams(
                    path: selectedProject.path,
                    skillKey: managed.skillKey,
                    stateRevision: managed.stateRevision
                )
            )
            skillDefinitions.removeValue(forKey: item.skillId)
            skillsError = nil
            await refreshSkills()
            return true
        } catch {
            let removeError = helperErrorMessage(error)
            skillDefinitions.removeValue(forKey: item.skillId)
            await refreshSkills()
            skillsError = removeError
            return false
        }
    }

    func refreshProjectMemory() async {
        guard let selectedProject else {
            clearProjectMemory()
            return
        }
        let requestedProjectID = selectedProject.id
        guard projectMemoryRefreshProjectID != requestedProjectID else { return }
        projectMemoryRefreshProjectID = requestedProjectID
        projectMemoryLoading = true
        defer {
            if projectMemoryRefreshProjectID == requestedProjectID {
                projectMemoryRefreshProjectID = nil
                projectMemoryLoading = false
            }
            if self.selectedProject?.id == requestedProjectID {
                projectMemoryLastRefreshUptime = ProcessInfo.processInfo.systemUptime
            }
        }
        do {
            let catalog: ProjectMemoryCatalog = try await helper.request(
                method: "getProjectMemoryCatalog",
                params: InspectProjectParams(path: selectedProject.path)
            )
            guard self.selectedProject?.id == requestedProjectID else { return }
            projectMemoryCatalog = catalog
            projectMemoryError = nil
            let revisionsByKey = Dictionary(uniqueKeysWithValues: catalog.memories.map { ($0.memoryKey, $0.revision) })
            projectMemoryRecords = projectMemoryRecords.filter { key, record in
                revisionsByKey[key] == record.revision
            }
        } catch {
            guard self.selectedProject?.id == requestedProjectID else { return }
            projectMemoryCatalog = nil
            projectMemoryRecords.removeAll()
            projectMemoryError = helperErrorMessage(error)
        }
    }

    func loadProjectMemory(_ descriptor: ProjectMemoryDescriptor) async {
        guard let selectedProject else { return }
        if let record = projectMemoryRecords[descriptor.memoryKey], record.revision == descriptor.revision {
            return
        }
        guard !projectMemoryReadLoadingKeys.contains(descriptor.memoryKey) else { return }
        let requestedProjectID = selectedProject.id
        projectMemoryReadLoadingKeys.insert(descriptor.memoryKey)
        defer { projectMemoryReadLoadingKeys.remove(descriptor.memoryKey) }
        do {
            let record: ProjectMemoryRecord = try await helper.request(
                method: "getProjectMemory",
                params: ProjectMemoryReadParams(
                    path: selectedProject.path,
                    memoryKey: descriptor.memoryKey,
                    expectedRevision: descriptor.revision
                )
            )
            guard self.selectedProject?.id == requestedProjectID else { return }
            guard let current = projectMemoryCatalog?.memories.first(where: { $0.memoryKey == descriptor.memoryKey }),
                  current.revision == record.revision else {
                await refreshProjectMemory()
                return
            }
            projectMemoryRecords[descriptor.memoryKey] = record
            projectMemoryError = nil
        } catch {
            guard self.selectedProject?.id == requestedProjectID else { return }
            projectMemoryRecords.removeValue(forKey: descriptor.memoryKey)
            let message = isProjectMemoryConflict(error)
                ? L10n.string("memory.conflict")
                : helperErrorMessage(error)
            await refreshProjectMemory()
            guard self.selectedProject?.id == requestedProjectID else { return }
            projectMemoryError = message
        }
    }

    @discardableResult
    func saveProjectMemory(
        memoryKey: String,
        summary: String,
        body: String,
        priority: String,
        bootstrap: Bool,
        tags: [String],
        expectedRevision: String?
    ) async -> Bool {
        guard let selectedProject else { return false }
        let key = memoryKey.trimmingCharacters(in: .whitespacesAndNewlines)
        let summary = summary.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty, !summary.isEmpty else { return false }
        guard ["high", "normal", "low"].contains(priority) else { return false }
        guard !projectMemoryMutationInFlightKeys.contains(key) else { return false }
        projectMemoryMutationInFlightKeys.insert(key)
        defer { projectMemoryMutationInFlightKeys.remove(key) }
        do {
            let _: ProjectMemorySetResult = try await helper.request(
                method: "setProjectMemory",
                params: ProjectMemorySetParams(
                    path: selectedProject.path,
                    memoryKey: key,
                    summary: summary,
                    body: body,
                    priority: priority,
                    bootstrap: bootstrap,
                    tags: tags,
                    expectedRevision: expectedRevision
                )
            )
            projectMemoryRecords.removeValue(forKey: key)
            projectMemoryError = nil
            await refreshProjectMemory()
            return true
        } catch {
            let message = isProjectMemoryConflict(error)
                ? L10n.string("memory.conflict")
                : helperErrorMessage(error)
            await refreshProjectMemory()
            projectMemoryError = message
            return false
        }
    }

    @discardableResult
    func deleteProjectMemory(_ descriptor: ProjectMemoryDescriptor) async -> Bool {
        guard let selectedProject else { return false }
        let key = descriptor.memoryKey
        guard !projectMemoryMutationInFlightKeys.contains(key) else { return false }
        projectMemoryMutationInFlightKeys.insert(key)
        defer { projectMemoryMutationInFlightKeys.remove(key) }
        do {
            let _: ProjectMemoryDeleteResult = try await helper.request(
                method: "deleteProjectMemory",
                params: ProjectMemoryDeleteParams(
                    path: selectedProject.path,
                    memoryKey: key,
                    expectedRevision: descriptor.revision
                )
            )
            projectMemoryRecords.removeValue(forKey: key)
            projectMemoryError = nil
            await refreshProjectMemory()
            return true
        } catch {
            let message = isProjectMemoryConflict(error)
                ? L10n.string("memory.conflict")
                : helperErrorMessage(error)
            await refreshProjectMemory()
            projectMemoryError = message
            return false
        }
    }

    private func clearProjectMemory() {
        projectMemoryCatalog = nil
        projectMemoryRecords.removeAll()
        projectMemoryError = nil
        projectMemoryReadLoadingKeys.removeAll()
        projectMemoryMutationInFlightKeys.removeAll()
        projectMemoryLastRefreshUptime = nil
    }

    private func isProjectMemoryConflict(_ error: Error) -> Bool {
        guard let payload = helperErrorPayload(error) else { return false }
        return payload.code == "memory_changed" || payload.code == "memory_expected_revision_required"
    }

    private func clearSkills() {
        skillCatalog = nil
        skillInventory = nil
        skillDefinitions.removeAll()
        skillsError = nil
        skillsInventoryErrorCode = nil
        skillDefinitionLoadingIDs.removeAll()
        skillMutationInFlightIDs.removeAll()
        skillsLastRefreshUptime = nil
        externalSkillsError = nil
    }

    private func helperErrorPayload(_ error: Error) -> HelperErrorPayload? {
        if let helperError = error as? HelperClientError, case .backend(let payload) = helperError {
            return payload
        }
        return error as? HelperErrorPayload
    }

    private func helperErrorMessage(_ error: Error) -> String {
        guard let payload = helperErrorPayload(error) else { return error.localizedDescription }
        return SkillManagementErrorMessage.message(forCode: payload.code) ?? payload.message
    }

    private static func yamlQuoted(_ value: String) -> String {
        "\"" + value
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n") + "\""
    }

    private func refreshMascotTraces() async {
        guard snapshot.tunnelReady, UserDefaults.standard.object(forKey: "ferret.visible") as? Bool != false else {
            mascotTraces = []
            return
        }
        do {
            mascotTraces = try await helper.request(
                method: "queryPerformanceTraces", params: PerformanceTraceQueryParams(limit: 100)
            )
        } catch {
            // Never leave stale requests animating as live work after a failed observation.
            mascotTraces = []
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

    /// Title for the primary connection action, shared by the project
    /// overview, the main menu and the menu bar extra so one action keeps
    /// one name everywhere.
    var primaryActionTitle: String {
        if let actionText = connectionActionStatusText { return actionText }
        switch connectionPresentationPhase {
        case .unconfigured: return L10n.string("connection.configure")
        case .preparing: return snapshot.currentOperation?.cancellable == true ? L10n.string("connection.cancel") : L10n.string("status.preparing")
        case .waitingForChatGPTVerification: return L10n.string("connection.disconnect")
        case .verified: return L10n.string("connection.disconnect")
        case .stopped: return L10n.string("connection.connect")
        case .error: return L10n.string("connection.retry")
        }
    }

    /// Whether the primary connection action can run now. Shared by the
    /// overview button, the File menu and the menu bar extra so none of them
    /// can trigger a connection the others deliberately block.
    var primaryActionEnabled: Bool {
        guard selectedProject != nil, !connectionActionInFlight, !isSwitchingProject else { return false }
        if connectionPresentationPhase == .preparing {
            // Bootstrap can present `.error` as `.preparing`; only a real,
            // cancellable operation is actionable here.
            return snapshot.phase == .preparing && snapshot.currentOperation?.cancellable == true
        }
        return true
    }

    /// Menu titles name their object, since a bare "Cancel" or "Retry"
    /// is ambiguous outside the overview.
    var primaryMenuActionTitle: String {
        if let actionText = connectionActionStatusText { return actionText }
        switch connectionPresentationPhase {
        case .preparing:
            return snapshot.currentOperation?.cancellable == true
                ? L10n.string("connection.cancelMenu")
                : L10n.string("status.preparing")
        case .error: return L10n.string("connection.retryMenu")
        default: return primaryActionTitle
        }
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
            let hadActiveConnection = snapshot.tunnelReady
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

            let shouldConnect = Self.shouldConnectAfterSavingConnectionSettings(
                hasSelectedProject: selectedProject != nil
            )
            if shouldConnect {
                if hadActiveConnection {
                    _ = try await requestSnapshot(method: "disconnectAI", params: EmptyParams())
                }
                guard await connectChatGPT() else { return false }
            }

            await refreshActivities()
            return true
        } catch {
            present(error)
            await refreshStatus(force: true)
            return false
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

    func setPrepareServiceOnLaunch(_ value: Bool) {
        preferences.prepareServiceOnLaunch = value
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
        if snapshot.tunnelReady && !snapshot.chatGPTConnected,
           let since = awaitingChatGPTSinceUptime,
           ProcessInfo.processInfo.systemUptime - since < awaitingChatGPTFastPollWindow {
            return 1
        }
        if snapshot.tunnelReady && snapshot.chatGPTConnected {
            return isAppActive ? 1 : 5
        }
        if snapshot.tunnelReady && UserDefaults.standard.object(forKey: "ferret.visible") as? Bool != false {
            return isAppActive ? 1 : 5
        }
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

            await applyComputerControlDefaultForNewSession()

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
                } else {
                    // Restoring the connection already starts the local runtime
                    // itself; only when it is off is the background warm-up the
                    // way to prepare it (never both, so it is started once).
                    startRuntimePrewarmIfEnabled()
                }
            } else {
                await refreshStatus(force: true)
            }
            bootstrapSucceeded = true
        } catch {
            present(error)
        }
    }

    /// Resumes an already configured local runtime in the background so a
    /// later Connect only has to start the tunnel. Controlled by the "prepare
    /// the local service at launch" setting. The helper skips this when the
    /// user explicitly stopped the service, does not publish it as a user
    /// operation (the UI keeps offering Connect), and joins/cancels it for
    /// requests that contend with it. Failures stay quiet: Connect still runs
    /// the full recovery path and reports real errors.
    func startRuntimePrewarmIfEnabled() {
        guard preferences.prepareServiceOnLaunchEnabled,
              selectedProject != nil,
              !isShuttingDown,
              runtimePrewarmTask == nil else { return }
        runtimePrewarmTask = Task { [weak self] in
            guard let self else { return }
            let timestamp = Date()
            let started = ProcessInfo.processInfo.systemUptime
            let succeeded: Bool
            do {
                _ = try await self.requestSnapshot(method: "prewarmRuntime", params: EmptyParams())
                succeeded = true
            } catch {
                succeeded = false
            }
            self.recordAppPhase(
                timestamp: timestamp,
                operation: "bootstrap",
                phase: "runtime_prewarm",
                startedUptime: started,
                succeeded: succeeded
            )
            self.runtimePrewarmTask = nil
            if !succeeded, !Task.isCancelled, !self.isShuttingDown {
                await self.refreshStatus(force: true)
            } else if succeeded, !Task.isCancelled {
                // Skills opened during the warm-up only saw a starting runtime.
                await self.reloadSkillsIfNotLoaded()
            }
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

    private func applyComputerControlDefaultForNewSession() async {
        guard !computerSafety.stopped else { return }
        let mode = computerControlDefaultMode
        do {
            let status: ComputerSafetyStatus = try await helper.request(
                method: "setComputerControlMode",
                params: ComputerControlModeParams(mode: mode)
            )
            computerSafety = status
            computerSafetyError = nil
        } catch {
            // Fail closed: the helper defaults to Ask before control if this
            // preference cannot be applied. Connection recovery remains usable.
            computerSafetyError = error.localizedDescription
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
            await applyComputerControlDefaultForNewSession()
            let candidate = try await requestSnapshot(method: "connectChatGPT", params: EmptyParams())
            if !candidate.tunnelReady && candidate.phase == .stopped {
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
            if !computerSafety.stopped && computerSafety.mode == .allowSession {
                await applyComputerControlDefaultForNewSession()
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

    private func checkConnectionNow() async {
        guard !connectionActionInFlight, !connectionCheckInFlight else { return }
        connectionCheckInFlight = true
        connectionCheckMessage = nil
        connectionCheckSucceeded = nil
        defer { connectionCheckInFlight = false }

        do {
            _ = try await requestSnapshot(method: "getStatus", params: MascotStatusParams(
                includeMascotJobs: UserDefaults.standard.object(forKey: "ferret.visible") as? Bool != false
            ))
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

#if DEBUG
    /// Visual review only: present a fixed backend state without a helper.
    func presentForReview(snapshot: BackendSnapshot, activities: [ActivityEntry]) {
        self.snapshot = snapshot
        self.activities = activities
    }
#endif

    @discardableResult
    private func applySnapshot(_ candidate: BackendSnapshot, requestSequence: UInt64? = nil) -> Bool {
        if let requestSequence {
            guard snapshotRequestGate.shouldApply(
                requestSequence,
                candidateRevision: candidate.stateRevision,
                currentRevision: snapshot.stateRevision
            ) else { return false }
        } else if candidate.stateRevision < snapshot.stateRevision {
            return false
        }
        let previousPollInterval = pollInterval
        snapshot = candidate
        snapshotFreshnessGate.markApplied(at: ProcessInfo.processInfo.systemUptime)
        if candidate.tunnelReady && !candidate.chatGPTConnected {
            if awaitingChatGPTSinceUptime == nil {
                awaitingChatGPTSinceUptime = ProcessInfo.processInfo.systemUptime
            }
        } else {
            awaitingChatGPTSinceUptime = nil
        }
        if pollInterval < previousPollInterval {
            pollWaker.signal()
        }
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

private struct MascotStatusParams: Encodable {
    let includeMascotJobs: Bool
}
