import AppKit
import SwiftUI

/// Connects Skill folders that already exist for Codex, Claude Code, or shared agents.
struct ExternalSkillSourcesView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel

    var availability: ExternalSkillAvailability = .unknown
    var onImport: () -> Void = {}

    @State private var expandedInvalid: Set<String> = []

    private var roots: [String] { model.externalSkillRoots?.roots ?? [] }
    private var scriptRoots: Set<String> { Set(model.externalSkillRoots?.scriptRoots ?? []) }
    private var busy: Bool { model.externalSkillsApplying || model.externalSkillsLoading }
    private var canEdit: Bool { model.externalSkillRoots != nil && !busy }

    private var sources: [ExternalSkillSource] { model.externalSkillSources?.sources ?? [] }

    private var discoveredCanonicalPaths: Set<String> {
        Set(sources.compactMap { isConnectable($0) ? $0.canonicalPath : nil })
    }

    /// Roots that are configured or recommended but not shown as a discovered source row.
    private var extraRoots: [(path: String, configured: Bool)] {
        var seen = discoveredCanonicalPaths
        var result: [(String, Bool)] = []
        for path in roots where seen.insert(path).inserted {
            result.append((path, true))
        }
        for path in model.externalSkillSources?.recommendedRoots ?? [] where seen.insert(path).inserted {
            result.append((path, false))
        }
        return result
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                SectionEyebrow(title: L10n.string("skills.external.title"))
                Spacer(minLength: 12)
                if busy { ProgressView().controlSize(.small) }
                Button(L10n.string("skills.external.chooseFolder")) { chooseFolder() }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(!canEdit)
            }

            Text(L10n.string("skills.external.subtitle"))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            Label(L10n.string("skills.external.uploadVsConnect"), systemImage: "arrow.triangle.branch")
                .chadexFont(.caption)
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)

            Label(L10n.string("skills.external.scriptsNote"), systemImage: "terminal")
                .chadexFont(.caption)
                .foregroundStyle(.tertiary)
                .fixedSize(horizontal: false, vertical: true)

            if let error = model.externalSkillsError {
                Label(error, systemImage: "exclamationmark.triangle")
                    .chadexFont(.callout)
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("skills.external.error")
            }

            if availability == .none {
                VStack(alignment: .leading, spacing: 8) {
                    Text(L10n.string("skills.external.noneUsable"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Button(L10n.string("skills.external.import")) { onImport() }
                        .buttonStyle(.borderedProminent)
                        .controlSize(.regular)
                        .disabled(model.skillInstallInFlight || model.isSwitchingProject)
                        .accessibilityIdentifier("skills.external.import")
                }
            }

            let extras = extraRoots
            if sources.isEmpty && extras.isEmpty {
                if !model.externalSkillsLoading && availability != .none {
                    Text(L10n.string("skills.external.none"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                }
            } else {
                VStack(spacing: 0) {
                    ForEach(Array(sources.enumerated()), id: \.element.id) { index, source in
                        sourceRow(source)
                        if index < sources.count - 1 || !extras.isEmpty {
                            Divider().padding(.leading, layout.spacing(12))
                        }
                    }
                    ForEach(Array(extras.enumerated()), id: \.element.path) { index, extra in
                        extraRow(path: extra.path, configured: extra.configured)
                        if index < extras.count - 1 {
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
        .task { await model.refreshExternalSkillSources() }
    }

    // MARK: Rows

    private func isConnectable(_ source: ExternalSkillSource) -> Bool {
        ExternalSkillAvailability.isConnectable(source)
    }

    private func isProvidedElsewhere(_ source: ExternalSkillSource) -> Bool {
        ExternalSkillAvailability.isProvidedElsewhere(source)
    }

    private func kindLabel(_ kind: String) -> String {
        switch kind {
        case "agents": return L10n.string("skills.external.kind.agents")
        case "claude": return L10n.string("skills.external.kind.claude")
        case "codex": return L10n.string("skills.external.kind.codex")
        default: return kind
        }
    }

    static func displayPath(_ path: String) -> String {
        (path as NSString).abbreviatingWithTildeInPath
    }

    private func providedByPaths(_ source: ExternalSkillSource) -> [String] {
        if !source.providedBy.isEmpty { return source.providedBy.map(Self.displayPath) }
        if let same = source.sameAs,
           let target = sources.first(where: { $0.kind == same }) {
            return [Self.displayPath(target.path)]
        }
        return []
    }

    private func countsText(_ source: ExternalSkillSource) -> String {
        var text = L10n.string("skills.external.counts", source.validCount, source.scriptCount)
        if source.invalidCount > 0 {
            text += " · " + L10n.string("skills.external.invalid", source.invalidCount)
        }
        return text
    }

    private func sourceRow(_ source: ExternalSkillSource) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(kindLabel(source.kind))
                    .chadexFont(.callout, weight: .medium)
                Text(Self.displayPath(source.path))
                    .chadexFont(.caption, design: .monospaced)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(source.path)
                Spacer(minLength: 8)
            }

            if isProvidedElsewhere(source) {
                Label(
                    L10n.string("skills.external.providedBy", providedByPaths(source).joined(separator: ", ")),
                    systemImage: "link"
                )
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
            } else if source.status == "available", let canonical = source.canonicalPath {
                Text(countsText(source))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                if source.truncated {
                    Label(L10n.string("skills.external.truncated"), systemImage: "exclamationmark.triangle")
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                }
                toggles(for: canonical)
                invalidDisclosure(source)
            } else {
                Text(statusText(source.status))
                    .chadexFont(.caption)
                    .foregroundStyle(.tertiary)
            }
        }
        .chadexPadding(.horizontal, 10)
        .chadexPadding(.vertical, 8)
        .accessibilityElement(children: .contain)
    }

    private func extraRow(path: String, configured: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Text(L10n.string(configured ? "skills.external.custom" : "skills.external.linked"))
                    .chadexFont(.callout, weight: .medium)
                Text(Self.displayPath(path))
                    .chadexFont(.caption, design: .monospaced)
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .help(path)
                Spacer(minLength: 8)
                if configured {
                    Button(L10n.string("skills.external.remove")) { setConnected(path, false) }
                        .buttonStyle(.borderless)
                        .controlSize(.small)
                        .disabled(!canEdit)
                        .accessibilityLabel(L10n.string("skills.external.removeLabel", Self.displayPath(path)))
                }
            }
            toggles(for: path)
        }
        .chadexPadding(.horizontal, 10)
        .chadexPadding(.vertical, 8)
        .accessibilityElement(children: .contain)
    }

    private func toggles(for path: String) -> some View {
        let connected = roots.contains(path)
        return HStack(spacing: 18) {
            Toggle(L10n.string("skills.external.connect"), isOn: Binding(
                get: { connected },
                set: { setConnected(path, $0) }
            ))
            .disabled(!canEdit)

            Toggle(L10n.string("skills.external.allowScripts"), isOn: Binding(
                get: { connected && scriptRoots.contains(path) },
                set: { setScripts(path, $0) }
            ))
            .disabled(!canEdit || !connected)
            Spacer(minLength: 0)
        }
        .toggleStyle(.switch)
        .controlSize(.small)
        .chadexFont(.caption)
    }

    @ViewBuilder
    private func invalidDisclosure(_ source: ExternalSkillSource) -> some View {
        let invalid = source.packages.filter { $0.state == "invalid" }
        if !invalid.isEmpty {
            DisclosureGroup(
                isExpanded: Binding(
                    get: { expandedInvalid.contains(source.id) },
                    set: { expanded in
                        if expanded { expandedInvalid.insert(source.id) } else { expandedInvalid.remove(source.id) }
                    }
                )
            ) {
                VStack(alignment: .leading, spacing: 3) {
                    ForEach(invalid) { package in
                        Text(package.package + " — " + (package.invalidReason ?? "invalid"))
                            .chadexFont(.caption, design: .monospaced)
                            .foregroundStyle(.tertiary)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                .chadexPadding(.top, 4)
            } label: {
                Text(L10n.string("skills.external.invalid", source.invalidCount))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func statusText(_ status: String) -> String {
        switch status {
        case "not_found": return L10n.string("skills.external.status.notFound")
        case "not_directory": return L10n.string("skills.external.status.notDirectory")
        case "unavailable": return L10n.string("skills.external.status.unavailable")
        case "scan_limit_exceeded": return L10n.string("skills.external.status.scanLimit")
        default: return status
        }
    }

    // MARK: Actions

    private func setConnected(_ path: String, _ connected: Bool) {
        var newRoots = roots.filter { $0 != path }
        var newScripts = scriptRoots
        if connected {
            newRoots.append(path)
        } else {
            newScripts.remove(path)
        }
        apply(roots: newRoots, scripts: newScripts)
    }

    private func setScripts(_ path: String, _ allowed: Bool) {
        var newScripts = scriptRoots
        if allowed { newScripts.insert(path) } else { newScripts.remove(path) }
        apply(roots: roots, scripts: newScripts)
    }

    private func apply(roots newRoots: [String], scripts: Set<String>) {
        let orderedScripts = newRoots.filter { scripts.contains($0) }
        Task { await model.applyExternalSkillRoots(roots: newRoots, scriptRoots: orderedScripts) }
    }

    private func chooseFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.directoryURL = FileManager.default.homeDirectoryForCurrentUser
        panel.prompt = L10n.string("skills.external.chooseFolderPrompt")
        guard panel.runModal() == .OK, let url = panel.url else { return }
        let canonical = url.resolvingSymlinksInPath().standardizedFileURL.path
        guard !roots.contains(canonical) else { return }
        apply(roots: roots + [canonical], scripts: scriptRoots)
    }
}
