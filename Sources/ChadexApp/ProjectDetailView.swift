import AppKit
import SwiftUI

struct ProjectDetailView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord
    let destination: ProjectWorkspaceDestination
    let onShowAllActivity: () -> Void
    @State private var showingErrorDetails = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: layout.spacing(ChadexMetrics.sectionSpacing)) {
                workspaceContent
            }
            .frame(maxWidth: layout.control(ChadexMetrics.detailMaxWidth), alignment: .leading)
            .chadexPadding(.horizontal, ChadexMetrics.detailHorizontalPadding)
            .chadexPadding(.vertical, ChadexMetrics.detailVerticalPadding)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .navigationTitle(navigationTitle)
        .task(id: "\(project.id.uuidString):\(destination.rawValue)") {
            guard model.selectedProject?.id == project.id else { return }

            switch destination {
            case .agentSettings:
                await model.refreshProjectInstructions()
                await model.refreshSkills()
            case .computerUse:
                await model.refreshComputerSafety()
            case .overview:
                break
            }
        }
    }

    @ViewBuilder
    private var workspaceContent: some View {
        switch destination {
        case .overview:
            header
            Divider()
            connectionSection

            if let error = model.connectionError {
                errorSection(error)
            }

            if let task = model.snapshot.taskProgress {
                Divider()
                TaskProgressView(task: task) {
                    Task { await model.cancelTask(task) }
                }
            }

            Divider()
            recentActivity

        case .agentSettings:
            agentSettingsSection

        case .computerUse:
            computerControlSection
        }
    }

    private var navigationTitle: String {
        switch destination {
        case .overview:
            return project.name
        case .agentSettings:
            return L10n.string("sidebar.agentSettings")
        case .computerUse:
            return L10n.string("sidebar.computerUse")
        }
    }

    private var agentSettingsSection: some View {
        VStack(alignment: .leading, spacing: layout.spacing(20)) {
            VStack(alignment: .leading, spacing: 7) {
                Text(L10n.string("agentSettings.subtitle"))
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                HStack(spacing: 7) {
                    Image(systemName: "folder")
                        .chadexFont(.caption, weight: .medium)
                        .foregroundStyle(.secondary)
                    Text(L10n.string("agentSettings.currentProject", project.name))
                        .chadexFont(.caption, weight: .medium)
                    Text(project.path)
                        .chadexFont(.caption, design: .monospaced)
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                        .textSelection(.enabled)
                }
            }

            Divider()
            GlobalInstructionsEditor()
            Divider()
            SkillsCenterView(project: project)
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: "folder")
                .chadexFont(.callout, weight: .semibold)
                .foregroundStyle(Color.accentColor)
                .frame(width: layout.control(22), height: layout.control(24), alignment: .top)

            VStack(alignment: .leading, spacing: 5) {
                Text(project.name)
                    .chadexFont(.title2, weight: .semibold)
                    .tracking(-0.2)

                HStack(alignment: .firstTextBaseline, spacing: 7) {
                    Text(project.path)
                        .chadexFont(.callout, design: .monospaced)
                        .foregroundStyle(.tertiary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)

                    Button {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(project.path, forType: .string)
                    } label: {
                        Image(systemName: "doc.on.doc")
                            .chadexFont(.caption2, weight: .medium)
                    }
                    .buttonStyle(.borderless)
                    .help(L10n.string("project.copyPath"))
                    .accessibilityLabel(L10n.string("project.copyPath"))
                }
            }

            Spacer(minLength: 20)
        }
    }

    private var connectionSection: some View {
        VStack(alignment: .leading, spacing: 14) {
            SectionEyebrow(title: L10n.string("connection.title"))

            ViewThatFits(in: .horizontal) {
                HStack(alignment: .top, spacing: layout.spacing(24)) {
                    connectionStatusCopy
                    Spacer(minLength: 20)
                    connectionActions
                }

                VStack(alignment: .leading, spacing: layout.spacing(16)) {
                    connectionStatusCopy
                    connectionActions
                }
            }

            ConnectionPathView(
                phase: model.connectionPresentationPhase,
                tunnelReady: model.isSwitchingProject ? false : model.snapshot.tunnelReady,
                chatGPTConnected: model.isSwitchingProject ? false : model.snapshot.chatGPTConnected,
                chatGPTVerified: model.isSwitchingProject ? false : model.snapshot.chatGPTVerifiedForSelectedProject,
                isConnecting: model.connectionAction == .connecting
            )
            .chadexPadding(.vertical, 1)

            Divider()

            LazyVGrid(
                columns: [
                    GridItem(.flexible(minimum: layout.control(220)), spacing: layout.spacing(32), alignment: .leading),
                    GridItem(.flexible(minimum: layout.control(180)), spacing: layout.spacing(32), alignment: .leading)
                ],
                alignment: .leading,
                spacing: layout.spacing(12)
            ) {
                TechnicalMetadataItem(
                    title: L10n.string("settings.tunnelID"),
                    value: tunnelIDDisplayValue,
                    monospaced: true,
                    copyValue: model.preferences.tunnelID.isEmpty ? nil : model.preferences.tunnelID
                )
                if model.snapshot.lastVerifiedAtMs != nil {
                    TechnicalMetadataItem(
                        title: L10n.string("connection.lastVerifiedLabel"),
                        value: lastVerifiedValue,
                        monospaced: true
                    )
                }
            }
        }
    }


    private var computerControlSection: some View {
        VStack(alignment: .leading, spacing: layout.spacing(22)) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                SectionEyebrow(title: L10n.string("computer.title"))
                Spacer(minLength: 12)
                Label(
                    computerControlStatusText,
                    systemImage: model.computerSafety.stopped
                        ? "hand.raised.fill"
                        : (model.computerSafety.pendingApprovals.isEmpty ? "desktopcomputer" : "exclamationmark.circle.fill")
                )
                .chadexFont(.caption, weight: .medium)
                .foregroundStyle(
                    model.computerSafety.stopped || !model.computerSafety.pendingApprovals.isEmpty
                        ? Color.orange
                        : Color.secondary
                )
            }

            if model.computerSafety.stopped {
                VStack(alignment: .leading, spacing: layout.spacing(12)) {
                    Text(L10n.string("computer.description.stopped"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)

                    Button {
                        Task { await model.resumeComputerControl() }
                    } label: {
                        Label(L10n.string("computer.resume"), systemImage: "play.fill")
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.regular)
                    .disabled(model.computerSafetyMutationInFlight)
                    .help(L10n.string("computer.resumeHelp"))
                }
            } else {
                VStack(alignment: .leading, spacing: layout.spacing(10)) {
                    Text(L10n.string("computer.controlModeTitle"))
                        .chadexFont(.callout, weight: .semibold)
                    Text(L10n.string("computer.controlModeHelp"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    computerControlModePicker
                        .chadexPadding(.top, 2)
                    Text(computerControlModeDescription)
                        .chadexFont(.caption)
                        .foregroundStyle(.tertiary)
                        .fixedSize(horizontal: false, vertical: true)
                    if !model.snapshot.tunnelReady {
                        Text(L10n.string("computer.sessionUnavailable"))
                            .chadexFont(.caption)
                            .foregroundStyle(.tertiary)
                    }
                }

                if let approval = model.computerSafety.pendingApprovals.first {
                    Divider()
                    VStack(alignment: .leading, spacing: layout.spacing(10)) {
                        Text(L10n.string("computer.requestTitle"))
                            .chadexFont(.caption, weight: .semibold)
                            .foregroundStyle(.secondary)

                        HStack(alignment: .center, spacing: 12) {
                            VStack(alignment: .leading, spacing: 3) {
                                Text(L10n.string("computer.approvalTitle"))
                                    .chadexFont(.callout, weight: .semibold)
                                Text(L10n.string("computer.approvalAction", computerActionLabel(approval.action)))
                                    .chadexFont(.caption)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer(minLength: 12)
                            ViewThatFits(in: .horizontal) {
                                HStack(spacing: 8) {
                                    computerApprovalButtons(approval)
                                }
                                VStack(alignment: .trailing, spacing: 8) {
                                    computerApprovalButtons(approval)
                                }
                            }
                        }
                        .accessibilityElement(children: .contain)

                        if model.computerSafety.pendingApprovals.count > 1 {
                            Text(L10n.string(
                                "computer.moreApprovals",
                                model.computerSafety.pendingApprovals.count - 1
                            ))
                            .chadexFont(.caption)
                            .foregroundStyle(.tertiary)
                        }
                    }
                }

                Divider()
                VStack(alignment: .leading, spacing: layout.spacing(10)) {
                    Text(L10n.string("computer.safetyTitle"))
                        .chadexFont(.callout, weight: .semibold)
                    Text(L10n.string("computer.safetyNote"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)

                    Button(role: .destructive) {
                        Task { await model.stopComputerControl() }
                    } label: {
                        Label(L10n.string("computer.stop"), systemImage: "stop.circle.fill")
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.regular)
                    .disabled(model.computerSafetyMutationInFlight)
                    .help(L10n.string("computer.stopHelp"))
                }
            }

            if let error = model.computerSafetyError {
                Label(error, systemImage: "exclamationmark.triangle")
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var computerControlModePicker: some View {
        Picker(
            L10n.string("computer.controlModeTitle"),
            selection: Binding(
                get: { model.computerSafety.mode },
                set: { mode in
                    Task {
                        if mode == .allowSession {
                            await model.setComputerControlMode(mode)
                        } else {
                            await model.setComputerControlDefaultMode(mode)
                        }
                    }
                }
            )
        ) {
            Text(L10n.string("computer.mode.ask")).tag(ComputerControlMode.askBeforeControl)
            if model.snapshot.tunnelReady {
                Text(L10n.string("computer.mode.allowSession")).tag(ComputerControlMode.allowSession)
            }
            Text(L10n.string("computer.mode.alwaysAllow")).tag(ComputerControlMode.alwaysAllow)
            Text(L10n.string("computer.mode.readOnly")).tag(ComputerControlMode.readOnly)
        }
        .pickerStyle(.radioGroup)
        .labelsHidden()
        .disabled(model.computerSafetyMutationInFlight)
        .accessibilityLabel(L10n.string("computer.controlModeTitle"))
    }

    @ViewBuilder
    private func computerApprovalButtons(_ approval: ComputerApproval) -> some View {
        Button(L10n.string("computer.deny")) {
            Task { await model.denyComputerControl(approval) }
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(model.computerSafetyMutationInFlight)

        Button(L10n.string("computer.allowOnce")) {
            Task { await model.approveComputerControl(approval) }
        }
        .buttonStyle(.borderedProminent)
        .controlSize(.small)
        .disabled(model.computerSafetyMutationInFlight)

        Button(L10n.string("computer.alwaysAllow")) {
            Task { await model.alwaysAllowComputerControl(approval) }
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .disabled(model.computerSafetyMutationInFlight)
        .help(L10n.string("computer.alwaysAllowHelp"))
    }

    private var computerControlStatusText: String {
        if model.computerSafety.stopped {
            return L10n.string("computer.status.stopped")
        }
        if !model.computerSafety.pendingApprovals.isEmpty {
            return L10n.string("computer.status.waiting")
        }
        switch model.computerSafety.mode {
        case .readOnly:
            return L10n.string("computer.status.readOnly")
        case .askBeforeControl:
            return L10n.string("computer.status.ask")
        case .allowSession:
            return L10n.string("computer.status.allowSession")
        case .alwaysAllow:
            return L10n.string("computer.status.alwaysAllow")
        }
    }

    private var computerControlModeDescription: String {
        switch model.computerSafety.mode {
        case .readOnly:
            return L10n.string("computer.description.readOnly")
        case .askBeforeControl:
            return L10n.string("computer.description.ask")
        case .allowSession:
            return L10n.string("computer.description.allowSession")
        case .alwaysAllow:
            return L10n.string("computer.description.alwaysAllow")
        }
    }

    private func computerActionLabel(_ action: String) -> String {
        switch action {
        case "launch_application": return L10n.string("computer.action.launch")
        case "activate_window": return L10n.string("computer.action.activate")
        case "press": return L10n.string("computer.action.press")
        case "focus": return L10n.string("computer.action.focus")
        case "scroll_to_element": return L10n.string("computer.action.scroll")
        case "key": return L10n.string("computer.action.key")
        case "input_text": return L10n.string("computer.action.inputText")
        case "pointer_move": return L10n.string("computer.action.pointerMove")
        case "pointer_click": return L10n.string("computer.action.pointerClick")
        case "write_clipboard": return L10n.string("computer.action.clipboardWrite")
        default: return L10n.string("computer.action.control")
        }
    }

    private var connectionStatusCopy: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 7) {
                if model.connectionActionInFlight {
                    ProgressView()
                        .controlSize(.small)
                } else if connectionIsUsable {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                }
                Text(connectionStatusTitle)
            }
            .chadexFont(.title3, weight: .semibold)

            Text(statusExplanation)
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: layout.control(620), alignment: .leading)

            if let message = model.connectionCheckMessage {
                Label(
                    message,
                    systemImage: model.connectionCheckSucceeded == false ? "exclamationmark.circle" : "checkmark.circle"
                )
                .chadexFont(.caption)
                .foregroundStyle(model.connectionCheckSucceeded == false ? Color.red : Color.secondary)
                .chadexPadding(.top, 2)
            }
        }
    }

    private var connectionActions: some View {
        HStack(spacing: 8) {
            if model.isSwitchingProject {
                EmptyView()
            } else if model.connectionPresentationPhase == .unconfigured {
                Button(primaryActionTitle) {
                    model.showConnectionSettings()
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.regular)
            } else if model.connectionPresentationPhase == .waitingForChatGPTVerification || model.connectionPresentationPhase == .verified {
                Button(role: .destructive) {
                    model.primaryAction()
                } label: {
                    HStack(spacing: 6) {
                        if model.connectionAction == .disconnecting {
                            ProgressView()
                                .controlSize(.mini)
                        }
                        Text(primaryActionTitle)
                    }
                }
                .buttonStyle(.bordered)
                .controlSize(.regular)
                .help(L10n.string("connection.disconnect"))
                .disabled(model.connectionActionInFlight || model.isSwitchingProject)
            } else {
                Button {
                    model.primaryAction()
                } label: {
                    HStack(spacing: 6) {
                        if model.connectionActionInFlight {
                            ProgressView()
                                .controlSize(.mini)
                        }
                        Text(primaryActionTitle)
                    }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.regular)
                .disabled(
                    model.connectionActionInFlight
                        || model.isSwitchingProject
                        || (model.connectionPresentationPhase == .preparing && model.snapshot.currentOperation?.cancellable != true)
                )
            }
        }
    }

    private func errorSection(_ error: HelperErrorPayload) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(
                error.code == "tunnel_credentials_rejected"
                    ? L10n.string("connection.credentialsRejectedTitle")
                    : error.message,
                systemImage: "exclamationmark.triangle.fill"
            )
                .chadexFont(.headline)
                .foregroundStyle(.red)

            Text(
                error.code == "tunnel_credentials_rejected"
                    ? L10n.string("connection.credentialsRejectedRecovery")
                    : (error.recovery ?? L10n.string("error.retry"))
            )
                .chadexFont(.callout)
                .foregroundStyle(.secondary)

            HStack(alignment: .top, spacing: 12) {
                if error.code == "tunnel_credentials_rejected" {
                    Button(L10n.string("connection.editSettings")) {
                        model.showConnectionSettings()
                    }
                    .buttonStyle(.borderedProminent)
                } else {
                    Button(L10n.string("connection.retry")) { model.primaryAction() }
                        .buttonStyle(.bordered)
                }

                DisclosureGroup(L10n.string("error.technicalDetails"), isExpanded: $showingErrorDetails) {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(error.code)
                            .chadexFont(.caption, design: .monospaced)
                            .textSelection(.enabled)
                        if let details = error.details {
                            Text(String(describing: details))
                                .chadexFont(.caption, design: .monospaced)
                                .textSelection(.enabled)
                        }
                    }
                    .foregroundStyle(.tertiary)
                    .chadexPadding(.top, 6)
                }
            }
        }
        .chadexPadding(13)
        .background(Color.red.opacity(0.04), in: RoundedRectangle(cornerRadius: layout.control(ChadexMetrics.contentCornerRadius), style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: layout.control(ChadexMetrics.contentCornerRadius), style: .continuous)
                .stroke(Color.red.opacity(0.14), lineWidth: 1)
        }
        .accessibilityElement(children: .contain)
    }

    private var recentActivity: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline) {
                Text(L10n.string("activity.recent"))
                    .chadexFont(.headline)
                Spacer()
                if !model.filteredActivities.isEmpty {
                    Button(L10n.string("activity.viewAll"), action: onShowAllActivity)
                        .buttonStyle(.link)
                        .chadexFont(.callout)
                }
            }

            if model.filteredActivities.isEmpty {
                HStack(spacing: 9) {
                    Image(systemName: "clock.arrow.circlepath")
                        .foregroundStyle(.tertiary)
                    Text(L10n.string("activity.empty"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                }
                .chadexPadding(.vertical, 12)
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(model.filteredActivities.prefix(4))) { activity in
                        ActivityRow(entry: activity)
                            .chadexPadding(.vertical, 5)
                        if activity.id != model.filteredActivities.prefix(4).last?.id {
                            Divider()
                        }
                    }
                }
            }
        }
    }

    private var lastVerifiedValue: String {
        guard let verified = model.snapshot.lastVerifiedAtMs else { return L10n.string("connection.notVerifiedShort") }
        return Date(timeIntervalSince1970: Double(verified) / 1000)
            .formatted(date: .abbreviated, time: .shortened)
    }

    private var tunnelIDDisplayValue: String {
        let tunnelID = model.preferences.tunnelID
        guard !tunnelID.isEmpty else { return "—" }
        guard tunnelID.count > 24 else { return tunnelID }
        return "\(tunnelID.prefix(13))…\(tunnelID.suffix(8))"
    }

    private var primaryActionTitle: String {
        if let actionText = model.connectionActionStatusText { return actionText }
        switch model.connectionPresentationPhase {
        case .unconfigured: return L10n.string("connection.configure")
        case .preparing: return model.snapshot.currentOperation?.cancellable == true ? L10n.string("connection.cancel") : L10n.string("status.preparing")
        case .waitingForChatGPTVerification: return L10n.string("connection.disconnect")
        case .verified: return L10n.string("connection.disconnect")
        case .stopped: return L10n.string("connection.connect")
        case .error: return L10n.string("connection.retry")
        }
    }

    private var connectionStatusTitle: String {
        if model.isSwitchingProject {
            return model.switchingProjectName.map { L10n.string("project.switching", $0) }
                ?? L10n.string("status.preparing")
        }
        if let actionText = model.connectionActionStatusText { return actionText }
        if model.connectionPresentationPhase == .waitingForChatGPTVerification {
            return model.snapshot.chatGPTConnected
                ? L10n.string("status.chatGPTConnected")
                : L10n.string("status.waiting")
        }
        return StatusPresentation.text(for: model.connectionPresentationPhase)
    }

    private var connectionIsUsable: Bool {
        !model.isSwitchingProject
            && (model.snapshot.chatGPTConnected
                || model.snapshot.chatGPTVerifiedForSelectedProject)
    }

    private var statusExplanation: String {
        if model.isSwitchingProject {
            return L10n.string("connection.explainPreparing")
        }
        if let actionExplanation = model.connectionActionExplanationText { return actionExplanation }
        switch model.connectionPresentationPhase {
        case .unconfigured: return L10n.string("connection.explainUnconfigured")
        case .preparing: return L10n.string("connection.explainPreparing")
        case .waitingForChatGPTVerification:
            return model.snapshot.chatGPTConnected ? L10n.string("connection.explainConnected") : L10n.string("connection.explainReadyToUse")
        case .verified: return L10n.string("connection.explainVerified")
        case .stopped: return L10n.string("connection.explainStopped")
        case .error: return L10n.string("connection.explainError")
        }
    }

}

