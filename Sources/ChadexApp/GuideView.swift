import AppKit
import SwiftUI

struct GuideView: View {
    @Environment(\.chadexLayout) private var layout
    @EnvironmentObject private var model: AppModel
    @AppStorage("guide.openAIProjectConfirmed") private var openAIProjectConfirmed = false
    @AppStorage("guide.chatGPTPluginConfirmed") private var chatGPTPluginConfirmed = false

    private enum GuideSection: Hashable {
        case gettingStarted
        case openAIProject
        case credentials
        case localProject
        case connection
        case chatGPTPlugin
        case finalTest
    }

    private let projectsURL = URL(string: "https://platform.openai.com/settings/organization/projects")!
    private let tunnelsURL = URL(string: "https://platform.openai.com/settings/organization/tunnels")!
    private let apiKeysURL = URL(string: "https://platform.openai.com/settings/organization/api-keys")!
    private let chatGPTURL = URL(string: "https://chatgpt.com")!
    private let officialMCPGuideURL = URL(string: "https://help.openai.com/en/articles/12584461-developer-mode-and-mcp-apps-in-chatgpt")!

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                ViewThatFits(in: .horizontal) {
                    wideLayout(proxy: proxy)
                    narrowLayout
                }
                .frame(maxWidth: .infinity, alignment: .topLeading)
            }
        }
        .navigationTitle(L10n.string("sidebar.guide"))
    }

    private func wideLayout(proxy: ScrollViewProxy) -> some View {
        HStack(alignment: .top, spacing: layout.spacing(ChadexMetrics.guideWideGap)) {
            guideRail(proxy: proxy)
                .frame(width: ChadexMetrics.guideRailWidth, alignment: .topLeading)

            article(showInlineProgress: false)
                .frame(width: ChadexMetrics.guideArticleWidth, alignment: .topLeading)
        }
        .chadexPadding(.horizontal, 32)
        .chadexPadding(.top, 28)
        .chadexPadding(.bottom, 36)
    }

    private var narrowLayout: some View {
        article(showInlineProgress: true)
            .frame(maxWidth: ChadexMetrics.guideArticleWidth, alignment: .topLeading)
            .chadexPadding(.horizontal, 28)
            .chadexPadding(.top, 28)
            .chadexPadding(.bottom, 36)
            .frame(maxWidth: .infinity, alignment: .topLeading)
    }

    private func article(showInlineProgress: Bool) -> some View {
        VStack(alignment: .leading, spacing: layout.spacing(24)) {
            header
                .id(GuideSection.gettingStarted)

            if showInlineProgress {
                progress
            }

            GuideGroup(title: L10n.string("guide.groupOpenAI")) {
                GuideStep(
                    number: 1,
                    title: L10n.string("guide.openAIProjectTitle"),
                    message: L10n.string("guide.openAIProjectMessage"),
                    complete: openAIProjectConfirmed
                ) {
                    VStack(alignment: .leading, spacing: 10) {
                        GuideInstructionRow(number: 1, text: L10n.string("guide.openAIProject1"))
                        GuideInstructionRow(number: 2, text: L10n.string("guide.openAIProject2"))
                        Link(L10n.string("guide.openProjects"), destination: projectsURL)

                        Toggle(L10n.string("guide.confirmOpenAIProject"), isOn: $openAIProjectConfirmed)
                            .toggleStyle(.checkbox)
                            .controlSize(.small)
                    }
                }
                .id(GuideSection.openAIProject)

                GuideStep(
                    number: 2,
                    title: L10n.string("guide.credentialsTitle"),
                    message: L10n.string("guide.credentialsMessage"),
                    complete: credentialsReady
                ) {
                    VStack(alignment: .leading, spacing: 10) {
                        GuideInstructionRow(number: 1, text: L10n.string("guide.tunnelSetup"))
                        GuideInstructionRow(number: 2, text: L10n.string("guide.apiKeySetup"))

                        Text(L10n.string("guide.apiKeyCopyWarning"))
                            .chadexFont(.caption, weight: .medium)
                            .foregroundStyle(.secondary)

                        HStack(spacing: 12) {
                            Link(L10n.string("onboarding.openTunnels"), destination: tunnelsURL)
                            Link(L10n.string("onboarding.openAPIKeys"), destination: apiKeysURL)
                        }

                        Label(
                            model.hasStoredAPIKey
                                ? L10n.string("settings.apiKeyStored")
                                : L10n.string("settings.apiKeyNotStored"),
                            systemImage: model.hasStoredAPIKey ? "checkmark.circle.fill" : "circle.dashed"
                        )
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                    }
                }
                .id(GuideSection.credentials)
            }

            Divider()

            GuideGroup(title: L10n.string("guide.groupChadex")) {
                GuideStep(
                    number: 3,
                    title: L10n.string("guide.projectTitle"),
                    message: L10n.string("guide.projectMessage"),
                    complete: projectReady
                ) {
                    Button(L10n.string("project.add")) {
                        model.addProjectFromPanel()
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.regular)
                }
                .id(GuideSection.localProject)

                GuideStep(
                    number: 4,
                    title: L10n.string("guide.connectTitle"),
                    message: L10n.string("guide.connectMessage"),
                    complete: model.snapshot.tunnelReady
                ) {
                    VStack(alignment: .leading, spacing: 10) {
                        Button(L10n.string("guide.openConnectionSetup")) {
                            model.showConnectionSettings()
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.regular)

                        if model.snapshot.tunnelReady {
                            Label(L10n.string("status.readyToUse"), systemImage: "checkmark.circle.fill")
                                .foregroundStyle(.green)
                                .chadexFont(.callout, weight: .medium)
                        } else if credentialsReady && projectReady {
                            Button(L10n.string("connection.connect")) {
                                model.primaryAction()
                            }
                            .buttonStyle(.borderedProminent)
                            .controlSize(.regular)
                            .disabled(model.connectionActionInFlight)
                        } else {
                            Text(L10n.string("guide.connectPrerequisite"))
                                .chadexFont(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                .id(GuideSection.connection)
            }

            Divider()

            GuideGroup(title: L10n.string("guide.groupChatGPT")) {
                GuideStep(
                    number: 5,
                    title: L10n.string("guide.createAppTitle"),
                    message: L10n.string("guide.createAppMessage"),
                    complete: chatGPTPluginConfirmed
                ) {
                    VStack(alignment: .leading, spacing: 10) {
                        GuideInstructionRow(number: 1, text: L10n.string("guide.createApp1"))
                        GuideInstructionRow(number: 2, text: L10n.string("guide.createApp2"))
                        GuideInstructionRow(number: 3, text: L10n.string("guide.createApp3"))
                        GuideInstructionRow(number: 4, text: L10n.string("guide.createApp4"))
                        GuideInstructionRow(number: 5, text: L10n.string("guide.createApp5"))

                        Text(L10n.string("guide.createAppAccessNote"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .chadexPadding(.top, 2)

                        HStack(spacing: 12) {
                            Link(L10n.string("guide.openChatGPT"), destination: chatGPTURL)
                            Link(L10n.string("guide.officialMCPGuide"), destination: officialMCPGuideURL)
                        }

                        Toggle(L10n.string("guide.confirmChatGPTPlugin"), isOn: $chatGPTPluginConfirmed)
                            .toggleStyle(.checkbox)
                            .controlSize(.small)
                    }
                }
                .id(GuideSection.chatGPTPlugin)
            }

            Divider()

            GuideGroup(title: L10n.string("guide.groupUse")) {
                VStack(alignment: .leading, spacing: 12) {
                    Text(L10n.string("guide.firstUseTitle"))
                        .chadexFont(.headline)

                    Text(L10n.string("guide.firstUseMessage"))
                        .chadexFont(.callout)
                        .foregroundStyle(.secondary)

                    HStack(alignment: .center, spacing: 10) {
                        Text(L10n.string("guide.testPrompt"))
                            .chadexFont(.callout, design: .monospaced)
                            .textSelection(.enabled)

                        Button {
                            NSPasteboard.general.clearContents()
                            NSPasteboard.general.setString(
                                L10n.string("guide.testPrompt"),
                                forType: .string
                            )
                        } label: {
                            Image(systemName: "doc.on.doc")
                        }
                        .buttonStyle(.borderless)
                        .help(L10n.string("common.copy"))
                        .accessibilityLabel(L10n.string("common.copy"))
                    }

                    if model.snapshot.chatGPTVerifiedForSelectedProject {
                        VStack(alignment: .leading, spacing: 4) {
                            Label(L10n.string("guide.finishTitle"), systemImage: "checkmark.circle.fill")
                                .foregroundStyle(.green)
                                .chadexFont(.callout, weight: .semibold)

                            Text(L10n.string("guide.finishMessage"))
                                .chadexFont(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .chadexPadding(.top, 2)
                    } else if model.snapshot.tunnelReady {
                        Label(L10n.string("guide.waitingForTest"), systemImage: "sparkles")
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .chadexPadding(.top, 2)
                    }
                }
            }
            .id(GuideSection.finalTest)

            Text(L10n.string("guide.uiMayChange"))
                .chadexFont(.caption)
                .foregroundStyle(.tertiary)
                .chadexPadding(.bottom, 8)
        }
    }

    private func guideRail(proxy: ScrollViewProxy) -> some View {
        VStack(alignment: .leading, spacing: layout.spacing(6)) {
            SectionEyebrow(title: L10n.string("guide.progress"))
                .chadexPadding(.bottom, 4)

            railButton(
                title: L10n.string("guide.progressFirstUse"),
                section: .gettingStarted,
                complete: nil,
                proxy: proxy
            )
            railButton(
                title: L10n.string("guide.progressOpenAIProject"),
                section: .openAIProject,
                complete: openAIProjectConfirmed,
                proxy: proxy
            )
            railButton(
                title: L10n.string("guide.progressCredentials"),
                section: .credentials,
                complete: credentialsReady,
                proxy: proxy
            )
            railButton(
                title: L10n.string("guide.progressProject"),
                section: .localProject,
                complete: projectReady,
                proxy: proxy
            )
            railButton(
                title: L10n.string("guide.progressTunnel"),
                section: .connection,
                complete: model.snapshot.tunnelReady,
                proxy: proxy
            )
            railButton(
                title: L10n.string("guide.progressPlugin"),
                section: .chatGPTPlugin,
                complete: chatGPTPluginConfirmed,
                proxy: proxy
            )
            railButton(
                title: L10n.string("guide.progressFinalTest"),
                section: .finalTest,
                complete: model.snapshot.chatGPTVerifiedForSelectedProject,
                proxy: proxy
            )
        }
        .chadexPadding(.top, 4)
    }

    private func railButton(
        title: String,
        section: GuideSection,
        complete: Bool?,
        proxy: ScrollViewProxy
    ) -> some View {
        Button {
            proxy.scrollTo(section, anchor: .top)
        } label: {
            HStack(spacing: 8) {
                Image(systemName: railSymbol(for: complete))
                    .chadexFont(.caption, weight: .medium)
                    .foregroundStyle(complete == true ? Color.green : Color.secondary)
                    .frame(width: 16)

                Text(title)
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)

                Spacer(minLength: 0)
            }
            .chadexPadding(.vertical, 5)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
    }

    private func railSymbol(for complete: Bool?) -> String {
        guard let complete else { return "book.closed" }
        return complete ? "checkmark.circle.fill" : "circle"
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 7) {
            Text(L10n.string("guide.title"))
                .chadexFont(.largeTitle, weight: .semibold)
                .tracking(-0.4)

            Text(L10n.string("guide.subtitle"))
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: 720, alignment: .leading)

            Label(L10n.string("guide.beginnerNote"), systemImage: "hand.wave")
                .chadexFont(.caption)
                .foregroundStyle(.secondary)
                .chadexPadding(.top, 3)
        }
    }

    private var progress: some View {
        VStack(alignment: .leading, spacing: layout.spacing(10)) {
            SectionEyebrow(title: L10n.string("guide.progress"))

            LazyVGrid(
                columns: [GridItem(.adaptive(minimum: layout.control(130)), spacing: layout.spacing(12), alignment: .leading)],
                alignment: .leading,
                spacing: layout.spacing(8)
            ) {
                GuideProgressItem(title: L10n.string("guide.progressOpenAIProject"), complete: openAIProjectConfirmed)
                GuideProgressItem(title: L10n.string("guide.progressCredentials"), complete: credentialsReady)
                GuideProgressItem(title: L10n.string("guide.progressProject"), complete: projectReady)
                GuideProgressItem(title: L10n.string("guide.progressTunnel"), complete: model.snapshot.tunnelReady)
                GuideProgressItem(title: L10n.string("guide.progressPlugin"), complete: chatGPTPluginConfirmed)
                GuideProgressItem(title: L10n.string("guide.progressFinalTest"), complete: model.snapshot.chatGPTVerifiedForSelectedProject)
            }
        }
    }

    private var projectReady: Bool {
        !model.projects.isEmpty
    }

    private var credentialsReady: Bool {
        !model.preferences.tunnelID.isEmpty && model.hasStoredAPIKey
    }
}

private struct GuideGroup<Content: View>: View {
    @Environment(\.chadexLayout) private var layout
    let title: String
    @ViewBuilder let content: Content

    init(title: String, @ViewBuilder content: () -> Content) {
        self.title = title
        self.content = content()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: layout.spacing(18)) {
            SectionEyebrow(title: title)
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct GuideStep<Content: View>: View {
    @Environment(\.chadexLayout) private var layout
    let number: Int
    let title: String
    let message: String
    let complete: Bool
    @ViewBuilder let content: Content

    init(
        number: Int,
        title: String,
        message: String,
        complete: Bool,
        @ViewBuilder content: () -> Content
    ) {
        self.number = number
        self.title = title
        self.message = message
        self.complete = complete
        self.content = content()
    }

    var body: some View {
        HStack(alignment: .top, spacing: layout.spacing(14)) {
            Group {
                if complete {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(.green)
                } else {
                    Text("\(number)")
                        .chadexFont(.caption, weight: .semibold)
                        .foregroundStyle(.secondary)
                        .overlay {
                            Circle()
                                .stroke(Color.secondary.opacity(0.35), lineWidth: 1)
                                .frame(width: layout.control(24), height: layout.control(24))
                        }
                }
            }
            .frame(width: layout.control(26), height: layout.control(26))

            VStack(alignment: .leading, spacing: layout.spacing(8)) {
                Text(title)
                    .chadexFont(.headline)

                Text(message)
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)

                content
                    .chadexPadding(.top, 2)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

private struct GuideProgressItem: View {
    let title: String
    let complete: Bool

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: complete ? "checkmark.circle.fill" : "circle")
                .foregroundStyle(complete ? Color.green : Color.secondary.opacity(0.55))
            Text(title)
                .foregroundStyle(.secondary)
        }
        .chadexFont(.caption)
        .accessibilityElement(children: .combine)
    }
}

private struct GuidePath: View {
    let text: String

    init(_ text: String) {
        self.text = text
    }

    var body: some View {
        Text(text)
            .chadexFont(.caption, design: .monospaced)
            .foregroundStyle(.secondary)
            .textSelection(.enabled)
    }
}

private struct GuideInstructionRow: View {
    let number: Int
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 9) {
            Text("\(number).")
                .chadexFont(.caption, weight: .semibold, design: .monospaced)
                .foregroundStyle(.tertiary)
                .frame(width: 18, alignment: .trailing)
            Text(text)
                .chadexFont(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

