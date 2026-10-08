import AppKit
import SwiftUI

enum SettingsTab: String, Hashable {
    case general
    case connection
    case advanced
}

struct SettingsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var selection: SettingsTab

    static let lastTabKey = "settings.lastTab"

    /// With no explicit tab, reopen the pane the person used last (HIG › Settings).
    init(initialTab: SettingsTab? = nil) {
        _selection = State(initialValue: Self.openingTab(initialTab))
    }

    static func openingTab(_ requested: SettingsTab?, defaults: UserDefaults = .standard) -> SettingsTab {
        requested
            ?? defaults.string(forKey: lastTabKey).flatMap(SettingsTab.init(rawValue:))
            ?? .general
    }

    var body: some View {
        TabView(selection: $selection) {
            GeneralSettingsView()
                .environmentObject(model)
                .tabItem { Label(L10n.string("settings.general"), systemImage: "gearshape") }
                .tag(SettingsTab.general)
            ConnectionSettingsView()
                .environmentObject(model)
                .tabItem { Label(L10n.string("settings.connection"), systemImage: "link") }
                .tag(SettingsTab.connection)
            AdvancedSettingsView()
                .environmentObject(model)
                .tabItem { Label(L10n.string("settings.advanced"), systemImage: "wrench.and.screwdriver") }
                .tag(SettingsTab.advanced)
        }
        .onChange(of: selection) { _, tab in
            UserDefaults.standard.set(tab.rawValue, forKey: Self.lastTabKey)
        }
    }
}

private struct SettingsPage<Content: View>: View {
    @Environment(\.chadexLayout) private var layout
    @ViewBuilder let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: layout.spacing(ChadexMetrics.settingsSectionSpacing)) {
                content
            }
            .frame(maxWidth: ChadexMetrics.settingsContentWidth, alignment: .leading)
            .chadexPadding(.top, 20)
            .chadexPadding(.bottom, 22)
            .chadexPadding(.horizontal, 16)
            .frame(maxWidth: .infinity, alignment: .top)
        }
    }
}

private struct SettingsSection<Content: View>: View {
    @Environment(\.chadexLayout) private var layout
    let title: String
    @ViewBuilder let content: Content