struct GlobalInstructionsEditor: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    @State private var draft = ""
    @State private var loadedDraft = false
    @State private var saved = false

    private var byteCount: Int { draft.lengthOfBytes(using: .utf8) }
    private var canSave: Bool {
        byteCount <= ProjectStore.maxGlobalInstructionsBytes
            && draft != model.globalInstructions
            && !model.globalInstructionsSaving
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                SectionEyebrow(title: L10n.string("globalInstructions.title"))
                Spacer(minLength: 12)
                if saved {
                    Label(L10n.string("globalInstructions.saved"), systemImage: "checkmark")
                        .chadexFont(.caption, weight: .medium)
                        .foregroundStyle(.secondary)
                }
                Button {
                    saved = model.saveGlobalInstructions(draft)
                } label: {
                    if model.globalInstructionsSaving {
                        ProgressView().controlSize(.small)
                    } else {
                        Text(L10n.string("globalInstructions.save"))
                    }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.small)
                .disabled(!canSave)
            }

            Text(L10n.string("globalInstructions.subtitle"))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            TextEditor(text: $draft)
                .font(.system(.body, design: .monospaced))
                .frame(minHeight: layout.control(180), maxHeight: layout.control(320))
                .padding(7)
                .background(.quaternary.opacity(0.16), in: RoundedRectangle(cornerRadius: layout.control(9), style: .continuous))
                .onChange(of: draft) { _, _ in saved = false }

            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Label(L10n.string("globalInstructions.storageNote"), systemImage: "externaldrive")
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 10)
                Text("\(byteCount) / \(ProjectStore.maxGlobalInstructionsBytes) bytes")
                    .chadexFont(.caption, design: .monospaced)
                    .foregroundStyle(byteCount > ProjectStore.maxGlobalInstructionsBytes ? Color.red : Color.secondary)
            }

            Text(L10n.string("globalInstructions.precedence"))
                .chadexFont(.caption)
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)

            if let error = model.globalInstructionsError {
                Label(error, systemImage: "exclamationmark.triangle")
                    .chadexFont(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .onAppear { loadDraftIfNeeded() }
        .onChange(of: model.globalInstructions) { _, value in
            guard draft == model.globalInstructions || !loadedDraft else { return }
            draft = value
        }
    }

    private func loadDraftIfNeeded() {
        guard !loadedDraft else { return }
        draft = model.globalInstructions
        loadedDraft = true
    }
}

