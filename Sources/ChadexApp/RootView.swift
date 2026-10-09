import AppKit
import SwiftUI

enum SidebarSelection: Hashable {
    case project(UUID)
    case agentSettings
    case skills
    case computerUse
    case activity
    case guide
}

struct RootView: View {
    @Environment(\.openSettings) private var openSettings
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var updateManager: UpdateManager
    @AppStorage("root.didRouteFirstLaunch") private var didRouteFirstLaunch = false
    @AppStorage("root.lastSidebarDestination") private var lastSidebarDestination = "project"
    @State private var selection: SidebarSelection?
    @State private var projectSwitchRequestInFlight = false
    @State private var pendingProjectSwitchID: UUID?
    @State private var memoryInspectorProject: ProjectRecord?

    var body: some View {
        NavigationSplitView {
            VStack(spacing: 0) {
                HStack(spacing: 6) {
                    Text(L10n.string("sidebar.projects"))
                        .chadexFont(.caption, weight: .semibold)
                        .foregroundStyle(.secondary)

                    Spacer(minLength: 8)

                    Button {
                        model.addProjectFromPanel()
                    } label: {
                        Image(systemName: "plus")
                            .chadexFont(.caption, weight: .semibold)
                            .frame(width: 18, height: 18)
                    }
                    .buttonStyle(.borderless)
                    .contentShape(Rectangle())
                    .help(L10n.string("project.addHint"))
                    .accessibilityLabel(L10n.string("project.add"))
                }
                .chadexPadding(.horizontal, 12)
                .chadexPadding(.top, 8)
                .chadexPadding(.bottom, 3)

                List(selection: $selection) {
                    ForEach(model.projects) { project in
                        ProjectSidebarRow(
                            project: project,
                            isCurrent: model.selectedProject?.id == project.id,
                            phase: model.connectionPresentationPhase
                        )
                        .tag(SidebarSelection.project(project.id))
                        .help(project.path)
                        .contextMenu {
                            Button(L10n.string("project.openInFinder")) {
                                NSWorkspace.shared.activateFileViewerSelecting([
                                    URL(fileURLWithPath: project.path)
                                ])
                            }

                            Button(L10n.string("project.copyPath")) {
                                NSPasteboard.general.clearContents()
                                NSPasteboard.general.setString(project.path, forType: .string)
                            }

                            Button(L10n.string("project.memoryInspector")) {
                                Task { await showMemoryInspector(for: project) }
                            }

                            Divider()

                            Button(L10n.string("project.remove"), role: .destructive) {
                                confirmProjectRemoval(project)
                            }
                        }
                    }

                    Section {
                        SidebarNavigationRow(
                            title: L10n.string("sidebar.agentSettings"),
                            systemImage: "person.crop.circle"
                        )
                        .tag(SidebarSelection.agentSettings)

                        SidebarNavigationRow(
                            title: L10n.string("sidebar.skills"),
                            systemImage: "puzzlepiece.extension"
                        )
                        .tag(SidebarSelection.skills)

                        SidebarNavigationRow(
                            title: L10n.string("sidebar.computerUse"),
                            systemImage: "desktopcomputer"
                        )
                        .tag(SidebarSelection.computerUse)
                    }

                    Section {
                        HStack(spacing: 8) {
                            SidebarNavigationRow(
                                title: L10n.string("sidebar.activity"),
                                systemImage: "clock.arrow.circlepath"
                            )
                            Spacer(minLength: 8)
                            if !model.activities.isEmpty {
                                Text("\(model.activities.count)")
                                    .chadexFont(.caption, design: .monospaced)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        .tag(SidebarSelection.activity)

                        SidebarNavigationRow(
                            title: L10n.string("sidebar.guide"),
                            systemImage: "questionmark.circle"
                        )
                        .tag(SidebarSelection.guide)
                    }
                }
                .listStyle(.sidebar)
                .scrollContentBackground(.hidden)

                CodeFerretCompanion()

                Divider()

                HStack(spacing: layout.spacing(8)) {
                    Button {
                        openSettings()
                    } label: {
                        HStack(spacing: layout.spacing(8)) {
                            Image(systemName: "gearshape")
                                .frame(width: layout.control(16))
                            Text(L10n.string("menubar.settings"))
                                .chadexFont(.body)
                            Spacer(minLength: 0)
                        }
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.vertical, layout.spacing(6))
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .contentShape(Rectangle())
                    .accessibilityLabel(L10n.string("menubar.settings"))

                    if let release = updateManager.availableRelease {
                        Button {
                            Task {
                                await updateManager.installAvailableUpdate {
                                    !model.hasUpdateBlockingWork
                                }
                            }
                        } label: {
                            HStack(spacing: 5) {
                                Image(systemName: "arrow.down.circle.fill")
                                Text(L10n.string("updates.quickUpdate"))
                                    .chadexFont(.callout, weight: .semibold)
                            }
                        }
                        .buttonStyle(.borderedProminent)
                        .tint(.blue)
                        .chadexControlSize(.small)
                        .disabled(updateManager.isBusy)
                        .help(L10n.string("updates.quickUpdateHint", release.version))
                        .accessibilityLabel(L10n.string("updates.quickUpdateHint", release.version))
                    }
                }
                .padding(.horizontal, layout.spacing(12))
                .padding(.vertical, layout.spacing(4))
            }
            // Keep the native collapsible sidebar, but give the split item no
            // horizontal resize range. This also keeps the titlebar tracking
            // separator and the content divider on one stable boundary.
            .background {
                // The floating Liquid Glass sidebar (macOS 26+) has no seam to
                // align, and repositioning its split view fights the system.
                if #unavailable(macOS 26.0) {
                    SidebarSplitViewAlignmentBridge(width: fixedSidebarWidth)
                        .frame(width: 0, height: 0)
                }
            }
            .navigationSplitViewColumnWidth(
                min: fixedSidebarWidth,
                ideal: fixedSidebarWidth,
                max: fixedSidebarWidth
            )
        } detail: {
            // The connection's color is the window's, not one page's: it stays
            // put while moving between pages and under the sidebar glass.
            detail
                .background {
                    ConnectionAmbience(phase: model.connectionPresentationPhase)
                }
        }
        .navigationSplitViewStyle(.balanced)
        .focusedSceneValue(\.sidebarSelection, $selection)
        .onReceive(NotificationCenter.default.publisher(for: .chadexShowGuide)) { _ in
            selection = .guide
        }
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                if model.isSwitchingProject {
                    ProgressView()
                        .chadexControlSize(.small)
                        .help(model.switchingProjectName.map { L10n.string("project.switching", $0) } ?? L10n.string("status.preparing"))
                } else if selection != .guide && selection != .skills {
                    Button {
                        Task { await model.refreshStatus(force: true) }
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .help(L10n.string("settings.refresh"))
                    .accessibilityLabel(L10n.string("settings.refresh"))
                }

                if let project = selectedSidebarProject {
                    Menu {
                        Button(L10n.string("project.openInFinder")) {
                            NSWorkspace.shared.activateFileViewerSelecting([
                                URL(fileURLWithPath: project.path)
                            ])
                        }

                        Button(L10n.string("project.copyPath")) {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(project.path, forType: .string)
                        }

                        Button(L10n.string("project.memoryInspector")) {
                            Task { await showMemoryInspector(for: project) }
                        }

                        Divider()

                        Button(L10n.string("project.remove"), role: .destructive) {
                            confirmProjectRemoval(project)
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .help(L10n.string("project.actions"))
                    .accessibilityLabel(L10n.string("project.actions"))
                }
            }
        }
        .onAppear {
            if selection == nil {
                if !didRouteFirstLaunch && isFreshInstall {
                    selection = .guide
                    lastSidebarDestination = "guide"
                } else {
                    selection = restoredSelection
                }
                didRouteFirstLaunch = true
            }

        }
        .onChange(of: selection) { _, value in
            persistSidebarDestination(value)

            if case .project(let id) = value {
                pendingProjectSwitchID = id
                guard !projectSwitchRequestInFlight else { return }
                projectSwitchRequestInFlight = true
                Task { @MainActor in
                    defer { projectSwitchRequestInFlight = false }
                    while let requestedID = pendingProjectSwitchID {
                        pendingProjectSwitchID = nil
                        if requestedID != model.selectedProject?.id {
                            _ = await model.selectProject(requestedID)
                        }
                    }
                    if case .project = selection {
                        selection = model.selectedProject.map { .project($0.id) } ?? .activity
                    }
                }
            } else if projectSwitchRequestInFlight {
                pendingProjectSwitchID = nil
            }
        }
        .onChange(of: model.selectedProject?.id) { _, activeID in
            guard case .project(let rowID) = selection,
                  let target = SidebarProjectFollow.projectToSelect(
                      rowProjectID: rowID,
                      activeProjectID: activeID,
                      sidebarSwitchInFlight: projectSwitchRequestInFlight
                  ) else { return }
            selection = .project(target)
        }
        .sheet(isPresented: $model.showingConnectionSettings) {
            ConnectionSettingsSheet()
                .environmentObject(model)
        }
        .sheet(item: $memoryInspectorProject) { project in
            ProjectMemoryInspectorSheet(project: project)
                .environmentObject(model)
        }
        .alert(item: rootPresentedError) { error in
            Alert(
                title: Text(error.title),
                message: Text("\(error.message)\n\n\(error.recovery)"),
                dismissButton: .default(Text(L10n.string("common.ok")))
            )
        }
    }

    private var fixedSidebarWidth: CGFloat {
        layout.sidebar(ChadexMetrics.sidebarFixedWidth)
    }

    private var selectedSidebarProject: ProjectRecord? {
        switch selection {
        case .project(let id):
            return model.projects.first(where: { $0.id == id })
        case .agentSettings, .skills, .computerUse:
            return model.selectedProject
        case .activity, .guide, .none:
            return nil
        }
    }

    /// Confirms with an app-modal NSAlert: a second SwiftUI `.alert(item:)` on
    /// this view never appeared next to the error alert, so Remove did nothing.
    private func confirmProjectRemoval(_ project: ProjectRecord) {
        let alert = NSAlert()
        alert.alertStyle = .warning
        alert.messageText = L10n.string("project.removeTitle")
        alert.informativeText = L10n.string("project.removeMessage", project.name)
        let cancel = alert.addButton(withTitle: L10n.string("common.cancel"))
        cancel.keyEquivalent = "\r"
        let remove = alert.addButton(withTitle: L10n.string("project.removeConfirm"))
        remove.keyEquivalent = ""
        remove.hasDestructiveAction = true
        guard alert.runModal() == .alertSecondButtonReturn else { return }
        Task {
            guard await model.removeProject(project.id) else { return }
            switch selection {
            case .agentSettings, .skills, .computerUse:
                if model.selectedProject == nil {
                    selection = .activity
                }
            default:
                selection = model.selectedProject.map { .project($0.id) } ?? .activity
            }
        }
    }

    private var rootPresentedError: Binding<AppModel.PresentedError?> {
        Binding(
            get: {
                guard !model.showingConnectionSettings else { return nil }
                return model.presentedError
            },
            set: { model.presentedError = $0 }
        )
    }

    private var isFreshInstall: Bool {
        model.projects.isEmpty
            && model.preferences.tunnelID.isEmpty
            && !model.hasStoredAPIKey
    }

    private var restoredSelection: SidebarSelection {
        switch lastSidebarDestination {
        case "guide":
            return .guide
        case "activity":
            return .activity
        case "agent", "project.instructions":
            return .agentSettings
        case "skills", "project.skills":
            return .skills
        case "computer", "project.computer":
            return .computerUse
        default:
            if let project = model.selectedProject {
                return .project(project.id)
            }
            return .activity
        }
    }

    private func persistSidebarDestination(_ selection: SidebarSelection?) {
        switch selection {
        case .guide:
            lastSidebarDestination = "guide"
        case .activity:
            lastSidebarDestination = "activity"
        case .agentSettings:
            lastSidebarDestination = "agent"
        case .skills:
            lastSidebarDestination = "skills"
        case .computerUse:
            lastSidebarDestination = "computer"
        case .project:
            lastSidebarDestination = "project"
        case .none:
            break
        }
    }

    @ViewBuilder
    private var detail: some View {
        switch selection {
        case .activity:
            ActivityView()
        case .guide:
            GuideView()
        case .agentSettings:
            if let project = model.selectedProject {
                ProjectDetailView(
                    project: project,
                    destination: .agentSettings,
                    onShowAllActivity: { selection = .activity },
                    onManageSkills: { selection = .skills }
                )
            } else {
                GlobalInstructionsStandaloneView()
            }
        case .skills:
            selectedProjectDetail(destination: .skills)
        case .computerUse:
            selectedProjectDetail(destination: .computerUse)
        case .project(let id):
            if let project = model.projects.first(where: { $0.id == id }) {
                if model.selectedProject?.id == id {
                    ProjectDetailView(
                        project: project,
                        destination: .overview
                    ) {
                        selection = .activity
                    }
                } else {
                    projectSwitchingPlaceholder(project)
                }
            } else {
                EmptyProjectView()
            }
        case .none:
            EmptyProjectView()
        }
    }

    @ViewBuilder
    private func selectedProjectDetail(destination: ProjectWorkspaceDestination) -> some View {
        if let project = model.selectedProject {
            ProjectDetailView(project: project, destination: destination) {
                selection = .activity
            }
        } else {
            EmptyProjectView()
        }
    }

    @MainActor
    private func showMemoryInspector(for project: ProjectRecord) async {
        if model.selectedProject?.id != project.id {
            guard await model.selectProject(project.id) else { return }
        }
        guard model.selectedProject?.id == project.id else { return }
        await model.refreshProjectMemory()
        memoryInspectorProject = project
    }

    private func projectSwitchingPlaceholder(_ project: ProjectRecord) -> some View {
        VStack(spacing: layout.spacing(12)) {
            ProgressView()
                .chadexControlSize(.small)
            Text(L10n.string("project.switching", project.name))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .navigationTitle(project.name)
    }
}

private struct SidebarNavigationRow: View {
    @Environment(\.chadexLayout) private var layout
    let title: String
    let systemImage: String

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: systemImage)
                .chadexFont(.body, weight: .medium)
                .foregroundStyle(.secondary)
                .frame(width: layout.control(16))
                .accessibilityHidden(true)

            Text(title)
                .chadexFont(.body)
                .lineLimit(1)
        }
        .chadexPadding(.vertical, 1)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
    }
}

private struct ProjectMemoryInspectorSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel

    let project: ProjectRecord

    var body: some View {
        NavigationStack {
            ScrollView {
                ProjectMemoryView(project: project)
                    .frame(maxWidth: layout.control(760), alignment: .topLeading)
                    .chadexPadding(.horizontal, 20)
                    .chadexPadding(.vertical, 18)
                    .frame(maxWidth: .infinity, alignment: .topLeading)
            }
            .navigationTitle("\(project.name) — \(L10n.string("memory.title"))")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button(L10n.string("common.done")) { dismiss() }
                }
            }
        }
        .frame(
            minWidth: layout.control(720),
            minHeight: layout.control(560)
        )
        .task(id: project.id) {
            guard model.selectedProject?.id == project.id else { return }
            await model.refreshProjectMemory()
        }
    }
}

private struct SidebarSplitViewAlignmentBridge: NSViewRepresentable {
    let width: CGFloat

    func makeNSView(context: Context) -> SidebarSplitViewAlignmentView {
        let view = SidebarSplitViewAlignmentView()
        view.sidebarWidth = width
        return view
    }

    func updateNSView(_ nsView: SidebarSplitViewAlignmentView, context: Context) {
        nsView.sidebarWidth = width
        nsView.scheduleAlignment()
    }
}

