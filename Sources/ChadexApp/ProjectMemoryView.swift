import SwiftUI

struct ProjectMemoryView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    let project: ProjectRecord

    @State private var expandedMemoryKeys: Set<String> = []
    @State private var editorTarget: MemoryEditorTarget?
    @State private var deleteTarget: ProjectMemoryDescriptor?

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header

            Text(L10n.string("memory.subtitle"))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            if model.projectMemoryLoading && model.projectMemoryCatalog == nil {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(L10n.string("memory.loading"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                }
                .chadexPadding(.vertical, 8)
            } else {
                if let error = model.projectMemoryError {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                if let catalog = model.projectMemoryCatalog, catalog.memories.isEmpty {
                    Label(L10n.string("memory.none"), systemImage: "memorychip")
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .chadexPadding(.vertical, 6)
                } else {
                    memoryGroups
                }
            }

            VStack(alignment: .leading, spacing: 5) {
                Label(L10n.string("memory.authorityNote"), systemImage: "lock.shield")
                Label(L10n.string("memory.agentApprovalNote"), systemImage: "person.badge.shield.checkmark")
                Label(L10n.string("memory.secretNote"), systemImage: "key.slash")
            }
            .chadexFont(.caption)
            .foregroundStyle(.tertiary)
            .fixedSize(horizontal: false, vertical: true)
        }
        .sheet(item: $editorTarget) { target in
            ProjectMemoryEditorSheet(project: project, record: target.record)
                .environmentObject(model)
        }
        .alert(item: $deleteTarget) { descriptor in
            Alert(
                title: Text(L10n.string("memory.deleteTitle")),
                message: Text(L10n.string("memory.deleteMessage", descriptor.memoryKey)),
                primaryButton: .destructive(Text(L10n.string("memory.delete"))) {
                    Task { await model.deleteProjectMemory(descriptor) }
                },
                secondaryButton: .cancel()
            )
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            SectionEyebrow(title: L10n.string("memory.title"))
            Spacer(minLength: 12)

            if let catalog = model.projectMemoryCatalog {
                Text(L10n.string("memory.count", catalog.totalCount))
                    .chadexFont(.caption)
                    .foregroundStyle(.tertiary)
            }

            Button {
                Task { await model.refreshProjectMemory() }
            } label: {
                Image(systemName: "arrow.clockwise")
            }
            .buttonStyle(.borderless)
            .disabled(model.projectMemoryLoading || model.isSwitchingProject)
            .help(L10n.string("memory.refresh"))
            .accessibilityLabel(L10n.string("memory.refresh"))

            Button(L10n.string("memory.add")) {
                editorTarget = MemoryEditorTarget(record: nil)
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
            .disabled(model.isSwitchingProject)
        }
    }

    @ViewBuilder
    private var memoryGroups: some View {
        VStack(alignment: .leading, spacing: 14) {
            ForEach(ProjectMemoryCategory.allCases) { category in
                let memories = descriptors(in: category)
                if !memories.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        Text(categoryLabel(category))
                            .chadexFont(.caption, weight: .semibold)
                            .foregroundStyle(.secondary)

                        VStack(spacing: 0) {
                            ForEach(memories) { descriptor in
                                memoryRow(descriptor)
                                if descriptor.id != memories.last?.id {
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
            }
        }
    }

    private func descriptors(in category: ProjectMemoryCategory) -> [ProjectMemoryDescriptor] {
        (model.projectMemoryCatalog?.memories ?? [])
            .filter { $0.category == category }
            .sorted { lhs, rhs in
                let lhsRank = priorityRank(lhs.priority)
                let rhsRank = priorityRank(rhs.priority)
                if lhsRank != rhsRank { return lhsRank < rhsRank }
                return lhs.memoryKey.localizedCaseInsensitiveCompare(rhs.memoryKey) == .orderedAscending
            }
    }

    private func memoryRow(_ descriptor: ProjectMemoryDescriptor) -> some View {
        DisclosureGroup(
            isExpanded: Binding(
                get: { expandedMemoryKeys.contains(descriptor.memoryKey) },
                set: { expanded in
                    if expanded {
                        expandedMemoryKeys.insert(descriptor.memoryKey)
                        Task { await model.loadProjectMemory(descriptor) }
                    } else {
                        expandedMemoryKeys.remove(descriptor.memoryKey)
                    }
                }
            )
        ) {
            memoryDetails(descriptor)
                .chadexPadding(.leading, 22)
                .chadexPadding(.top, 8)
                .chadexPadding(.bottom, 4)
        } label: {
            HStack(alignment: .top, spacing: 10) {
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 7) {
                        Text(descriptor.memoryKey)
                            .chadexFont(.callout, weight: .medium, design: .monospaced)
                        badge(priorityLabel(descriptor.priority))
                        if descriptor.bootstrap {
                            badge(L10n.string("memory.bootstrapBadge"))
                        }
                    }

                    Text(descriptor.summary)
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)

                    if !descriptor.tags.isEmpty {
                        Text(descriptor.tags.map { "#\($0)" }.joined(separator: "  "))
                            .chadexFont(.caption2)
                            .foregroundStyle(.tertiary)
                            .lineLimit(2)
                    }
                }

                Spacer(minLength: 10)

                if model.projectMemoryMutationInFlightKeys.contains(descriptor.memoryKey) {
                    ProgressView().controlSize(.small)
                }
            }
            .contentShape(Rectangle())
        }
        .chadexPadding(.horizontal, 10)
        .chadexPadding(.vertical, 8)
    }

    @ViewBuilder
    private func memoryDetails(_ descriptor: ProjectMemoryDescriptor) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 5) {
                detailRow(L10n.string("memory.revision"), descriptor.revision)
                detailRow(L10n.string("memory.priority"), priorityLabel(descriptor.priority))
                detailRow(
                    L10n.string("memory.bootstrap"),
                    descriptor.bootstrap ? L10n.string("memory.yes") : L10n.string("memory.no")
                )
            }

            if model.projectMemoryReadLoadingKeys.contains(descriptor.memoryKey) {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text(L10n.string("memory.loadingBody"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                }
            } else if let record = model.projectMemoryRecords[descriptor.memoryKey],
                      record.revision == descriptor.revision {
                if record.body.isEmpty {
                    Text(L10n.string("memory.emptyBody"))
                        .chadexFont(.caption)
                        .foregroundStyle(.tertiary)
                } else {
                    Text(record.body)
                        .chadexFont(.caption)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .chadexPadding(10)
                        .background(
                            .quaternary.opacity(0.14),
                            in: RoundedRectangle(cornerRadius: layout.control(8), style: .continuous)
                        )
                }

                Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 5) {
                    detailRow(L10n.string("memory.createdBy"), record.provenance.createdByKind)
                    detailRow(L10n.string("memory.updatedBy"), record.provenance.updatedByKind)
                    detailRow(L10n.string("memory.createdAt"), formattedDate(record.createdAtUnixMs))
                    detailRow(L10n.string("memory.updatedAt"), formattedDate(record.updatedAtUnixMs))
                }

                HStack(spacing: 8) {
                    Button(L10n.string("memory.edit")) {
                        editorTarget = MemoryEditorTarget(record: record)
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(model.projectMemoryMutationInFlightKeys.contains(descriptor.memoryKey))

                    Button(L10n.string("memory.delete"), role: .destructive) {
                        deleteTarget = descriptor
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(model.projectMemoryMutationInFlightKeys.contains(descriptor.memoryKey))
                }
            }

            Text(L10n.string("memory.lazyNote"))
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
                .chadexFont(.caption, design: label == L10n.string("memory.revision") ? .monospaced : .default)
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

    private func categoryLabel(_ category: ProjectMemoryCategory) -> String {
        L10n.string("memory.category.\(category.rawValue)")
    }

    private func priorityLabel(_ priority: String) -> String {
        L10n.string("memory.priority.\(priority)")
    }

    private func priorityRank(_ priority: String) -> Int {
        switch priority {
        case "high": return 0
        case "normal": return 1
        default: return 2
        }
    }

    private func formattedDate(_ unixMs: Int64) -> String {
        Date(timeIntervalSince1970: Double(unixMs) / 1_000)
            .formatted(date: .abbreviated, time: .shortened)
    }
}

private struct MemoryEditorTarget: Identifiable {
    let id = UUID()
    let record: ProjectMemoryRecord?
}

private struct ProjectMemoryEditorSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var model: AppModel

    let project: ProjectRecord
    let record: ProjectMemoryRecord?

    @State private var memoryKey: String
    @State private var summary: String
    @State private var bodyText: String
    @State private var priority: String
    @State private var bootstrap: Bool
    @State private var category: ProjectMemoryCategory
    @State private var tagsText: String

    init(project: ProjectRecord, record: ProjectMemoryRecord?) {
        self.project = project
        self.record = record
        _memoryKey = State(initialValue: record?.memoryKey ?? "")
        _summary = State(initialValue: record?.summary ?? "")
        _bodyText = State(initialValue: record?.body ?? "")
        _priority = State(initialValue: record?.priority ?? "normal")
        _bootstrap = State(initialValue: record?.bootstrap ?? false)

        let initialCategory = ProjectMemoryEditorSheet.category(for: record?.tags ?? [])
        _category = State(initialValue: initialCategory)
        let nonCategoryTags = (record?.tags ?? []).filter {
            !ProjectMemoryEditorSheet.categoryTags.contains($0.lowercased())
        }
        _tagsText = State(initialValue: nonCategoryTags.joined(separator: ", "))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(record == nil ? L10n.string("memory.editor.createTitle") : L10n.string("memory.editor.editTitle"))
                .chadexFont(.title3, weight: .semibold)

            Text(record == nil ? L10n.string("memory.editor.createMessage") : L10n.string("memory.editor.editMessage"))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            Form {
                TextField(L10n.string("memory.key"), text: $memoryKey)
                    .disabled(record != nil)

                Picker(L10n.string("memory.category"), selection: $category) {
                    ForEach(ProjectMemoryCategory.allCases) { category in
                        Text(L10n.string("memory.category.\(category.rawValue)")).tag(category)
                    }
                }

                TextField(L10n.string("memory.summary"), text: $summary)

                Picker(L10n.string("memory.priority"), selection: $priority) {
                    Text(L10n.string("memory.priority.high")).tag("high")
                    Text(L10n.string("memory.priority.normal")).tag("normal")
                    Text(L10n.string("memory.priority.low")).tag("low")
                }

                TextField(L10n.string("memory.tags"), text: $tagsText)

                Toggle(L10n.string("memory.bootstrap"), isOn: $bootstrap)
            }
            .formStyle(.grouped)

            VStack(alignment: .leading, spacing: 6) {
                Text(L10n.string("memory.body"))
                    .chadexFont(.caption, weight: .semibold)
                    .foregroundStyle(.secondary)
                TextEditor(text: $bodyText)
                    .font(.body)
                    .frame(minHeight: 150)
                    .padding(6)
                    .background(.quaternary.opacity(0.14), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            }

            if let validationMessage {
                Label(validationMessage, systemImage: "exclamationmark.triangle")
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
            }

            Text(L10n.string("memory.editor.boundaryNote"))
                .chadexFont(.caption)
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)

            HStack {
                Spacer()
                Button(L10n.string("common.cancel")) { dismiss() }
                    .keyboardShortcut(.cancelAction)

                Button(record == nil ? L10n.string("memory.addSave") : L10n.string("memory.editSave")) {
                    Task {
                        let saved = await model.saveProjectMemory(
                            memoryKey: memoryKey,
                            summary: summary,
                            body: bodyText,
                            priority: priority,
                            bootstrap: bootstrap,
                            tags: effectiveTags,
                            expectedRevision: record?.revision
                        )
                        if saved { dismiss() }
                    }
                }
                .keyboardShortcut(.defaultAction)
                .disabled(!canSave || model.projectMemoryMutationInFlightKeys.contains(trimmedKey))
            }
        }
        .padding(22)
        .frame(width: 590)
    }

    private var trimmedKey: String {
        memoryKey.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var parsedTags: [String] {
        var seen = Set<String>()
        return tagsText
            .split(separator: ",", omittingEmptySubsequences: true)
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
            .filter { seen.insert($0.lowercased()).inserted }
    }

    private var effectiveTags: [String] {
        var tags = parsedTags.filter { !Self.categoryTags.contains($0.lowercased()) }
        if let categoryTag = Self.tag(for: category) {
            tags.append(categoryTag)
        }
        return tags
    }

    private var canSave: Bool {
        validationMessage == nil
    }

    private var validationMessage: String? {
        guard Self.validMemoryKey(trimmedKey) else { return L10n.string("memory.validation.key") }
        let trimmedSummary = summary.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmedSummary.isEmpty, trimmedSummary.count <= 512 else {
            return L10n.string("memory.validation.summary")
        }
        guard bodyText.lengthOfBytes(using: .utf8) <= 8_192 else {
            return L10n.string("memory.validation.body")
        }
        guard effectiveTags.count <= 8, effectiveTags.allSatisfy({ !$0.isEmpty && $0.count <= 64 }) else {
            return L10n.string("memory.validation.tags")
        }
        return nil
    }

    private static let categoryTags: Set<String> = [
        "architecture", "architectural", "decision", "decisions", "adr", "workflow", "process", "procedure"
    ]

    private static func category(for tags: [String]) -> ProjectMemoryCategory {
        let normalized = Set(tags.map { $0.lowercased() })
        if !normalized.isDisjoint(with: ["architecture", "architectural"]) { return .architecture }
        if !normalized.isDisjoint(with: ["decision", "decisions", "adr"]) { return .decisions }
        if !normalized.isDisjoint(with: ["workflow", "process", "procedure"]) { return .workflow }
        return .other
    }

    private static func tag(for category: ProjectMemoryCategory) -> String? {
        switch category {
        case .architecture: return "architecture"
        case .decisions: return "decision"
        case .workflow: return "workflow"
        case .other: return nil
        }
    }

    private static func validMemoryKey(_ key: String) -> Bool {
        guard !key.isEmpty, key.utf8.count <= 96, key != ".", key != ".." else { return false }
        return key.utf8.allSatisfy { byte in
            (byte >= 48 && byte <= 57)
                || (byte >= 65 && byte <= 90)
                || (byte >= 97 && byte <= 122)
                || byte == 46
                || byte == 95
                || byte == 45
        }
    }
}