struct GlobalInstructionsStandaloneView: View {
    @Environment(\.chadexLayout) private var layout

    var body: some View {
        ScrollView {
            GlobalInstructionsEditor()
                .frame(maxWidth: layout.control(ChadexMetrics.detailMaxWidth), alignment: .leading)
                .chadexPadding(.horizontal, ChadexMetrics.detailHorizontalPadding)
                .chadexPadding(.vertical, ChadexMetrics.detailVerticalPadding)
                .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .navigationTitle(L10n.string("sidebar.agentSettings"))
    }
}

enum ActivityPresentation {
    static func message(for entry: ActivityEntry) -> String {
        switch entry.eventKind {
        case "local_runtime_ready":
            return L10n.string("activity.event.localRuntimeReady")
        case "local_setup_preparing":
            return L10n.string("activity.event.localSetupPreparing")
        case "process_started":
            if let pid = processID(in: entry.message) {
                return L10n.string("activity.event.processStarted", pid)
            }
            return L10n.string("activity.event.processStartedGeneric")
        default:
            return entry.message
        }
    }

    private static func processID(in message: String) -> String? {
        guard let marker = message.range(of: "PID ") else { return nil }
        let remainder = message[marker.upperBound...]
        let digits = remainder.prefix(while: { $0.isNumber })
        return digits.isEmpty ? nil : String(digits)
    }
}

struct ActivityRow: View {
    let entry: ActivityEntry

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: symbol)
                .chadexFont(.caption2, weight: .medium)
                .frame(width: 16, height: 16)
                .foregroundStyle(iconColor)

            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 10) {
                    Text(ActivityPresentation.message(for: entry))
                        .chadexFont(.callout)
                        .fixedSize(horizontal: false, vertical: true)

                    Spacer(minLength: 12)

                    Text(entry.date.formatted(date: .omitted, time: .shortened))
                        .chadexFont(.caption, design: .monospaced)
                        .monospacedDigit()
                        .foregroundStyle(.tertiary)
                }

                Text("\(entry.source) · \(entry.eventKind)")
                    .chadexFont(.caption, design: .monospaced)
                    .foregroundStyle(.tertiary)
            }
        }
        .contextMenu {
            Button(L10n.string("activity.copyMessage")) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(ActivityPresentation.message(for: entry), forType: .string)
            }
            Button(L10n.string("activity.copyDetails")) {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(
                    "\(entry.source) · \(entry.eventKind) · \(entry.date.formatted(date: .abbreviated, time: .standard))",
                    forType: .string
                )
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var iconColor: Color {
        switch entry.level {
        case .info: return .secondary
        case .warning: return .orange
        case .error: return .red
        }
    }

    private var symbol: String {
        switch entry.level {
        case .info: return "info.circle"
        case .warning: return "exclamationmark.triangle"
        case .error: return "xmark.octagon"
        }
    }
}
