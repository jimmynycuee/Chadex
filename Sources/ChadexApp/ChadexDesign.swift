import AppKit
import SwiftUI

enum ChadexMetrics {
    static let detailMaxWidth: CGFloat = 1040
    static let detailHorizontalPadding: CGFloat = 28
    static let detailVerticalPadding: CGFloat = 24
    static let sectionSpacing: CGFloat = 24
    static let compactSectionSpacing: CGFloat = 18
    static let contentCornerRadius: CGFloat = 6
    static let sidebarFixedWidth: CGFloat = 200
    static let settingsContentWidth: CGFloat = 612
    static let settingsLabelWidth: CGFloat = 126
    static let settingsControlWidth: CGFloat = 360
    static let settingsBodyWidth: CGFloat = 560
    static let settingsColumnSpacing: CGFloat = 14
    static let settingsSectionSpacing: CGFloat = 20
    static let settingsSectionContentSpacing: CGFloat = 8
    static let settingsRowSpacing: CGFloat = 8
    static let settingsControlBaseHeight: CGFloat = 30
    static let guideRailWidth: CGFloat = 196
    static let guideArticleWidth: CGFloat = 820
    static let guideWideGap: CGFloat = 28
}

enum ChadexFontStyle {
    case largeTitle
    case title
    case title2
    case title3
    case headline
    case body
    case callout
    case subheadline
    case footnote
    case caption
    case caption2

    var baseSize: CGFloat {
        switch self {
        case .largeTitle: return 26
        case .title: return 22
        case .title2: return 17
        case .title3: return 15
        case .headline: return 13
        case .body: return 13
        case .callout: return 12
        case .subheadline: return 11
        case .footnote: return 10
        case .caption: return 10
        case .caption2: return 9
        }
    }
}

private struct ChadexScaledFontModifier: ViewModifier {
    @Environment(\.chadexLayout) private var layout
    let style: ChadexFontStyle
    let weight: Font.Weight
    let design: Font.Design

    func body(content: Content) -> some View {
        content.font(
            .system(
                size: style.baseSize * layout.fontScale,
                weight: weight,
                design: design
            )
        )
    }
}

private struct ChadexScaledPaddingModifier: ViewModifier {
    @Environment(\.chadexLayout) private var layout
    let edges: Edge.Set
    let length: CGFloat

    func body(content: Content) -> some View {
        content.padding(edges, layout.spacing(length))
    }
}

extension View {
    func chadexFont(
        _ style: ChadexFontStyle,
        weight: Font.Weight = .regular,
        design: Font.Design = .default
    ) -> some View {
        modifier(
            ChadexScaledFontModifier(
                style: style,
                weight: weight,
                design: design
            )
        )
    }

    func chadexPadding(_ length: CGFloat) -> some View {
        modifier(
            ChadexScaledPaddingModifier(
                edges: .all,
                length: length
            )
        )
    }

    func chadexPadding(_ edges: Edge.Set, _ length: CGFloat) -> some View {
        modifier(
            ChadexScaledPaddingModifier(
                edges: edges,
                length: length
            )
        )
    }
}

enum ChadexSurface {
    static let subtle = Color(nsColor: .controlBackgroundColor).opacity(0.55)
    static let quiet = Color(nsColor: .controlBackgroundColor).opacity(0.32)
    static let sidebarFooter = Color(nsColor: .underPageBackgroundColor).opacity(0.42)
    static let separator = Color(nsColor: .separatorColor)
}

extension ConnectionPhase {
    var chadexSymbol: String {
        switch self {
        case .unconfigured: return "circle.dashed"
        case .preparing: return "arrow.triangle.2.circlepath"
        case .waitingForChatGPTVerification: return "hourglass"
        case .verified: return "checkmark.circle.fill"
        case .stopped: return "stop.circle"
        case .error: return "exclamationmark.triangle.fill"
        }
    }

    var chadexTint: Color {
        switch self {
        case .verified, .waitingForChatGPTVerification, .preparing:
            return .accentColor
        case .error:
            return .red
        case .unconfigured, .stopped:
            return .secondary
        }
    }
}

struct PhaseBadge: View {
    let phase: ConnectionPhase
    var isConnecting = false

    var body: some View {
        HStack(spacing: 6) {
            if isConnecting || phase == .preparing {
                ProgressView()
                    .controlSize(.mini)
            } else {
                Image(systemName: phase.chadexSymbol)
                    .chadexFont(.caption, weight: .semibold)
            }

            Text(isConnecting ? L10n.string("status.connecting") : StatusPresentation.text(for: phase))
                .chadexFont(.callout, weight: .medium)
        }
        .foregroundStyle(isConnecting ? Color.accentColor : phase.chadexTint)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(L10n.string("status.accessibility", isConnecting ? L10n.string("status.connecting") : StatusPresentation.text(for: phase)))
    }
}

