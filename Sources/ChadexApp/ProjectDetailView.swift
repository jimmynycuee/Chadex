import AppKit
import SwiftUI

struct ProjectDetailView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord
    let onShowAllActivity: () -> Void
    @State private var showingErrorDetails = false
    @State private var showingEffectiveInstructions = false
    @State private var showingAgentsDraft = false
    @State private var agentsDraft = AppModel.agentsDraftTemplate

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: layout.spacing(ChadexMetrics.sectionSpacing)) {
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
                instructionsSection

                Divider()
                SkillsCenterView(project: project)

                Divider()
                ProjectMemoryView(project: project)

                Divider()
                recentActivity
            }
            .frame(maxWidth: layout.control(ChadexMetrics.detailMaxWidth), alignment: .leading)
            .chadexPadding(.horizontal, ChadexMetrics.detailHorizontalPadding)
            .chadexPadding(.vertical, ChadexMetrics.detailVerticalPadding)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .navigationTitle(project.name)
        .task(id: project.id) {
            guard model.selectedProject?.id == project.id else { return }
            await model.refreshProjectInstructions()
            await model.refreshSkills()
            await model.refreshProjectMemory()
        }
        .sheet(isPresented: $showingAgentsDraft) {
            AgentsDraftSheet(project: project, draft: $agentsDraft)
                .environmentObject(model)
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
            Label(error.message, systemImage: "exclamationmark.triangle.fill")
                .chadexFont(.headline)
                .foregroundStyle(.red)

            Text(error.recovery ?? L10n.string("error.retry"))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)

            HStack(alignment: .top, spacing: 12) {
                Button(L10n.string("connection.retry")) { model.primaryAction() }
                    .buttonStyle(.bordered)

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

    private var instructionsSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .firstTextBaseline, spacing: 12) {
                SectionEyebrow(title: L10n.string("instructions.title"))
                Spacer(minLength: 12)

                Button {
                    Task { await model.refreshProjectInstructions() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                }
                .buttonStyle(.borderless)
                .disabled(model.projectInstructionsLoading || model.isSwitchingProject)
                .help(L10n.string("instructions.refresh"))
                .accessibilityLabel(L10n.string("instructions.refresh"))

                Button(L10n.string("instructions.create")) {
                    agentsDraft = AppModel.agentsDraftTemplate
                    showingAgentsDraft = true
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(
                    model.projectAgentsWriteInFlight
                        || model.projectInstructionsLoading
                        || model.isSwitchingProject
                        || model.projectInstructions?.isAvailable != true
                        || model.projectInstructions?.hasTargetAgentsFile == true
                )
            }

            Text(L10n.string("instructions.subtitle"))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(L10n.string("instructions.target"))
                    .chadexFont(.caption, weight: .semibold)
                    .foregroundStyle(.secondary)
                Text(project.path)
                    .chadexFont(.caption, design: .monospaced)
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
            }

            if model.projectInstructionsLoading && model.projectInstructions == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(L10n.string("instructions.loading"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                }
                .chadexPadding(.vertical, 8)
            } else if let error = model.projectInstructionsError, model.projectInstructions == nil {
                Label(error, systemImage: "exclamationmark.triangle")
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let inspection = model.projectInstructions {
                if !model.projectInstructionChangedPaths.isEmpty {
                    Label(
                        L10n.string("instructions.changed", model.projectInstructionChangedPaths.joined(separator: ", ")),
                        systemImage: "arrow.triangle.2.circlepath"
                    )
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                }

                if inspection.sources.isEmpty {
                    Label(
                        inspection.isAvailable ? L10n.string("instructions.none") : L10n.string("instructions.unavailable"),
                        systemImage: inspection.isAvailable ? "doc.badge.plus" : "exclamationmark.circle"
                    )
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
                    .chadexPadding(.vertical, 6)

                    VStack(spacing: 0) {
                        currentTaskRow(index: 0)
                    }
                    .background(.quaternary.opacity(0.16), in: RoundedRectangle(cornerRadius: layout.control(10), style: .continuous))
                } else {
                    Text(L10n.string("instructions.hierarchy"))
                        .chadexFont(.headline)

                    VStack(spacing: 0) {
                        ForEach(Array(inspection.sources.enumerated()), id: \.element.id) { index, source in
                            instructionSourceRow(source, index: index)
                            if index < inspection.sources.count - 1 {
                                Divider().padding(.leading, layout.spacing(30))
                            }
                        }

                        Divider()
                        currentTaskRow(index: inspection.sources.count)
                    }
                    .background(.quaternary.opacity(0.16), in: RoundedRectangle(cornerRadius: layout.control(10), style: .continuous))

                    Text(L10n.string("instructions.broadToSpecific"))
                        .chadexFont(.caption)
                        .foregroundStyle(.tertiary)

                    if inspection.contentIncluded && !inspection.effectiveContent.isEmpty {
                        DisclosureGroup(L10n.string("instructions.effective"), isExpanded: $showingEffectiveInstructions) {
                            Text(inspection.effectiveContent)
                                .chadexFont(.caption, design: .monospaced)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .chadexPadding(.top, 8)
                        }
                        .chadexFont(.callout, weight: .medium)
                    }
                }
            }
        }
    }

    private func instructionSourceRow(_ source: ProjectInstructionSource, index: Int) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text("\(index + 1)")
                .chadexFont(.caption, design: .monospaced)
                .foregroundStyle(.tertiary)
                .frame(width: layout.control(20), alignment: .trailing)

            VStack(alignment: .leading, spacing: 3) {
                Text(source.path)
                    .chadexFont(.callout, weight: .medium, design: .monospaced)
                    .textSelection(.enabled)

                HStack(spacing: 7) {
                    Text(source.isInherited ? L10n.string("instructions.inherited") : L10n.string("instructions.targetScope"))
                    Text(String(source.fingerprint.prefix(12)))
                        .monospaced()
                        .help(source.fingerprint)
                }
                .chadexFont(.caption)
                .foregroundStyle(.tertiary)
            }

            Spacer(minLength: 12)

            if source.truncated {
                Image(systemName: "ellipsis.circle")
                    .foregroundStyle(.tertiary)
                    .help(L10n.string("instructions.truncated"))
            }
        }
        .chadexPadding(.horizontal, 10)
        .chadexPadding(.vertical, 8)
    }

    private func currentTaskRow(index: Int) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Text("\(index + 1)")
                .chadexFont(.caption, design: .monospaced)
                .foregroundStyle(.tertiary)
                .frame(width: layout.control(20), alignment: .trailing)

            Image(systemName: "bubble.left.and.text.bubble.right")
                .chadexFont(.caption)
                .foregroundStyle(Color.accentColor)
                .frame(width: layout.control(18))

            VStack(alignment: .leading, spacing: 3) {
                Text(L10n.string("instructions.currentTask"))
                    .chadexFont(.callout, weight: .medium)
                Text(L10n.string("instructions.currentTaskNote"))
                    .chadexFont(.caption)
                    .foregroundStyle(.tertiary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 12)
        }
        .chadexPadding(.horizontal, 10)
        .chadexPadding(.vertical, 8)
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