private final class SidebarSplitViewAlignmentView: NSView {
    var sidebarWidth: CGFloat = ChadexMetrics.sidebarFixedWidth
    private var windowUpdateObserver: NSObjectProtocol?

    deinit {
        removeWindowObserver()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        installWindowObserver()
        scheduleAlignment()
    }

    override func viewDidMoveToSuperview() {
        super.viewDidMoveToSuperview()
        scheduleAlignment()
    }

    func scheduleAlignment() {
        DispatchQueue.main.async { [weak self] in
            self?.alignSplitViewAndTrackingSeparator()
        }
    }

    private func installWindowObserver() {
        removeWindowObserver()
        guard let window else { return }

        // NavigationSplitView already extends its sidebar/detail materials through
        // the titlebar. Keeping AppKit's extra titlebar background opaque creates
        // a second boundary at a different edge of the 4-pt vibrant divider
        // (3 pt / 6 Retina pixels away from the body boundary). Let the shared
        // split-view material draw through the titlebar so both regions use the
        // exact same divider geometry. From macOS 26 the floating sidebar has
        // no such seam, and a transparent titlebar would hide the toolbar's
        // scroll edge effect, letting scrolled content run under the title.
        if #unavailable(macOS 26.0) {
            window.titlebarAppearsTransparent = true
        }

        windowUpdateObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didUpdateNotification,
            object: window,
            queue: .main
        ) { [weak self] _ in
            self?.alignSplitViewAndTrackingSeparator()
        }
    }

    private func removeWindowObserver() {
        if let windowUpdateObserver {
            NotificationCenter.default.removeObserver(windowUpdateObserver)
            self.windowUpdateObserver = nil
        }
    }

    private func alignSplitViewAndTrackingSeparator() {
        guard let splitView = enclosingSplitView(),
              splitView.subviews.count >= 2
        else {
            return
        }

        guard let sidebar = splitView.subviews.min(by: { $0.frame.minX < $1.frame.minX }) else {
            return
        }
        if !sidebar.isHidden,
           sidebar.frame.width > 1,
           abs(sidebar.frame.width - sidebarWidth) > 0.5 {
            splitView.setPosition(sidebarWidth, ofDividerAt: 0)
        }

        guard let toolbarItems = window?.toolbar?.items else {
            return
        }
        for case let trackingItem as NSTrackingSeparatorToolbarItem in toolbarItems {
            if trackingItem.splitView !== splitView || trackingItem.dividerIndex != 0 {
                trackingItem.splitView = splitView
                trackingItem.dividerIndex = 0
            }
        }
    }

    private func enclosingSplitView() -> NSSplitView? {
        var candidate = superview
        while let view = candidate {
            if let splitView = view as? NSSplitView {
                return splitView
            }
            candidate = view.superview
        }
        return nil
    }
}