struct ConnectionPathView: View {
    let phase: ConnectionPhase
    let tunnelReady: Bool
    let chatGPTConnected: Bool
    let chatGPTVerified: Bool
    var isConnecting = false

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            ConnectionNode(
                title: L10n.string("connection.projectNode"),
                symbol: "folder",
                state: .complete
            )
            connector(active: isConnecting || tunnelReady || chatGPTVerified)
            ConnectionNode(
                title: L10n.string("connection.tunnelNode"),
                symbol: "lock.shield",
                state: tunnelState
            )
            connector(active: chatGPTConnected || chatGPTVerified)
            ConnectionNode(
                title: L10n.string("connection.chatGPTNode"),
                symbol: "sparkles",
                state: chatGPTState,
                detail: chatGPTDetail
            )
        }
        .accessibilityElement(children: .contain)
    }

    private var tunnelState: ConnectionNode.State {
        if phase == .error { return .error }
        if tunnelReady || chatGPTVerified { return .complete }
        if isConnecting || phase == .preparing { return .active }
        return .inactive
    }

    private var chatGPTState: ConnectionNode.State {
        if phase == .error { return .error }
        if chatGPTConnected || chatGPTVerified { return .complete }
        if tunnelReady { return .ready }
        return .inactive
    }

    private var chatGPTDetail: String? {
        if chatGPTVerified { return nil }
        if chatGPTConnected { return L10n.string("connection.connectedShort") }
        if tunnelReady { return L10n.string("connection.awaitingFirstUseShort") }
        return nil
    }

    private func connector(active: Bool) -> some View {
        Rectangle()
            .fill(active ? Color.accentColor.opacity(0.38) : ChadexSurface.separator.opacity(0.8))
            .frame(maxWidth: .infinity)
            .frame(height: 1)
            .chadexPadding(.top, 10)
            .chadexPadding(.horizontal, 8)
            .accessibilityHidden(true)
    }
}

struct TechnicalMetadataItem: View {
    let title: String
    let value: String
    var monospaced = false
    var copyValue: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title)
                .chadexFont(.caption, weight: .medium)
                .foregroundStyle(.tertiary)

            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text(value)
                    .chadexFont(.callout, design: monospaced ? .monospaced : .default)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
                    .textSelection(.enabled)

                if let copyValue {
                    Button {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(copyValue, forType: .string)
                    } label: {
                        Image(systemName: "doc.on.doc")
                            .chadexFont(.caption2, weight: .medium)
                    }
                    .buttonStyle(.borderless)
                    .help(L10n.string("common.copy"))
                    .accessibilityLabel(L10n.string("common.copy"))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct SectionEyebrow: View {
    let title: String

    var body: some View {
        Text(title)
            .chadexFont(.caption, weight: .semibold)
            .foregroundStyle(.secondary)
    }
}

private struct ConnectionNode: View {
    @Environment(\.chadexLayout) private var layout

    enum State {
        case complete
        case ready
        case active
        case inactive
        case error
    }

    let title: String
    let symbol: String
    let state: State
    var detail: String? = nil

    var body: some View {
        VStack(spacing: 5) {
            Group {
                if state == .active {
                    ProgressView()
                        .controlSize(.mini)
                } else {
                    Image(systemName: displayedSymbol)
                        .chadexFont(.caption, weight: .semibold)
                        .foregroundStyle(tint)
                }
            }
            .frame(width: layout.control(20), height: layout.control(20))

            Text(title)
                .chadexFont(.caption, weight: state == .inactive ? .regular : .medium)
                .foregroundStyle(state == .inactive ? .tertiary : .secondary)
                .lineLimit(1)

            if let detail {
                Text(detail)
                    .chadexFont(.caption)
                    .foregroundStyle(state == .ready ? Color.green.opacity(0.82) : Color.secondary)
                    .lineLimit(1)
            }
        }
        .frame(minWidth: layout.control(74))
        .accessibilityElement(children: .combine)
    }

    private var displayedSymbol: String {
        switch state {
        case .complete: return "checkmark.circle.fill"
        case .error: return "exclamationmark.triangle.fill"
        case .ready, .active, .inactive: return symbol
        }
    }

    private var tint: Color {
        switch state {
        case .complete, .active: return .accentColor
        case .ready: return .green
        case .error: return .red
        case .inactive: return .secondary
        }
    }
}