private struct AgentsDraftSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord
    @Binding var draft: String

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 5) {
                Text(L10n.string("instructions.createTitle"))
                    .chadexFont(.title2, weight: .semibold)
                Text(L10n.string("instructions.createMessage"))
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
                Text(project.path + "/AGENTS.md")
                    .chadexFont(.caption, design: .monospaced)
                    .foregroundStyle(.tertiary)
                    .textSelection(.enabled)
            }

            TextEditor(text: $draft)
                .font(.system(.body, design: .monospaced))
                .frame(minWidth: 620, minHeight: 330)
                .padding(7)
                .background(.quaternary.opacity(0.16), in: RoundedRectangle(cornerRadius: 9, style: .continuous))

            Label(L10n.string("instructions.noOverwrite"), systemImage: "lock.shield")
                .chadexFont(.caption)
                .foregroundStyle(.secondary)

            if let error = model.projectInstructionsError {
                Text(error)
                    .chadexFont(.caption)
                    .foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                Spacer()
                Button(L10n.string("common.cancel")) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button {
                    Task {
                        if await model.createProjectAgentsFile(content: draft) {
                            dismiss()
                        }
                    }
                } label: {
                    if model.projectAgentsWriteInFlight {
                        ProgressView().controlSize(.small)
                    } else {
                        Text(L10n.string("instructions.save"))
                    }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || model.projectAgentsWriteInFlight)
            }
        }
        .padding(22)
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