private struct ProjectSidebarRow: View {
    @Environment(\.chadexLayout) private var layout
    let project: ProjectRecord
    let isCurrent: Bool
    let phase: ConnectionPhase

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "folder")
                .chadexFont(.body, weight: .medium)
                .foregroundStyle(.secondary)
                .frame(width: layout.control(16))
                .accessibilityHidden(true)

            Text(project.name)
                .chadexFont(.body)
                .lineLimit(1)

            Spacer(minLength: 8)

            if isCurrent {
                Circle()
                    .fill(phase.chadexTint)
                    .frame(width: layout.control(6), height: layout.control(6))
                    .accessibilityHidden(true)
            }
        }
        .chadexPadding(.vertical, 1)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityValue(isCurrent ? StatusPresentation.text(for: phase) : "")
    }
}

private struct EmptyProjectView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(spacing: 14) {
            Image(systemName: "folder")
                .chadexFont(.largeTitle, weight: .light)
                .foregroundStyle(.tertiary)

            VStack(spacing: 5) {
                Text(L10n.string("project.emptyTitle"))
                    .chadexFont(.title3, weight: .semibold)
                Text(L10n.string("project.emptyMessage"))
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
            }

            Button(L10n.string("project.add")) { model.addProjectFromPanel() }
                .buttonStyle(.borderedProminent)
                .chadexControlSize(.regular)
        }
        .frame(maxWidth: 420)
        .chadexPadding(32)
    }
}

extension Notification.Name {
    static let chadexShowGuide = Notification.Name("app.chadex.showGuide")
}

