import AppKit
import SwiftUI
import UniformTypeIdentifiers

/// The Skills page: external source cards on top, then a searchable, filterable, one-line-per-Skill list.
struct SkillsCenterView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord

    @State private var expandedSkillIDs: Set<String> = []
    @State private var showingSkillDraft = false
    @State private var showingSkillInstall = false
    @State private var query = ""
    @State private var scope: SkillListScope = .all
    @State private var removeTarget: SkillCenterItem?

    var body: some View {
        VStack(alignment: .leading, spacing: layout.spacing(ChadexMetrics.compactSectionSpacing)) {
            header

            ExternalSkillSourcesView(availability: availability) { showingSkillInstall = true }

            skillListSection

            Label(L10n.string("skills.authorityNote"), systemImage: "lock.shield")
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .task(id: project.id) {
            guard model.selectedProject?.id == project.id else { return }
            await model.refreshSkills()
        }
        .sheet(isPresented: $showingSkillDraft) {
            SkillDraftSheet(project: project)
                .environmentObject(model)
        }
        .sheet(isPresented: $showingSkillInstall) {
            SkillInstallSheet(project: project)
                .environmentObject(model)
        }
        .alert(item: $removeTarget) { item in
            Alert(
                title: Text(L10n.string("skills.remove.confirmTitle", item.name)),
                message: Text(L10n.string("skills.remove.confirmMessage")),
                primaryButton: .destructive(Text(L10n.string("skills.remove.confirmAction"))) {
                    Task { await model.removeManagedSkill(item) }
                },
                secondaryButton: .cancel()
            )
        }
    }

    private var availability: ExternalSkillAvailability {
        ExternalSkillAvailability.evaluate(
            discovery: model.externalSkillSources,
            roots: model.externalSkillRoots,
            loading: model.externalSkillsLoading
        )
    }

    // MARK: Header

    private var header: some View {
        ChadexPageHeader(
            title: L10n.string("skills.title"),
            subtitle: L10n.string("skills.subtitle")
        ) {
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

            // With no usable external source the import action moves into the sources block as the primary action.
            if availability != .none {
                Button(L10n.string("skills.install")) {
                    showingSkillInstall = true
                }
                .buttonStyle(.bordered)
                .controlSize(.small)
                .disabled(model.skillInstallInFlight || model.isSwitchingProject)
            }
        }
    }

    // MARK: List

    private var visibleItems: [SkillCenterItem] {
        SkillListFilter.filter(model.skillCenterItems, scope: scope, query: query)
    }

    @ViewBuilder
    private var skillListSection: some View {
        let items = model.skillCenterItems
        let visible = visibleItems

        VStack(alignment: .leading, spacing: 10) {
            SectionEyebrow(
                title: items.isEmpty
                    ? L10n.string("skills.list.title")
                    : L10n.string("skills.list.titleCount", visible.count, items.count)
            )

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
                    ChadexInlineError(message: error, font: .callout) {
                        await model.refreshSkills()
                    }
                }

                catalogWarnings

                if items.isEmpty {
                    Label(L10n.string("skills.none"), systemImage: "square.stack.3d.up.badge.a")
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .chadexPadding(.vertical, 6)
                } else {
                    filterBar

                    if visible.isEmpty {
                        Label(L10n.string("skills.noMatches"), systemImage: "magnifyingglass")
                            .chadexFont(.callout)
                            .foregroundStyle(.secondary)
                            .chadexPadding(.vertical, 6)
                    } else {
                        VStack(spacing: 0) {
                            ForEach(visible) { item in
                                skillRow(item)
                                if item.id != visible.last?.id {
                                    Divider().padding(.leading, layout.spacing(12))
                                }
                            }
                        }
                        .chadexGroupSurface()
                    }
                }
            }
        }
    }

    /// Search field plus a segmented scope bar; the scope bar wraps below the field in a narrow column.
    private var filterBar: some View {
        ViewThatFits(in: .horizontal) {
            HStack(spacing: layout.spacing(12)) {
                searchField.frame(minWidth: layout.control(160))
                scopePicker.fixedSize()
            }
            VStack(alignment: .leading, spacing: layout.spacing(8)) {
                searchField
                scopePicker
            }
        }
    }

    private var searchField: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
            TextField(L10n.string("skills.search.placeholder"), text: $query)
                .textFieldStyle(.plain)
                .accessibilityLabel(L10n.string("skills.search.label"))
            if !query.isEmpty {
                Button {
                    query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.secondary)
                }
                .buttonStyle(.plain)
                .help(L10n.string("skills.search.clear"))
                .accessibilityLabel(L10n.string("skills.search.clear"))
            }
        }
        .chadexFont(.callout)
        .chadexPadding(.horizontal, 8)
        .chadexPadding(.vertical, 5)
        .background(
            .quaternary.opacity(0.3),
            in: RoundedRectangle(cornerRadius: layout.control(7), style: .continuous)
        )
    }

    private var scopePicker: some View {
        Picker(L10n.string("skills.scope.label"), selection: $scope) {
            ForEach(SkillListScope.allCases) { scope in
                Text(L10n.string(scope.titleKey)).tag(scope)
            }
        }
        .pickerStyle(.segmented)
        .labelsHidden()
        .controlSize(.small)
    }

    /// Collapsible one-line warning; the longer explanation only shows when opened.
    @ViewBuilder
    private var catalogWarnings: some View {
        if let catalog = model.skillCatalog, catalog.invalidCount > 0 || catalog.discoveryTruncated {
            DisclosureGroup {
                Text(
                    catalog.discoveryTruncated
                        ? L10n.string("skills.discoveryTruncated", catalog.invalidCount)
                        : L10n.string("skills.invalidCount", catalog.invalidCount)
                )
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .chadexPadding(.top, 2)
            } label: {
                Label(
                    catalog.invalidCount > 0
                        ? L10n.string("skills.invalidSummary", catalog.invalidCount)
                        : L10n.string("skills.truncatedSummary"),
                    systemImage: "exclamationmark.triangle"
                )
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
            }
        }
    }

    private func toggleExpanded(_ item: SkillCenterItem) {
        if expandedSkillIDs.contains(item.skillId) {
            expandedSkillIDs.remove(item.skillId)
        } else {
            expandedSkillIDs.insert(item.skillId)
            Task { await model.loadSkillDefinition(item) }
        }
    }

    private func skillRow(_ item: SkillCenterItem) -> some View {
        let expanded = expandedSkillIDs.contains(item.skillId)
        return VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Button {
                    toggleExpanded(item)
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: "chevron.right")
                            .chadexFont(.caption2, weight: .semibold)
                            .foregroundStyle(.tertiary)
                            .rotationEffect(.degrees(expanded ? 90 : 0))
                            .frame(width: layout.control(10))
                            .accessibilityHidden(true)

                        Text(item.name)
                            .chadexFont(.callout, weight: .medium)
                            .lineLimit(1)
                            .layoutPriority(2)

                        skillBadges(item)
                            .layoutPriority(1)

                        if !expanded {
                            Text(item.description)
                                .chadexFont(.caption)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.tail)
                        }

                        Spacer(minLength: 0)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel(item.name)
                .accessibilityValue(expanded ? L10n.string("skills.row.expanded") : L10n.string("skills.row.collapsed"))
                .accessibilityHint(expanded ? L10n.string("skills.row.collapseHint") : L10n.string("skills.row.expandHint"))

                // Only Skills Chadex stored can be toggled or removed; project and external Skills are not Chadex's files.
                if let managed = item.managed {
                    let inFlight = model.skillMutationInFlightIDs.contains(item.skillId)
                    Toggle(L10n.string("skills.enable"), isOn: Binding(
                        get: { item.isActive },
                        set: { newValue in
                            Task { await model.setManagedSkillEnabled(item, enabled: newValue) }
                        }
                    ))
                    .toggleStyle(.switch)
                    .controlSize(.small)
                    .labelsHidden()
                    .disabled(inFlight)
                    .help(L10n.string("skills.managedToggleHelp", managed.skillKey))
                    .accessibilityLabel(L10n.string("skills.enable"))

                    Button(role: .destructive) {
                        removeTarget = item
                    } label: {
                        Label(L10n.string("skills.remove"), systemImage: "trash")
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .tint(.red)
                    .disabled(inFlight)
                    .help(L10n.string("skills.removeHelp", managed.skillKey))
                }
            }

            if expanded {
                VStack(alignment: .leading, spacing: 10) {
                    Text(item.description)
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    skillDetails(item)
                }
                .chadexPadding(.leading, 20)
                .chadexPadding(.top, 8)
                .chadexPadding(.bottom, 4)
            }
        }
        .chadexPadding(.horizontal, 10)
        .chadexPadding(.vertical, 7)
    }

    private func skillBadges(_ item: SkillCenterItem) -> some View {
        HStack(spacing: 5) {
            badge(item.sourceDisplayLabel)
            if !item.isActive {
                badge(L10n.string("skills.disabled"))
            }
            if item.nameConflict {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(.orange)
                    .help(L10n.string("skills.nameConflict"))
                    .accessibilityLabel(L10n.string("skills.nameConflict"))
            }
            if item.trust == "operator_configured_guidance", item.scriptsAllowed == false {
                badge(L10n.string("skills.external.scriptsOff"))
                    .help(L10n.string("skills.external.scriptsOffHelp"))
            }
        }
        .fixedSize()
    }

    @ViewBuilder
    private func skillDetails(_ item: SkillCenterItem) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 5) {
                detailRow(L10n.string("skills.source"), item.sourceDisplayLabel)
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
                    .chadexGroupSurface(inset: true)
            }

            Text(executionNote(item))
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
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
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
        }
    }

    private func badge(_ text: String) -> some View {
        Text(text)
            .chadexFont(.caption2, weight: .medium)
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(.quaternary.opacity(0.35), in: Capsule())
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
                .foregroundStyle(.secondary)
                .textSelection(.enabled)

            TextEditor(text: $instructions)
                .font(.system(.body, design: .monospaced))
                .frame(minWidth: 640, minHeight: 310)
                .padding(7)
                .chadexEditorSurface()

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
    @State private var archiveURL: URL?
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
                Text(archiveURL.map { ($0.path as NSString).abbreviatingWithTildeInPath } ?? L10n.string("skills.noArchive"))
                    .chadexFont(.callout, design: .monospaced)
                    .foregroundStyle(.secondary)
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
                        guard let archiveURL else { return }
                        localError = nil
                        if await model.installSkill(skillKey: skillKey, archiveURL: archiveURL) {
                            dismiss()
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(!validKey || archiveURL == nil || model.skillInstallInFlight)
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
        panel.directoryURL = FileManager.default.urls(for: .downloadsDirectory, in: .userDomainMask).first
        panel.prompt = L10n.string("skills.chooseArchive")
        guard panel.runModal() == .OK, let url = panel.url else { return }

        do {
            try SkillArchiveStaging.preflight(url)
            archiveURL = url
            localError = nil
        } catch {
            archiveURL = nil
            localError = error.localizedDescription
        }
    }
}
