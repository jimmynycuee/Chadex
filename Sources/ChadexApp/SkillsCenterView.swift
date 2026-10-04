import AppKit
import SwiftUI
import UniformTypeIdentifiers

struct SkillsCenterView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord

    @State private var expandedSkillIDs: Set<String> = []
    @State private var showingSkillDraft = false
    @State private var showingSkillInstall = false

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header

            Text(L10n.string("skills.subtitle"))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if model.skillsLoading && model.skillCatalog == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(L10n.string("skills.loading"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                }
                .chadexPadding(.vertical, 8)
            } else {
                if let error = model.skillsError {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                catalogWarnings

                if model.skillCenterItems.isEmpty {
                    Label(L10n.string("skills.none"), systemImage: "square.stack.3d.up.badge.a")
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .chadexPadding(.vertical, 6)
                } else {
                    VStack(spacing: 0) {
                        ForEach(model.skillCenterItems) { item in
                            skillRow(item)
                            if item.id != model.skillCenterItems.last?.id {
                                Divider().padding(.leading, layout.spacing(12))
                            }
                        }
                    }
                    .background(
                        .quaternary.opacity(0.16),
                        in: RoundedRectangle(cornerRadius: layout.control(10), style: .continuous)
                    )
                }
            }

            Label(L10n.string("skills.authorityNote"), systemImage: "lock.shield")
                .chadexFont(.caption)
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .sheet(isPresented: $showingSkillDraft) {
            SkillDraftSheet(project: project)
                .environmentObject(model)
        }
        .sheet(isPresented: $showingSkillInstall) {
            SkillInstallSheet(project: project)
                .environmentObject(model)
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            SectionEyebrow(title: L10n.string("skills.title"))
            Spacer(minLength: 12)

            Button {
                Task { await model.refreshSkills() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .disabled(model.skillsLoading || model.isSwitchingProject)
            .help(L10n.string("skills.refresh"))
            .accessibilityLabel(L10n.string("skills.refresh"))

            Button(L10n.string("skills.create")) {
                showingSkillDraft = true
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(model.projectSkillWriteInFlight || model.isSwitchingProject)

            Button(L10n.string("skills.install")) {
                showingSkillInstall = true
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(model.skillInstallInFlight || model.isSwitchingProject)
        }
    }

    @ViewBuilder
    private var catalogWarnings: some View {
        if let catalog = model.skillCatalog, catalog.invalidCount > 0 || catalog.discoveryTruncated {
            Label(
                catalog.discoveryTruncated
                    ? L10n.string("skills.discoveryTruncated", catalog.invalidCount)
                    : L10n.string("skills.invalidCount", catalog.invalidCount),
                systemImage: "exclamationmark.triangle"
            )
            .chadexFont(.caption)
            .foregroundStyle(.secondary)
        }
    }

    private func skillRow(_ item: SkillCenterItem) -> some View {
        DisclosureGroup(
            isExpanded: Binding(
                get: { expandedSkillIDs.contains(item.skillId) },
                set: { expanded in
                    if expanded {
                        expandedSkillIDs.insert(item.skillId)
                        Task { await model.loadSkillDefinition(item) }
                    } else {
                        expandedSkillIDs.remove(item.skillId)
                    }
                }
            )
        ) {
            skillDetails(item)
                .chadexPadding(.leading, 22)
                .chadexPadding(.top, 8)
                .chadexPadding(.bottom, 4)
        } label: {
            HStack(alignment: .top, spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 7) {
                        Text(item.name)
                            .chadexFont(.callout, weight: .medium)
                        badge(sourceLabel(item))
                        badge(item.isActive ? L10n.string("skills.enabled") : L10n.string("skills.disabled"))
                        if item.nameConflict {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.orange)
                                .help(L10n.string("skills.nameConflict"))
                        }
                    }
                    Text(item.description)
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Text(String(item.definitionRevision.prefix(12)))
                        .chadexFont(.caption2, design: .monospaced)
                        .foregroundStyle(.tertiary)
                        .help(item.definitionRevision)
                }

                Spacer(minLength: 10)

                if let managed = item.managed {
                    Button(item.isActive ? L10n.string("skills.disable") : L10n.string("skills.enable")) {
                        Task { await model.setManagedSkillEnabled(item, enabled: !item.isActive) }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(model.skillMutationInFlightIDs.contains(item.skillId))
                    .help(L10n.string("skills.managedToggleHelp", managed.skillKey))
                }
            }
            .contentShape(Rectangle())
        }
        .chadexPadding(.horizontal, 10)
        .chadexPadding(.vertical, 8)
    }

    @ViewBuilder
    private func skillDetails(_ item: SkillCenterItem) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 5) {
                detailRow(L10n.string("skills.source"), sourceLabel(item))
                detailRow(L10n.string("skills.trust"), trustLabel(item.trust))
                detailRow(L10n.string("skills.definitionRevision"), item.definitionRevision)
                if let packageRevision = item.packageRevision {
                    detailRow(L10n.string("skills.packageRevision"), packageRevision)
                }
                if let managed = item.managed {
                    detailRow(L10n.string("skills.versions"), "\(managed.totalVersions)")
                }
            }

            if !item.isActive {
                Label(L10n.string("skills.disabledDefinition"), systemImage: "pause.circle")
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
            } else if model.skillDefinitionLoadingIDs.contains(item.skillId) {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(L10n.string("skills.loadingDefinition"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                }
            } else if let preview = model.skillDefinitions[item.skillId] {
                Text(preview.text)
                    .chadexFont(.caption, design: .monospaced)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .chadexPadding(10)
                    .background(
                        .quaternary.opacity(0.14),
                        in: RoundedRectangle(cornerRadius: layout.control(8), style: .continuous)
                    )
            }

            Text(executionNote(item))
                .chadexFont(.caption)
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func detailRow(_ label: String, _ value: String) -> some View {
        GridRow {
            Text(label)
                .chadexFont(.caption, weight: .semibold)
                .foregroundStyle(.secondary)
            Text(value)
                .chadexFont(.caption, design: .monospaced)
                .foregroundStyle(.tertiary)
                .textSelection(.enabled)
        }
    }

    private func badge(_ text: String) -> some View {
        Text(text)
            .chadexFont(.caption2, weight: .medium)
            .foregroundStyle(.secondary)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(.quaternary.opacity(0.35), in: Capsule())
    }

    private func sourceLabel(_ item: SkillCenterItem) -> String {
        switch item.trust {
        case "project_content": return L10n.string("skills.projectSource")
        case "operator_configured_guidance": return L10n.string("skills.configuredSource")
        case "operator_installed_guidance": return L10n.string("skills.installedSource")
        default: return item.sourceScope
        }
    }

    private func trustLabel(_ trust: String) -> String {
        switch trust {
        case "project_content": return L10n.string("skills.trustProject")
        case "operator_configured_guidance": return L10n.string("skills.trustConfigured")
        case "operator_installed_guidance": return L10n.string("skills.trustInstalled")
        default: return trust
        }
    }

    private func executionNote(_ item: SkillCenterItem) -> String {
        item.trust == "project_content"
            ? L10n.string("skills.projectExecutionNote")
            : L10n.string("skills.managedExecutionNote")
    }
}

private struct SkillDraftSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord

    @State private var skillKey = ""
    @State private var skillDescription = ""
    @State private var instructions = AppModel.skillDraftTemplate

    private var validKey: Bool {
        !skillKey.isEmpty
            && skillKey.count <= 96
            && skillKey != "."
            && skillKey != ".."
            && skillKey.utf8.allSatisfy { byte in
                (byte >= 48 && byte <= 57)
                    || (byte >= 65 && byte <= 90)
                    || (byte >= 97 && byte <= 122)
                    || byte == 46 || byte == 95 || byte == 45
            }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 5) {
                Text(L10n.string("skills.createTitle"))
                    .chadexFont(.title2, weight: .semibold)
                Text(L10n.string("skills.createMessage"))
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
            }

            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 10) {
                GridRow {
                    Text(L10n.string("skills.key"))
                    TextField("my-skill", text: $skillKey)
                        .textFieldStyle(.roundedBorder)
                }
                GridRow {
                    Text(L10n.string("skills.description"))
                    TextField(L10n.string("skills.descriptionPlaceholder"), text: $skillDescription)
                        .textFieldStyle(.roundedBorder)
                }
            }

            Text(project.path + "/.agents/skills/" + (skillKey.isEmpty ? "<skill>" : skillKey) + "/SKILL.md")
                .chadexFont(.caption, design: .monospaced)
                .foregroundStyle(.tertiary)
                .textSelection(.enabled)

            TextEditor(text: $instructions)
                .font(.system(.body, design: .monospaced))
                .frame(minWidth: 640, minHeight: 310)
                .padding(7)
                .background(.quaternary.opacity(0.16), in: RoundedRectangle(cornerRadius: 9, style: .continuous))

            Label(L10n.string("skills.noOverwrite"), systemImage: "lock.shield")
                .chadexFont(.caption)
                .foregroundStyle(.secondary)

            if let error = model.skillsError {
                Text(error)
                    .chadexFont(.caption)
                    .foregroundStyle(.red)
            }

            HStack {
                Spacer()
                Button(L10n.string("common.cancel")) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(L10n.string("skills.createSave")) {
                    Task {
                        if await model.createProjectSkill(
                            skillKey: skillKey,
                            description: skillDescription,
                            instructions: instructions
                        ) {
                            dismiss()
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(
                    !validKey
                        || skillDescription.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || instructions.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || model.projectSkillWriteInFlight
                )
            }
        }
        .padding(22)
    }
}

