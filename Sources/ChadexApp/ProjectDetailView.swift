import AppKit
import SwiftUI

struct ProjectDetailView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord
    let destination: ProjectWorkspaceDestination
    let onShowAllActivity: () -> Void
    /// Navigates to the Skills page; Agent Settings only links to it.
    var onManageSkills: () -> Void = {}
    @State private var showingErrorDetails = false
    @State private var pendingComputerMode: ComputerControlMode?
    @StateObject private var computerPermissions = ComputerPermissionMonitor()

    var body: some View {
        ChadexPageColumn {
            workspaceContent
        }
        .navigationTitle(navigationTitle)
        .task(id: "\(project.id.uuidString):\(destination.rawValue)") {
            guard model.selectedProject?.id == project.id else { return }

            switch destination {
            case .computerUse:
                await model.refreshComputerSafety()
            case .overview, .agentSettings, .skills:
                // The Skills page refreshes its own catalog.
                break
            }
        }
    }

    @ViewBuilder
    private var workspaceContent: some View {
        switch destination {
        case .overview:
            header
            connectionSection
                .chadexCard(padding: 22)

            if let error = model.connectionError {
                errorSection(error)
                    .chadexAttentionCard(tint: .red)
            }

            if let task = model.snapshot.taskProgress {
                TaskProgressView(task: task) {
                    Task { await model.cancelTask(task) }
                }
                .chadexCard()
            }

            recentActivity

        case .agentSettings:
            agentSettingsSection

        case .skills:
            SkillsCenterView(project: project)

        case .computerUse:
            computerControlSection
        }
    }

    private var computerControlNeedsAttention: Bool {
        model.computerSafety.stopped
            || !model.computerSafety.pendingApprovals.isEmpty
            || !computerPermissions.state.allGranted
    }

    private var navigationTitle: String {
        switch destination {
        case .overview:
            return project.name
        case .agentSettings:
            return L10n.string("sidebar.agentSettings")
        case .skills:
            return L10n.string("sidebar.skills")
        case .computerUse:
            return L10n.string("sidebar.computerUse")
        }
    }

    private var agentSettingsSection: some View {
        VStack(alignment: .leading, spacing: layout.spacing(20)) {
            ChadexPageHeader(
                title: L10n.string("sidebar.agentSettings"),
                subtitle: L10n.string("agentSettings.subtitle")
            ) {
                // Global settings: no per-project context in the header.
                EmptyView()
            }

            GlobalInstructionsEditor()
                .chadexCard(padding: 20)

            skillsLink
                .chadexCard(padding: 16)
        }
    }

    private var skillsLink: some View {
        HStack(alignment: .center, spacing: layout.spacing(14)) {
            Image(systemName: "puzzlepiece.extension.fill")
                .font(.system(size: layout.control(15), weight: .semibold))
                .foregroundStyle(.white)
                .frame(width: layout.control(34), height: layout.control(34))
                .background(Color.indigo, in: RoundedRectangle(cornerRadius: layout.control(9), style: .continuous))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                Text(L10n.string("agentSettings.skillsLink.title"))
                    .chadexFont(.callout, weight: .semibold)
                Text(L10n.string("agentSettings.skillsLink.message"))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 12)
            Button(L10n.string("agentSettings.manageSkills")) { onManageSkills() }
                .chadexControlSize(.regular)
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: layout.spacing(8)) {
            Text(project.name)
                .chadexFont(.title, weight: .semibold)
                .tracking(-0.3)
                .accessibilityAddTraits(.isHeader)

            Button {
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(project.path, forType: .string)
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "folder")
                        .foregroundStyle(.secondary)
                    Text(abbreviatedPath)
                        .chadexFont(.caption, design: .monospaced)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Image(systemName: "doc.on.doc")
                        .foregroundStyle(.secondary)
                }
                .chadexFont(.caption)
                .foregroundStyle(.primary)
                .padding(.horizontal, 9)
                .padding(.vertical, 4)
                .background(Color.secondary.opacity(0.1), in: Capsule())
                .contentShape(Capsule())
            }
            .buttonStyle(.plain)
            .help(L10n.string("project.copyPath") + " — " + project.path)
            .accessibilityLabel(L10n.string("project.copyPath"))
            .accessibilityValue(project.path)
        }
    }

    private func inlineMetadata(
        _ title: String,
        value: String,
        monospaced: Bool = false,
        copyValue: String? = nil
    ) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Text(title)
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: ChadexFontStyle.caption.size(at: layout.fontScale), design: monospaced ? .monospaced : .default))
                .foregroundStyle(.primary)
                .lineLimit(1)
                .truncationMode(.middle)
                .textSelection(.enabled)
            if let copyValue {
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(copyValue, forType: .string)
                } label: {
                    Image(systemName: "doc.on.doc")
                }
                .buttonStyle(.borderless)
                .help(L10n.string("common.copy"))
                .accessibilityLabel(L10n.string("common.copy") + " " + title)
            }
        }
        .chadexFont(.caption)
        .fixedSize()
        .accessibilityElement(children: .combine)
    }

    private var abbreviatedPath: String {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return project.path.hasPrefix(home) ? "~" + project.path.dropFirst(home.count) : project.path
    }

    private var connectionSection: some View {
        VStack(alignment: .leading, spacing: layout.spacing(20)) {
            // Copy wraps inside its own column so the action stays pinned to
            // the card's top-right corner at every width.
            HStack(alignment: .top, spacing: layout.spacing(24)) {
                connectionStatusCopy
                    .frame(maxWidth: layout.control(560), alignment: .leading)
                Spacer(minLength: 0)
                connectionActions
                    .fixedSize()
            }

            ConnectionCircuitView(
                projectName: project.name,
                phase: model.connectionPresentationPhase,
                tunnelReady: model.isSwitchingProject ? false : model.snapshot.tunnelReady,
                chatGPTConnected: model.isSwitchingProject ? false : model.snapshot.chatGPTConnected,
                chatGPTVerified: model.isSwitchingProject ? false : model.snapshot.chatGPTVerifiedForSelectedProject,
                isConnecting: model.connectionAction == .connecting,
                accent: model.connectionPresentationPhase == .verified ? .green : ChadexBrand.signal
            )
            .frame(maxWidth: .infinity)
            .chadexPadding(.vertical, 4)

            Divider()

            HStack(alignment: .firstTextBaseline, spacing: layout.spacing(24)) {
                inlineMetadata(
                    L10n.string("settings.tunnelID"),
                    value: tunnelIDDisplayValue,
                    monospaced: true,
                    copyValue: model.preferences.tunnelID.isEmpty ? nil : model.preferences.tunnelID
                )
                if model.snapshot.lastVerifiedAtMs != nil {
                    inlineMetadata(L10n.string("connection.lastVerifiedLabel"), value: lastVerifiedValue)
                }
                Spacer(minLength: 0)
            }
        }
    }

    private var computerControlSection: some View {
        VStack(alignment: .leading, spacing: layout.spacing(22)) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                SectionTitle(title: L10n.string("computer.title"))
                Spacer(minLength: 12)
                ChadexStatusLabel(
                    title: computerControlStatusText,
                    systemImage: model.computerSafety.stopped
                        ? "hand.raised.fill"
                        : (model.computerSafety.pendingApprovals.isEmpty && computerPermissions.state.allGranted
                            ? "desktopcomputer" : "exclamationmark.circle.fill"),
                    tint: computerControlNeedsAttention ? .orange : .secondary,
                    emphasized: computerControlNeedsAttention
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
                    .buttonStyle(ChadexButtonStyle(kind: .primary))
                    .chadexControlSize(.regular)
                    .disabled(model.computerSafetyMutationInFlight)
                    .help(L10n.string("computer.resumeHelp"))
                }
            } else {
                // ChatGPT is waiting on this request, so it leads the page.
                if let approval = model.computerSafety.pendingApprovals.first {
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
                            .foregroundStyle(.secondary)
                        }
                    }
                    .chadexAttentionCard(tint: .orange, padding: 18)
                }

                // Without these grants every control request fails, so the
                // card says so before the person picks a mode.
                ComputerPermissionsCard(monitor: computerPermissions)

                VStack(alignment: .leading, spacing: layout.spacing(10)) {
                    Text(L10n.string("computer.controlModeTitle"))
                        .chadexFont(.headline, weight: .semibold)
                        .accessibilityAddTraits(.isHeader)
                    Text(L10n.string("computer.controlModeHelp"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    computerControlModePicker
                        .chadexPadding(.top, 4)
                    if !model.snapshot.tunnelReady {
                        Text(L10n.string("computer.sessionUnavailable"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                    }
                }

                HStack(alignment: .top, spacing: layout.spacing(14)) {
                    Image(systemName: "lock.shield.fill")
                        .font(.system(size: layout.control(16), weight: .semibold))
                        .foregroundStyle(ChadexBrand.onSignal)
                        .frame(width: layout.control(34), height: layout.control(34))
                        .background(ChadexBrand.signal, in: RoundedRectangle(cornerRadius: layout.control(9), style: .continuous))
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: layout.spacing(6)) {
                        Text(L10n.string("computer.safetyTitle"))
                            .chadexFont(.headline, weight: .semibold)
                            .accessibilityAddTraits(.isHeader)
                        Text(L10n.string("computer.safetyNote"))
                            .chadexFont(.callout)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    Spacer(minLength: layout.spacing(12))
                    Button(role: .destructive) {
                        Task { await model.stopComputerControl() }
                    } label: {
                        Label(L10n.string("computer.stop"), systemImage: "stop.circle.fill")
                    }
                    .buttonStyle(ChadexButtonStyle(kind: .secondary))
                    .chadexControlSize(.regular)
                    .fixedSize()
                    .disabled(model.computerSafetyMutationInFlight)
                    .help(L10n.string("computer.stopHelp"))
                }
                .chadexCard(padding: 18)
            }

            Divider()
            computerCursorOverlayToggle

            if let error = model.computerSafetyError {
                ChadexInlineError(message: error) {
                    await model.refreshComputerSafety()
                }
            }
        }
    }

    private var computerCursorOverlayToggle: some View {
        VStack(alignment: .leading, spacing: layout.spacing(6)) {
            Toggle(
                L10n.string("computer.cursorOverlay.title"),
                isOn: Binding(
                    get: { model.computerCursorOverlayEnabled },
                    set: { enabled in
                        Task { await model.setComputerCursorOverlay(enabled) }
                    }
                )
            )
            .chadexFont(.callout, weight: .semibold)
            Text(L10n.string("computer.cursorOverlay.help"))
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var computerControlModePicker: some View {
        let modes: [ComputerControlMode] = model.snapshot.tunnelReady
            ? [.readOnly, .askBeforeControl, .allowSession, .alwaysAllow]
            : [.readOnly, .askBeforeControl, .alwaysAllow]
        return LazyVGrid(
            columns: [GridItem(.adaptive(minimum: layout.control(220)), spacing: layout.spacing(12), alignment: .top)],
            alignment: .leading,
            spacing: layout.spacing(12)
        ) {
            ForEach(modes) { mode in
                ComputerModeCard(
                    mode: mode,
                    isSelected: model.computerSafety.mode == mode,
                    isPending: pendingComputerMode == mode
                ) {
                    computerModeBinding.wrappedValue = mode
                }
                .disabled(model.computerSafetyMutationInFlight)
            }
        }
        // VoiceOver keeps hearing one native radio group (count, selection,
        // arrow keys) instead of three or four unrelated buttons.
        .accessibilityRepresentation {
            Picker(L10n.string("computer.controlModeTitle"), selection: computerModeBinding) {
                ForEach(modes) { mode in
                    Text(L10n.string(mode.titleKey)).tag(mode)
                }
            }
            .pickerStyle(.radioGroup)
            .disabled(model.computerSafetyMutationInFlight)
            .accessibilityValue(L10n.string(model.computerSafety.mode.descriptionKey))
        }
    }

    private var computerModeBinding: Binding<ComputerControlMode> {
        Binding(
            get: { model.computerSafety.mode },
            set: { mode in
                // Saving the default first and then being turned away by the
                // in-flight guard would leave the stored and live modes apart.
                guard !model.computerSafetyMutationInFlight, mode != model.computerSafety.mode else { return }
                pendingComputerMode = mode
                Task {
                    if mode == .allowSession {
                        await model.setComputerControlMode(mode)
                    } else {
                        await model.setComputerControlDefaultMode(mode)
                    }
                    pendingComputerMode = nil
                }
            }
        )
    }

    @ViewBuilder
    private func computerApprovalButtons(_ approval: ComputerApproval) -> some View {
        Button(L10n.string("computer.deny")) {
            Task { await model.denyComputerControl(approval) }
        }
        .buttonStyle(ChadexButtonStyle(kind: .secondary))
        .chadexControlSize(.small)
        .disabled(model.computerSafetyMutationInFlight)

        Button(L10n.string("computer.allowOnce")) {
            Task { await model.approveComputerControl(approval) }
        }
        .buttonStyle(ChadexButtonStyle(kind: .primary))
        .chadexControlSize(.small)
        .disabled(model.computerSafetyMutationInFlight)

        Button(L10n.string("computer.alwaysAllow")) {
            Task { await model.alwaysAllowComputerControl(approval) }
        }
        .buttonStyle(ChadexButtonStyle(kind: .secondary))
        .chadexControlSize(.small)
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
        if !computerPermissions.state.allGranted {
            return L10n.string("computer.status.needsPermission")
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
        VStack(alignment: .leading, spacing: 8) {
            ChadexStatusPill(
                title: L10n.string("connection.title"),
                tint: connectionPillTint,
                pulsing: model.connectionActionInFlight || model.connectionPresentationPhase == .waitingForChatGPTVerification
            )
            .padding(.bottom, 2)

            Text(connectionStatusTitle)
                .chadexFont(.title2, weight: .semibold)
                .tracking(-0.2)

            Text(statusExplanation)
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: layout.control(620), alignment: .leading)

            if let message = model.connectionCheckMessage {
                let failed = model.connectionCheckSucceeded == false
                HStack(alignment: .firstTextBaseline, spacing: 5) {
                    Image(systemName: failed ? "exclamationmark.circle.fill" : "checkmark.circle")
                        .foregroundStyle(failed ? Color.red : Color.secondary)
                    Text(message)
                        .foregroundStyle(failed ? Color.primary : Color.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .chadexFont(.caption)
                .accessibilityElement(children: .combine)
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
                .buttonStyle(ChadexButtonStyle(kind: .primary))
                .chadexControlSize(.regular)
            } else if model.connectionPresentationPhase == .waitingForChatGPTVerification || model.connectionPresentationPhase == .verified {
                Button(role: .destructive) {
                    model.primaryAction()
                } label: {
                    HStack(spacing: 6) {
                        if model.connectionAction == .disconnecting {
                            // Kept within the label's line height so the
                            // button does not grow while busy.
                            ProgressView()
                                .controlSize(.mini)
                                .frame(width: 12, height: 12)
                        }
                        Text(primaryActionTitle)
                    }
                }
                .buttonStyle(ChadexButtonStyle(kind: .secondary))
                .chadexControlSize(.regular)
                .help(L10n.string("connection.disconnect"))
                .disabled(model.connectionActionInFlight || model.isSwitchingProject)
            } else {
                Button {
                    model.primaryAction()
                } label: {
                    HStack(spacing: 6) {
                        if model.connectionActionInFlight {
                            // Kept within the label's line height so the
                            // button does not grow while busy.
                            ProgressView()
                                .controlSize(.mini)
                                .frame(width: 12, height: 12)
                        }
                        Text(primaryActionTitle)
                    }
                }
                .buttonStyle(ChadexButtonStyle(kind: .primary))
                .chadexControlSize(.regular)
                .disabled(!model.primaryActionEnabled)
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
                .foregroundStyle(.primary, .red)

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
                    .buttonStyle(ChadexButtonStyle(kind: .primary))
                } else {
                    Button(L10n.string("connection.retry")) { model.primaryAction() }
                        .buttonStyle(ChadexButtonStyle(kind: .secondary))
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
                    .foregroundStyle(.secondary)
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
        VStack(alignment: .leading, spacing: layout.spacing(10)) {
            HStack(alignment: .firstTextBaseline) {
                SectionTitle(title: L10n.string("activity.recent"))
                Spacer()
                if !model.recentActivities.isEmpty {
                    Button {
                        onShowAllActivity()
                    } label: {
                        HStack(spacing: 3) {
                            Text(L10n.string("activity.viewAll"))
                            Image(systemName: "chevron.right")
                                .imageScale(.small)
                        }
                    }
                    .buttonStyle(.link)
                    .chadexFont(.callout)
                }
            }

            Group {
                if model.recentActivities.isEmpty {
                    HStack(spacing: 10) {
                        Image(systemName: "clock.arrow.circlepath")
                            .foregroundStyle(.secondary)
                            .accessibilityHidden(true)
                        Text(L10n.string("activity.empty"))
                            .chadexFont(.callout)
                            .foregroundStyle(.secondary)
                    }
                    .chadexPadding(.vertical, 6)
                } else {
                    ActivityTimeline(entries: Array(model.recentActivities.prefix(5)))
                }
            }
            .chadexCard(padding: 16)
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
        model.primaryActionTitle
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

    private var connectionPillTint: Color {
        switch model.connectionPresentationPhase {
        case .verified: return .green
        case .waitingForChatGPTVerification, .preparing: return ChadexBrand.signal
        case .error: return .red
        case .unconfigured, .stopped: return .secondary
        }
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
                SectionTitle(title: L10n.string("globalInstructions.title"))
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
                        ProgressView().chadexControlSize(.small)
                    } else {
                        Text(L10n.string("globalInstructions.save"))
                    }
                }
                .buttonStyle(ChadexButtonStyle(kind: .primary))
                .chadexControlSize(.small)
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
                .chadexEditorSurface()
                .onChange(of: draft) { _, _ in saved = false }

            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Label(L10n.string("globalInstructions.storageNote"), systemImage: "externaldrive")
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                Spacer(minLength: 10)
                Text(L10n.string(
                    "globalInstructions.byteCount",
                    byteCount.formatted(),
                    ProjectStore.maxGlobalInstructionsBytes.formatted()
                ))
                    .chadexFont(.caption, design: .monospaced)
                    .foregroundStyle(byteCount > ProjectStore.maxGlobalInstructionsBytes ? Color.red : Color.secondary)
                    .fontWeight(byteCount > ProjectStore.maxGlobalInstructionsBytes ? .bold : .regular)
            }

            Text(L10n.string("globalInstructions.precedence"))
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
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
    var body: some View {
        ChadexPageColumn {
            GlobalInstructionsEditor()
                .chadexCard(padding: 20)
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
        case "project_activated":
            return L10n.string("activity.event.projectActivated", entry.message)
        case "operation_started":
            return operationMessage(entry.message)
        default:
            return entry.message
        }
    }

    /// Symbol that says what kind of thing happened, not just its severity.
    static func symbol(for entry: ActivityEntry) -> String {
        switch entry.level {
        case .warning: return "exclamationmark.triangle.fill"
        case .error: return "xmark.octagon.fill"
        case .info: break
        }
        switch entry.eventKind {
        case "project_activated": return "folder.fill"
        case "operation_started": return "play.fill"
        case "local_runtime_ready": return "checkmark"
        case "local_setup_preparing": return "hammer.fill"
        case "process_started": return "gearshape.2.fill"
        default:
            if entry.eventKind.contains("tunnel") || entry.eventKind.contains("connect") { return "link" }
            if entry.eventKind.contains("tool") || entry.eventKind.contains("mcp") { return "wrench.and.screwdriver.fill" }
            return "circle.fill"
        }
    }

    private static func operationMessage(_ message: String) -> String {
        let marker = "operation started: "
        guard let range = message.range(of: marker) else { return message }
        let operation = String(message[range.upperBound...]).trimmingCharacters(in: .whitespaces)
        let key = "activity.operation.\(operation)"
        let localized = L10n.string(key)
        return localized == key
            ? L10n.string("activity.operation.generic", operation)
            : localized
    }

    private static func processID(in message: String) -> String? {
        guard let marker = message.range(of: "PID ") else { return nil }
        let remainder = message[marker.upperBound...]
        let digits = remainder.prefix(while: { $0.isNumber })
        return digits.isEmpty ? nil : String(digits)
    }
}

/// One Computer Use mode as a selectable card: the symbol and title name the
/// mode, the description says what ChatGPT can do under it.
private struct ComputerModeCard: View {
    @Environment(\.chadexLayout) private var layout
    let mode: ComputerControlMode
    let isSelected: Bool
    var isPending = false
    let action: () -> Void
    @State private var hovering = false
    @FocusState private var focused: Bool

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: layout.control(12), style: .continuous)
        Button(action: action) {
            VStack(alignment: .leading, spacing: layout.spacing(8)) {
                HStack(spacing: layout.spacing(8)) {
                    Image(systemName: symbol)
                        .font(.system(size: layout.control(13), weight: .semibold))
                        .foregroundStyle(ChadexBrand.glyph(on: tint))
                        .frame(width: layout.control(28), height: layout.control(28))
                        .background(tint, in: Circle())
                    Text(L10n.string(mode.titleKey))
                        .chadexFont(.callout, weight: .semibold)
                        .foregroundStyle(.primary)
                    Spacer(minLength: 4)
                    if isPending {
                        ProgressView().chadexControlSize(.small)
                    } else {
                        Image(systemName: isSelected ? "checkmark.circle.fill" : "circle")
                            .font(.system(size: layout.control(15)))
                            .foregroundStyle(isSelected ? ChadexBrand.signal : Color.secondary.opacity(0.5))
                    }
                }
                Text(L10n.string(mode.descriptionKey))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.leading)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .chadexPadding(14)
            .background(ChadexBrand.cardFill, in: shape)
            .overlay(
                shape.strokeBorder(
                    isSelected ? ChadexBrand.signal : (hovering ? Color.secondary.opacity(0.45) : ChadexBrand.cardStroke),
                    lineWidth: isSelected ? 2 : 1
                )
            )
            .contentShape(shape)
        }
        .buttonStyle(.plain)
        // Keyboard navigation gets a ring that follows the card's corners
        // instead of the default rectangle around a plain button.
        .focused($focused)
        .focusEffectDisabled()
        .overlay {
            if focused {
                RoundedRectangle(cornerRadius: layout.control(12) + 3, style: .continuous)
                    .strokeBorder(Color(nsColor: .keyboardFocusIndicatorColor), lineWidth: 3)
                    .padding(-3)
                    .accessibilityHidden(true)
            }
        }
        .onHover { hovering = $0 }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(L10n.string(mode.titleKey))
        .accessibilityHint(L10n.string(mode.descriptionKey))
        .accessibilityAddTraits(isSelected ? [.isButton, .isSelected] : .isButton)
    }

    private var symbol: String {
        switch mode {
        case .askBeforeControl: return "hand.raised.fill"
        case .allowSession: return "clock.fill"
        case .alwaysAllow: return "bolt.fill"
        case .readOnly: return "eye.fill"
        }
    }

    /// Hue grows with how much control the mode hands over.
    private var tint: Color {
        switch mode {
        case .readOnly: return .gray
        case .askBeforeControl: return ChadexBrand.signal
        case .allowSession: return .indigo
        case .alwaysAllow: return .orange
        }
    }
}

private extension ComputerControlMode {
    var titleKey: String {
        switch self {
        case .askBeforeControl: return "computer.mode.ask"
        case .allowSession: return "computer.mode.allowSession"
        case .alwaysAllow: return "computer.mode.alwaysAllow"
        case .readOnly: return "computer.mode.readOnly"
        }
    }

    var descriptionKey: String {
        switch self {
        case .askBeforeControl: return "computer.description.ask"
        case .allowSession: return "computer.description.allowSession"
        case .alwaysAllow: return "computer.description.alwaysAllow"
        case .readOnly: return "computer.description.readOnly"
        }
    }
}