enum MainWindowPresenter {
    /// `openWindow(id:)` on a WindowGroup always creates another window, which
    /// would duplicate the main window and any sheet bound to the shared
    /// model. Bring an existing main window forward and open one only when
    /// none is left.
    @MainActor
    static func show(using openWindow: OpenWindowAction) {
        // A hidden app (⌘H) reports its windows as not visible until unhidden.
        let wasHidden = NSApp.isHidden
        if wasHidden { NSApp.unhide(nil) }
        NSApp.activate(ignoringOtherApps: true)
        // SwiftUI names WindowGroup(id: "main") windows "main-AppWindow-N".
        // If that ever changes, this falls back to opening a window.
        let existing = NSApp.windows.first { window in
            window.identifier?.rawValue.hasPrefix("main-AppWindow-") == true
                && (wasHidden || window.isVisible || window.isMiniaturized)
        }
        if let window = existing {
            if window.isMiniaturized { window.deminiaturize(nil) }
            window.makeKeyAndOrderFront(nil)
        } else {
            openWindow(id: "main")
        }
    }
}

struct SidebarSelectionFocusedKey: FocusedValueKey {
    typealias Value = Binding<SidebarSelection?>
}

extension FocusedValues {
    var sidebarSelection: Binding<SidebarSelection?>? {
        get { self[SidebarSelectionFocusedKey.self] }
        set { self[SidebarSelectionFocusedKey.self] = newValue }
    }
}

/// Keyboard routes to every sidebar destination (View menu) and the guide
/// (Help menu), so the whole window can be driven without the pointer.
struct ChadexNavigationCommands: Commands {
    @ObservedObject var model: AppModel
    @FocusedBinding(\.sidebarSelection) private var selection: SidebarSelection??
    @Environment(\.openWindow) private var openWindow

    /// Destinations are unavailable mid-switch: a stale selection would
    /// queue a switch back to the previous project.
    private var canNavigate: Bool {
        selection != nil && !model.isSwitchingProject
    }

    var body: some Commands {
        CommandGroup(after: .sidebar) {
            Divider()
            Button(L10n.string("navigation.overview")) {
                if let project = model.selectedProject {
                    selection = .project(project.id)
                }
            }
            .keyboardShortcut("1", modifiers: .command)
            .disabled(!canNavigate || model.selectedProject == nil)

            Button(L10n.string("sidebar.agentSettings")) { selection = .agentSettings }
                .keyboardShortcut("2", modifiers: .command)
                .disabled(!canNavigate)

            Button(L10n.string("sidebar.skills")) { selection = .skills }
                .keyboardShortcut("3", modifiers: .command)
                .disabled(!canNavigate || model.selectedProject == nil)

            Button(L10n.string("sidebar.computerUse")) { selection = .computerUse }
                .keyboardShortcut("4", modifiers: .command)
                .disabled(!canNavigate || model.selectedProject == nil)

            Button(L10n.string("sidebar.activity")) { selection = .activity }
                .keyboardShortcut("5", modifiers: .command)
                .disabled(!canNavigate)
            Divider()
        }

        CommandGroup(replacing: .help) {
            Button(L10n.string("navigation.guideHelp")) {
                if selection != nil {
                    selection = .guide
                } else {
                    // No focused main window (e.g. Settings is in front): route
                    // an existing window to the guide, or open one onto it.
                    UserDefaults.standard.set("guide", forKey: "root.lastSidebarDestination")
                    NotificationCenter.default.post(name: .chadexShowGuide, object: nil)
                    MainWindowPresenter.show(using: openWindow)
                }
            }
            .keyboardShortcut("?", modifiers: .command)
        }
    }
}

enum StatusPresentation {
    static func text(for phase: ConnectionPhase) -> String {
        switch phase {
        case .unconfigured: return L10n.string("status.unconfigured")
        case .preparing: return L10n.string("status.preparing")
        case .waitingForChatGPTVerification: return L10n.string("status.waiting")
        case .verified: return L10n.string("status.verified")
        case .stopped: return L10n.string("status.stopped")
        case .error: return L10n.string("status.error")
        }
    }
}