    init(_ title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: layout.spacing(ChadexMetrics.settingsSectionContentSpacing)) {
            Text(title)
                .chadexFont(.headline)
                .accessibilityAddTraits(.isHeader)
                .chadexPadding(.leading, 16)

            // Grouped like System Settings: the title sits above one raised card.
            VStack(alignment: .leading, spacing: layout.spacing(ChadexMetrics.settingsSectionContentSpacing)) {
                content
            }
            .chadexCard(padding: 16)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct SettingsFormRow<Control: View>: View {
    let label: String
    @ViewBuilder let control: Control

    init(_ label: String, @ViewBuilder control: () -> Control) {
        self.label = label
        self.control = control()
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: ChadexMetrics.settingsColumnSpacing) {
            Text(label)
                .foregroundStyle(.secondary)
                .frame(width: ChadexMetrics.settingsLabelWidth, alignment: .leading)

            control
                .frame(width: ChadexMetrics.settingsControlWidth, alignment: .leading)

            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct SettingsControlBlock<Content: View>: View {
    @ViewBuilder let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        content
            .frame(maxWidth: ChadexMetrics.settingsBodyWidth, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct SettingsFieldDetailBlock<Content: View>: View {
    @ViewBuilder let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        HStack(alignment: .top, spacing: ChadexMetrics.settingsColumnSpacing) {
            Color.clear
                .frame(width: ChadexMetrics.settingsLabelWidth, height: 1)
                .accessibilityHidden(true)

            content
                .frame(width: ChadexMetrics.settingsControlWidth, alignment: .leading)

            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct SettingsPrimaryControlModifier: ViewModifier {
    @Environment(\.chadexLayout) private var layout
    let monospaced: Bool

    func body(content: Content) -> some View {
        content
            .font(
                .system(
                    size: ChadexFontStyle.body.baseSize * settingsFontScale,
                    weight: .regular,
                    design: monospaced ? .monospaced : .default
                )
            )
            .controlSize(.regular)
            // Menus keep their intrinsic width; lead-align them so every
            // control in the column shares one left edge.
            .frame(maxWidth: .infinity, alignment: .leading)
            .frame(height: layout.control(ChadexMetrics.settingsControlBaseHeight))
    }

    private var settingsFontScale: CGFloat {
        1 + (layout.fontScale - 1) * 0.70
    }
}

private extension View {
    func settingsPrimaryControl(monospaced: Bool = false) -> some View {
        modifier(SettingsPrimaryControlModifier(monospaced: monospaced))
    }
}

private struct GeneralSettingsView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var updateManager: UpdateManager
    @AppStorage(ChadexPreferenceKey.language) private var languageRaw = ChadexLanguage.system.rawValue
    @AppStorage(ChadexPreferenceKey.interfaceSize) private var interfaceSizeRaw = ChadexInterfaceSize.comfortable.rawValue
    @AppStorage(ChadexPreferenceKey.appearance) private var appearanceRaw = ChadexAppearance.defaultValue.rawValue
    @AppStorage(ChadexPreferenceKey.autoCheckUpdates) private var autoCheckUpdates = true
    @AppStorage("ferret.visible") private var ferretVisible = true
    @AppStorage("ferret.motion") private var ferretMotion = true
    @State private var confirmingGuideReset = false

    var body: some View {
        SettingsPage {
            SettingsSection(L10n.string("settings.appearance")) {
                VStack(alignment: .leading, spacing: ChadexMetrics.settingsRowSpacing) {
                    SettingsFormRow(L10n.string("settings.language")) {
                        Picker("", selection: $languageRaw) {
                            Text(L10n.string("settings.language.system")).tag(ChadexLanguage.system.rawValue)
                            Text(L10n.string("settings.language.zhHant")).tag(ChadexLanguage.traditionalChinese.rawValue)
                            Text(L10n.string("settings.language.english")).tag(ChadexLanguage.english.rawValue)
                        }
                        .labelsHidden()
                        .pickerStyle(.menu)
                        // One shared minimum keeps the three menus aligned and
                        // stops them resizing as the selection changes.
                        .frame(minWidth: layout.control(170), alignment: .leading)
                        .fixedSize()
                        .settingsPrimaryControl()
                    }

                    SettingsFormRow(L10n.string("settings.interfaceSize")) {
                        Picker("", selection: interfaceSizeBinding) {
                            ForEach(ChadexInterfaceSize.allCases) { size in
                                Text(interfaceSizeTitle(size)).tag(size)
                            }
                        }
                        .labelsHidden()
                        .pickerStyle(.menu)
                        // One shared minimum keeps the three menus aligned and
                        // stops them resizing as the selection changes.
                        .frame(minWidth: layout.control(170), alignment: .leading)
                        .fixedSize()
                        .settingsPrimaryControl()
                    }

                    SettingsFormRow(L10n.string("settings.colorScheme")) {
                        Picker("", selection: $appearanceRaw) {
                            Text(L10n.string("settings.colorScheme.dark")).tag(ChadexAppearance.dark.rawValue)
                            Text(L10n.string("settings.colorScheme.light")).tag(ChadexAppearance.light.rawValue)
                            Text(L10n.string("settings.colorScheme.system")).tag(ChadexAppearance.system.rawValue)
                        }
                        .labelsHidden()
                        .pickerStyle(.menu)
                        // One shared minimum keeps the three menus aligned and
                        // stops them resizing as the selection changes.
                        .frame(minWidth: layout.control(170), alignment: .leading)
                        .fixedSize()
                        .settingsPrimaryControl()
                    }
                }

                SettingsControlBlock {
                    Text(L10n.string("settings.interfaceSizeNote"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            SettingsSection(L10n.string("settings.companion")) {
                SettingsControlBlock {
                    VStack(alignment: .leading, spacing: ChadexMetrics.settingsRowSpacing) {
                        Toggle(L10n.string("ferret.visible"), isOn: $ferretVisible)
                        Toggle(L10n.string("ferret.motion"), isOn: $ferretMotion)
                            .disabled(!ferretVisible)

                        Text(L10n.string("ferret.explanation"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }

            SettingsSection(L10n.string("settings.startup")) {
                SettingsControlBlock {
                    VStack(alignment: .leading, spacing: 4) {
                        Toggle(L10n.string("settings.launchAtLogin"), isOn: Binding(
                            get: { model.launchAtLoginEnabled },
                            set: { model.setLaunchAtLogin($0) }
                        ))

                        Text(L10n.string("settings.launchAtLoginNote"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }

            SettingsSection(L10n.string("updates.section")) {
                VStack(alignment: .leading, spacing: ChadexMetrics.settingsRowSpacing) {
                    SettingsFormRow(L10n.string("updates.currentVersion")) {
                        Text("v\(updateManager.currentVersion) (\(updateManager.currentBuildNumber))")
                            .chadexFont(.caption, design: .monospaced)
                    }

                    SettingsControlBlock {
                        VStack(alignment: .leading, spacing: 8) {
                            Toggle(L10n.string("updates.autoCheck"), isOn: $autoCheckUpdates)

                            Text(L10n.string("updates.autoCheckNote"))
                                .chadexFont(.caption)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)

                            HStack(spacing: 8) {
                                Button(L10n.string("updates.check")) {
                                    Task { await updateManager.checkForUpdates(userInitiated: true) }
                                }
                                .buttonStyle(.bordered)
                                .chadexControlSize(.small)
                                .disabled(updateManager.isBusy)

                                if let release = updateManager.availableRelease {
                                    Button(L10n.string("updates.install", release.version)) {
                                        Task {
                                            await updateManager.installAvailableUpdate {
                                                !model.hasUpdateBlockingWork
                                            }
                                        }
                                    }
                                    .buttonStyle(.borderedProminent)
                                    .chadexControlSize(.small)
                                    .disabled(updateManager.isBusy || model.hasUpdateBlockingWork)

                                    Link(L10n.string("updates.releaseNotes"), destination: release.releasePageURL)
                                        .chadexFont(.caption)
                                }
                            }

                            Text(updateStatusText)
                                .chadexFont(.caption)
                                .foregroundStyle(.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }

            SettingsSection(L10n.string("settings.guide")) {
                SettingsControlBlock {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(L10n.string("settings.guideResetNote"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)

                        Button(L10n.string("settings.resetGuide")) {
                            confirmingGuideReset = true
                        }
                        .buttonStyle(.bordered)
                        .chadexControlSize(.small)
                    }
                }
            }
        }
        .alert(L10n.string("settings.resetGuideTitle"), isPresented: $confirmingGuideReset) {
            Button(L10n.string("settings.resetGuide"), role: .destructive) {
                UserDefaults.standard.set(false, forKey: ChadexPreferenceKey.guideOpenAIProjectConfirmed)
                UserDefaults.standard.set(false, forKey: ChadexPreferenceKey.guideChatGPTPluginConfirmed)
            }
            Button(L10n.string("common.cancel"), role: .cancel) {}
        } message: {
            Text(L10n.string("settings.resetGuideMessage"))
        }
    }

    private var updateStatusText: String {
        switch updateManager.phase {
        case .idle:
            return L10n.string("updates.status.ready")
        case .checking:
            return L10n.string("updates.status.checking")
        case .upToDate:
            return L10n.string("updates.status.upToDate")
        case .available(let version):
            if model.hasUpdateBlockingWork {
                return L10n.string("updates.status.waitingForIdle", version)
            }
            return L10n.string("updates.status.available", version)
        case .preparing(let version):
            return L10n.string("updates.status.preparing", version)
        case .installing(let version):
            return L10n.string("updates.status.installing", version)
        case .failed(let message):
            return message
        }
    }

    private var interfaceSizeBinding: Binding<ChadexInterfaceSize> {
        Binding(
            get: { ChadexInterfaceSize.resolve(storedValue: interfaceSizeRaw) },
            set: { interfaceSizeRaw = $0.rawValue }
        )
    }

    private func interfaceSizeTitle(_ size: ChadexInterfaceSize) -> String {
        let key: String
        switch size {
        case .compact: key = "settings.interfaceSize.compact"
        case .standard: key = "settings.interfaceSize.standard"
        case .comfortable: key = "settings.interfaceSize.comfortable"
        case .large: key = "settings.interfaceSize.large"
        case .extraLarge: key = "settings.interfaceSize.extraLarge"
        }
        return "\(L10n.string(key)) (\(size.percent)%)"
    }
}

struct ConnectionSettingsSheet: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    @State private var tunnelID = ""
    @State private var apiKey = ""
    @State private var saving = false

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(L10n.string("settings.connection"))
                        .chadexFont(.title2, weight: .semibold)
                    Text(L10n.string("settings.connectionSheetMessage"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)
                }
                Spacer()
            }
            .chadexPadding(.horizontal, 24)
            .chadexPadding(.vertical, 16)

            Divider()

            VStack(alignment: .leading, spacing: 16) {
                Grid(alignment: .leading, horizontalSpacing: 18, verticalSpacing: 12) {
                    GridRow {
                        Text(L10n.string("settings.tunnelID"))
                            .foregroundStyle(.secondary)
                        TextField("", text: $tunnelID)
                            .textFieldStyle(.roundedBorder)
                            .settingsPrimaryControl(monospaced: true)
                            .frame(width: ChadexMetrics.settingsControlWidth)
                    }

                    GridRow {
                        Text(L10n.string("settings.apiKey"))
                            .foregroundStyle(.secondary)
                        SecureField(L10n.string("settings.apiKeyPlaceholder"), text: $apiKey)
                            .textFieldStyle(.roundedBorder)
                            .settingsPrimaryControl()
                            .frame(width: ChadexMetrics.settingsControlWidth)
                    }
                }

                Label(
                    model.hasStoredAPIKey ? L10n.string("settings.apiKeyStored") : L10n.string("settings.apiKeyNotStored"),
                    systemImage: model.hasStoredAPIKey ? "checkmark.circle.fill" : "circle.dashed"
                )
                .chadexFont(.caption)
                .foregroundStyle(.secondary)

                Text(L10n.string("settings.secretNote"))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)

                HStack(spacing: 10) {
                    Spacer()

                    Button(L10n.string("common.cancel")) {
                        dismiss()
                    }
                    .keyboardShortcut(.cancelAction)

                    if saving {
                        ProgressView()
                            .chadexControlSize(.small)
                    }

                    Button(L10n.string("settings.save")) {
                        Task { await saveAndDismiss() }
                    }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                    .disabled(saving || !isDirty)
                }
            }
            .chadexPadding(24)
        }
        .frame(width: layout.control(620), height: layout.control(310))
        .onAppear {
            model.clearPresentedError()
            tunnelID = model.preferences.tunnelID
            apiKey = ""
        }
        .alert(item: $model.presentedError) { error in
            Alert(
                title: Text(error.title),
                message: Text("\(error.message)\n\n\(error.recovery)"),
                dismissButton: .default(Text(L10n.string("common.ok")))
            )
        }
    }

    private var isDirty: Bool {
        tunnelID.trimmingCharacters(in: .whitespacesAndNewlines) != model.preferences.tunnelID
            || !apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func saveAndDismiss() async {
        saving = true
        defer { saving = false }
        if await model.saveConnectionSettings(tunnelID: tunnelID, apiKey: apiKey) {
            dismiss()
        }
    }
}

struct ConnectionSettingsView: View {
    @EnvironmentObject private var model: AppModel
    @State private var tunnelID = ""
    @State private var apiKey = ""
    @State private var saving = false
    @State private var confirmingClearAPIKey = false

    var body: some View {
        SettingsPage {
            SettingsSection(L10n.string("settings.connectionConfig")) {
                VStack(alignment: .leading, spacing: ChadexMetrics.settingsRowSpacing) {
                    SettingsFormRow(L10n.string("settings.tunnelID")) {
                        TextField("", text: $tunnelID)
                            .textFieldStyle(.roundedBorder)
                            .settingsPrimaryControl(monospaced: true)
                    }

                    SettingsFormRow(L10n.string("settings.apiKey")) {
                        SecureField(L10n.string("settings.apiKeyPlaceholder"), text: $apiKey)
                            .textFieldStyle(.roundedBorder)
                            .settingsPrimaryControl()
                    }
                }

                SettingsFieldDetailBlock {
                    HStack(spacing: 10) {
                        Label(
                            model.hasStoredAPIKey ? L10n.string("settings.apiKeyStored") : L10n.string("settings.apiKeyNotStored"),
                            systemImage: model.hasStoredAPIKey ? "checkmark.circle.fill" : "circle.dashed"
                        )
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)

                        Spacer()

                        if model.hasStoredAPIKey {
                            Button(L10n.string("settings.clearAPIKey"), role: .destructive) {
                                confirmingClearAPIKey = true
                            }
                            .buttonStyle(.borderless)
                            .chadexFont(.caption)
                            .disabled(saving)
                        }
                    }
                }

                SettingsControlBlock {
                    Text(L10n.string("settings.secretNote"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                SettingsFieldDetailBlock {
                    HStack(spacing: 10) {
                        Spacer()

                        if saving {
                            ProgressView()
                                .chadexControlSize(.small)
                        }

                        Button(L10n.string("settings.saveChanges")) {
                            Task { await save() }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(saving || !isDirty)
                    }
                }
            }

            SettingsSection(L10n.string("settings.resources")) {
                SettingsControlBlock {
                    VStack(alignment: .leading, spacing: 8) {
                        Link(destination: URL(string: "https://platform.openai.com/settings/organization/tunnels")!) {
                            Label(L10n.string("onboarding.openTunnels"), systemImage: "arrow.up.right")
                        }
                        Link(destination: URL(string: "https://platform.openai.com/settings/organization/api-keys")!) {
                            Label(L10n.string("onboarding.openAPIKeys"), systemImage: "arrow.up.right")
                        }
                    }
                    .chadexFont(.callout)
                }
            }
        }
        .onAppear {
            tunnelID = model.preferences.tunnelID
            apiKey = ""
        }
        .alert(L10n.string("settings.clearAPIKeyTitle"), isPresented: $confirmingClearAPIKey) {
            Button(L10n.string("settings.clearAPIKey"), role: .destructive) {
                Task { await clearAPIKey() }
            }
            Button(L10n.string("common.cancel"), role: .cancel) {}
        } message: {
            Text(L10n.string("settings.clearAPIKeyMessage"))
        }
    }

    private var isDirty: Bool {
        tunnelID.trimmingCharacters(in: .whitespacesAndNewlines) != model.preferences.tunnelID
            || !apiKey.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private func save() async {
        saving = true
        defer { saving = false }
        if await model.saveConnectionSettings(tunnelID: tunnelID, apiKey: apiKey) {
            apiKey = ""
            tunnelID = model.preferences.tunnelID
        }
    }

    private func clearAPIKey() async {
        saving = true
        defer { saving = false }
        if await model.clearAPIKey() {
            apiKey = ""
        }
    }
}

private struct AdvancedSettingsView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        SettingsPage {
            SettingsSection(L10n.string("settings.restore")) {
                SettingsControlBlock {
                    VStack(alignment: .leading, spacing: ChadexMetrics.settingsRowSpacing) {
                        VStack(alignment: .leading, spacing: 4) {
                            Toggle(L10n.string("settings.prepareService"), isOn: Binding(
                                get: { model.preferences.prepareServiceOnLaunchEnabled },
                                set: { model.setPrepareServiceOnLaunch($0) }
                            ))
                            Text(L10n.string("settings.prepareServiceNote"))
                                .chadexFont(.caption)
                                .foregroundStyle(.secondary)
                        }

                        VStack(alignment: .leading, spacing: 4) {
                            Toggle(L10n.string("settings.restoreConnection"), isOn: Binding(
                                get: { model.preferences.restoreConnectionOnLaunch },
                                set: { model.setRestoreConnection($0) }
                            ))
                            Text(L10n.string("settings.restoreConnectionNote"))
                                .chadexFont(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }

            SettingsSection(L10n.string("settings.diagnostics")) {
                VStack(alignment: .leading, spacing: ChadexMetrics.settingsRowSpacing) {
                    SettingsFormRow(L10n.string("settings.protocol")) {
                        Text("1")
                            .chadexFont(.caption, design: .monospaced)
                    }

                    SettingsFormRow(L10n.string("settings.graphify")) {
                        if let graphify = model.snapshot.graphify {
                            // Value column reads left-aligned like every other row.
                            VStack(alignment: .leading, spacing: 2) {
                                Text(graphify.available
                                    ? L10n.string("settings.graphifyAvailable")
                                    : L10n.string("settings.graphifyNotFound"))
                                if let path = graphify.path {
                                    Text(path)
                                        .chadexFont(.caption, design: .monospaced)
                                        .foregroundStyle(.secondary)
                                        .lineLimit(1)
                                        .truncationMode(.middle)
                                        .help(path)
                                }
                            }
                        } else {
                            Text("—")
                                .foregroundStyle(.secondary)
                        }
                    }

                    SettingsFormRow(L10n.string("settings.state")) {
                        Text(StatusPresentation.text(for: model.connectionPresentationPhase))
                    }

                    SettingsFormRow(L10n.string("settings.activityCount")) {
                        Text("\(model.activities.count)")
                            .chadexFont(.caption, design: .monospaced)
                    }

                    SettingsFormRow(L10n.string("settings.performanceTraceCount")) {
                        Text("\(model.performanceTraces.count)")
                            .chadexFont(.caption, design: .monospaced)
                    }

                    if let latest = model.performanceTraces.last {
                        SettingsFormRow(L10n.string("settings.latestMcpLatency")) {
                            Text(String(format: "%.1f ms", latest.totalMilliseconds))
                                .chadexFont(.caption, design: .monospaced)
                        }

                        SettingsFormRow(L10n.string("settings.backendHeadersLatency")) {
                            Text(String(format: "%.1f ms", latest.backendHeadersMilliseconds))
                                .chadexFont(.caption, design: .monospaced)
                        }
                    }
                }
                .chadexFont(.callout)

                SettingsControlBlock {
                    VStack(alignment: .leading, spacing: 8) {
                        HStack(spacing: 8) {
                            Button(L10n.string("settings.refresh")) {
                                Task { await model.refreshDiagnostics() }
                            }
                            .buttonStyle(.bordered)
                            .chadexControlSize(.small)

                            Button(L10n.string("settings.exportDiagnostics")) {
                                model.exportDiagnostics()
                            }
                            .buttonStyle(.bordered)
                            .chadexControlSize(.small)
                        }

                        Text(L10n.string("settings.exportDiagnosticsNote"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
            .task {
                await model.refreshPerformanceTraces()
            }

            SettingsSection(L10n.string("settings.serviceControl")) {
                SettingsControlBlock {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(L10n.string("settings.serviceControlNote"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)

                        Button(L10n.string("settings.stopService"), role: .destructive) {
                            model.stopLocalService()
                        }
                        .buttonStyle(.bordered)
                        .chadexControlSize(.small)
                        .disabled(model.connectionActionInFlight)
                    }
                }
            }
        }
    }
}

struct MenuBarContent: View {
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var updateManager: UpdateManager
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Text(model.connectionActionStatusText ?? StatusPresentation.text(for: model.connectionPresentationPhase))
        if let project = model.selectedProject {
            Text(project.name)
                .foregroundStyle(.secondary)
        }
        Divider()
        Button(L10n.string("menubar.open")) { MainWindowPresenter.show(using: openWindow) }
        if model.snapshot.phase == .unconfigured {
            // Same destination as the overview's Set Up Connection button.
            Button(L10n.string("connection.configure")) {
                MainWindowPresenter.show(using: openWindow)
                model.showConnectionSettings()
            }
        } else {
            Button(model.primaryMenuActionTitle) { model.primaryAction() }
                .disabled(!model.primaryActionEnabled)
        }
        if model.snapshot.tunnelReady {
            Button(L10n.string("computer.stop"), role: .destructive) {
                Task { await model.stopComputerControl() }
            }
            .disabled(model.computerSafety.stopped || model.computerSafetyMutationInFlight)
        }
        Button(L10n.string("updates.checkMenu")) {
            MainWindowPresenter.show(using: openWindow)
            Task { await updateManager.checkForUpdates(userInitiated: true) }
        }
        .disabled(updateManager.isBusy)
        SettingsLink { Text(L10n.string("menubar.settings")) }
        Divider()
        Button(L10n.string("menubar.quit")) { NSApplication.shared.terminate(nil) }
            .keyboardShortcut("q")
    }

}
