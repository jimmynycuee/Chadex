import AppKit
import SwiftUI

private enum SidebarSelection: Hashable {
    case project(UUID)
    case activity
    case guide
}

struct RootView: View {
    @Environment(\.openSettings) private var openSettings
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    @AppStorage("root.didRouteFirstLaunch") private var didRouteFirstLaunch = false
    @AppStorage("root.lastSidebarDestination") private var lastSidebarDestination = "project"
    @State private var selection: SidebarSelection?
    @State private var projectPendingRemoval: ProjectRecord?
    @State private var projectSwitchRequestInFlight = false
    @State private var pendingProjectSwitchID: UUID?

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

                            Divider()

                            Button(L10n.string("project.remove"), role: .destructive) {
                                projectPendingRemoval = project
                            }
                        }
                    }

                    Section {
                        HStack(spacing: 8) {
                            Label(L10n.string("sidebar.activity"), systemImage: "clock.arrow.circlepath")
                                .chadexFont(.body)
                            Spacer(minLength: 8)
                            if !model.activities.isEmpty {
                                Text("\(model.activities.count)")
                                    .chadexFont(.caption, design: .monospaced)
                                    .foregroundStyle(.tertiary)
                            }
                        }
                        .tag(SidebarSelection.activity)

                        Label(L10n.string("sidebar.guide"), systemImage: "questionmark.circle")
                            .chadexFont(.body)
                            .tag(SidebarSelection.guide)
                    }
                }
                .listStyle(.sidebar)
                .scrollContentBackground(.hidden)

                Divider()

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
                    .padding(.horizontal, layout.spacing(12))
                    .padding(.vertical, layout.spacing(8))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .contentShape(Rectangle())
                .accessibilityLabel(L10n.string("menubar.settings"))
            }
            .navigationSplitViewColumnWidth(
                min: layout.sidebar(ChadexMetrics.sidebarMinWidth),
                ideal: layout.sidebar(ChadexMetrics.sidebarIdealWidth),
                max: layout.sidebar(ChadexMetrics.sidebarMaxWidth)
            )
        } detail: {
            detail
        }
        .navigationSplitViewStyle(.balanced)
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                if model.isSwitchingProject {
                    ProgressView()
                        .controlSize(.small)
                        .help(model.switchingProjectName.map { L10n.string("project.switching", $0) } ?? L10n.string("status.preparing"))
                } else if selection != .guide {
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

                        Divider()

                        Button(L10n.string("project.remove"), role: .destructive) {
                            projectPendingRemoval = project
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
        .sheet(isPresented: $model.showingConnectionSettings) {
            ConnectionSettingsSheet()
                .environmentObject(model)
        }
        .alert(item: rootPresentedError) { error in
            Alert(
                title: Text(error.title),
                message: Text("\(error.message)\n\n\(error.recovery)"),
                dismissButton: .default(Text(L10n.string("common.ok")))
            )
        }
        .alert(item: $projectPendingRemoval) { project in
            Alert(
                title: Text(L10n.string("project.removeTitle")),
                message: Text(L10n.string("project.removeMessage", project.name)),
                primaryButton: .destructive(Text(L10n.string("project.remove"))) {
                    Task {
                        if await model.removeProject(project.id) {
                            selection = model.selectedProject.map { .project($0.id) } ?? .activity
                        }
                    }
                },
                secondaryButton: .cancel(Text(L10n.string("common.cancel")))
            )
        }
    }

    private var selectedSidebarProject: ProjectRecord? {
        guard case .project(let id) = selection else { return nil }
        return model.projects.first(where: { $0.id == id })
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
        case .project:
            if let project = model.selectedProject {
                ProjectDetailView(project: project) {
                    selection = .activity
                }
            } else {
                EmptyProjectView()
            }
        case .none:
            EmptyProjectView()
        }
    }
}

private struct ProjectSidebarRow: View {
    let project: ProjectRecord
    let isCurrent: Bool
    let phase: ConnectionPhase

    var body: some View {
        HStack(spacing: 9) {
            Image(systemName: "folder")
                .chadexFont(.callout, weight: .medium)
                .foregroundStyle(.secondary)
                .frame(width: 16)

            Text(project.name)
                .chadexFont(.body)
                .lineLimit(1)

            Spacer(minLength: 8)

            if isCurrent {
                Circle()
                    .fill(phase.chadexTint)
                    .frame(width: 5.5, height: 5.5)
                    .accessibilityHidden(true)
            }
        }
        .chadexPadding(.vertical, 2)
        .contentShape(Rectangle())
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
                .controlSize(.regular)
        }
        .frame(maxWidth: 420)
        .chadexPadding(32)
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