private struct SkillInstallSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord

    @State private var skillKey = ""
    @State private var artifactPath = ""
    @State private var localError: String?

    private var validKey: Bool {
        !skillKey.isEmpty
            && skillKey.count <= 96
            && skillKey != "."
            && skillKey != ".."
            && skillKey.utf8.allSatisfy { byte in
                (byte >= 48 && byte <= 57)
                    || (byte >= 65 && byte <= 90)
                    || (byte >= 97 && byte <= 122)
                    || byte == 46 || byte == 95 || byte == 45
            }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 5) {
                Text(L10n.string("skills.installTitle"))
                    .chadexFont(.title2, weight: .semibold)
                Text(L10n.string("skills.installMessage"))
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
            }

            TextField(L10n.string("skills.key"), text: $skillKey)
                .textFieldStyle(.roundedBorder)

            HStack(spacing: 10) {
                Text(artifactPath.isEmpty ? L10n.string("skills.noArchive") : artifactPath)
                    .chadexFont(.callout, design: .monospaced)
                    .foregroundStyle(artifactPath.isEmpty ? .tertiary : .secondary)
                    .lineLimit(2)
                    .textSelection(.enabled)
                Spacer(minLength: 12)
                Button(L10n.string("skills.chooseArchive"), action: chooseArchive)
                    .buttonStyle(.bordered)
            }

            Label(L10n.string("skills.archiveBoundary"), systemImage: "folder.badge.questionmark")
                .chadexFont(.caption)
                .foregroundStyle(.secondary)

            if let error = localError ?? model.skillsError {
                Text(error)
                    .chadexFont(.caption)
                    .foregroundStyle(.red)
            }

            HStack {
                Spacer()
                Button(L10n.string("common.cancel")) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(L10n.string("skills.installEnable")) {
                    Task {
                        if await model.installSkill(skillKey: skillKey, artifactPath: artifactPath) {
                            dismiss()
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(!validKey || artifactPath.isEmpty || model.skillInstallInFlight)
            }
        }
        .frame(width: 600)
        .padding(22)
    }

    private func chooseArchive() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        panel.allowedContentTypes = [.zip]
        panel.directoryURL = URL(fileURLWithPath: project.path, isDirectory: true)
        panel.prompt = L10n.string("skills.chooseArchive")
        guard panel.runModal() == .OK, let url = panel.url else { return }

        let root = URL(fileURLWithPath: project.path, isDirectory: true).standardizedFileURL.path
        let chosen = url.standardizedFileURL.path
        let prefix = root.hasSuffix("/") ? root : root + "/"
        guard chosen.hasPrefix(prefix) else {
            artifactPath = ""
            localError = L10n.string("skills.archiveOutsideProject")
            return
        }
        artifactPath = String(chosen.dropFirst(prefix.count))
        localError = nil
    }
}
