import AppKit
import SwiftUI

enum ChadexMetrics {
    /// Width of the centered content column shared by Skills, Agent Settings and the project pages.
    static let detailMaxWidth: CGFloat = 960
    static let detailHorizontalPadding: CGFloat = 28
    static let detailVerticalPadding: CGFloat = 24
    static let sectionSpacing: CGFloat = 24
    static let compactSectionSpacing: CGFloat = 18
    static let contentCornerRadius: CGFloat = 8
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

    /// HIG › Typography: macOS text never goes below 10 pt, including at
    /// the compact (80%) interface size.
    static let minimumSize: CGFloat = 10

    /// Rendered point size at an interface scale, never below the minimum.
    func size(at scale: CGFloat) -> CGFloat {
        max(Self.minimumSize, baseSize * scale)
    }

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
        // Above the HIG caption default: Chadex uses caption for explanatory
        // copy people must read, not only for metadata.
        case .caption: return 11
        // HIG macOS minimum text size is 10 pt.
        case .caption2: return 10
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
                size: style.size(at: layout.fontScale),
                weight: style == .headline && weight == .regular ? .semibold : weight,
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

/// Text-entry surface: the system text background plus a visible hairline,
/// so the editable area keeps a clear boundary in light, dark and Increase
/// Contrast (a 16% quaternary fill alone is ~1.05:1 against the window).
private struct ChadexEditorSurfaceModifier: ViewModifier {
    @Environment(\.chadexLayout) private var layout

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: layout.control(8), style: .continuous)
        content
            .background(Color(nsColor: .textBackgroundColor), in: shape)
            .overlay(shape.strokeBorder(ChadexSurface.separator, lineWidth: 1))
    }
}

extension View {
    func chadexEditorSurface() -> some View {
        modifier(ChadexEditorSurfaceModifier())
    }
}

/// Grouped-content surface (lists of skills, sources, memories): a faint
/// fill plus a hairline so the group edge stays visible (~1.05:1 fill alone).
/// One radius for every group so containers read as one family.
private struct ChadexGroupSurfaceModifier: ViewModifier {
    @Environment(\.chadexLayout) private var layout
    var inset = false

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: layout.control(inset ? 6 : 8), style: .continuous)
        content
            .background(.quaternary.opacity(inset ? 0.22 : 0.16), in: shape)
            .overlay(shape.strokeBorder(ChadexSurface.separator.opacity(inset ? 0 : 1), lineWidth: 1))
    }
}

extension View {
    /// `inset` marks a detail panel nested inside a group: no second border.
    func chadexGroupSurface(inset: Bool = false) -> some View {
        modifier(ChadexGroupSurfaceModifier(inset: inset))
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
                .foregroundStyle(.secondary)

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

/// Scrolling page container shared by the Skills, Agent Settings and project pages.
///
/// The content column is capped at `ChadexMetrics.detailMaxWidth` and centered in the detail pane, while the
/// scroll view (and the window background behind it) still extends to the window edges. Page-level actions
/// belong in a `ChadexPageHeader` inside the column so they stay aligned with the content on wide windows.
struct ChadexPageColumn<Content: View>: View {
    @Environment(\.chadexLayout) private var layout
    private let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: layout.spacing(ChadexMetrics.sectionSpacing)) {
                content
            }
            .frame(maxWidth: layout.control(ChadexMetrics.detailMaxWidth), alignment: .leading)
            .chadexPadding(.horizontal, ChadexMetrics.detailHorizontalPadding)
            .chadexPadding(.vertical, ChadexMetrics.detailVerticalPadding)
            .frame(maxWidth: .infinity, alignment: .top)
        }
    }
}

/// Title, one-line description and page-level actions for a `ChadexPageColumn`.
/// The actions sit trailing the title and drop below it when the column is too narrow.
struct ChadexPageHeader<Actions: View>: View {
    let title: String
    var subtitle: String?
    private let actions: Actions

    init(title: String, subtitle: String? = nil, @ViewBuilder actions: () -> Actions) {
        self.title = title
        self.subtitle = subtitle
        self.actions = actions()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            ViewThatFits(in: .horizontal) {
                HStack(alignment: .firstTextBaseline, spacing: 12) {
                    titleText
                    Spacer(minLength: 12)
                    actionRow
                }
                VStack(alignment: .leading, spacing: 8) {
                    titleText
                    actionRow
                }
            }

            if let subtitle {
                Text(subtitle)
                    .chadexFont(.callout)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var titleText: some View {
        Text(title)
            .chadexFont(.title2, weight: .semibold)
            .tracking(-0.2)
            .accessibilityAddTraits(.isHeader)
    }

    private var actionRow: some View {
        HStack(spacing: 8) { actions }
    }
}

/// Caption-size status: only the symbol carries the semantic tint, because
/// green and orange caption text fall to ~2.2:1 on a light window.
struct ChadexStatusLabel: View {
    let title: String
    let systemImage: String
    var tint: Color = .secondary
    var emphasized = false

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 5) {
            Image(systemName: systemImage)
                .foregroundStyle(tint)
            Text(title)
                .foregroundStyle(emphasized ? Color.primary : Color.secondary)
        }
        .chadexFont(.caption, weight: .medium)
        .accessibilityElement(children: .combine)
    }
}

/// Title for a top-level section of a page. It always outranks the
/// explanatory copy beneath it.
struct SectionTitle: View {
    let title: String

    var body: some View {
        Text(title)
            .chadexFont(.title3, weight: .semibold)
            .accessibilityAddTraits(.isHeader)
    }
}

/// Inline, recoverable error: says what went wrong and offers the retry
/// right where it happened instead of pointing at a menu command.
struct ChadexInlineError: View {
    let message: String
    var font: ChadexFontStyle = .caption
    let retry: () async -> Void
    @State private var retrying = false

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.orange)
                .accessibilityHidden(true)
            Text(message)
                .foregroundStyle(.primary)
                .fixedSize(horizontal: false, vertical: true)
                .textSelection(.enabled)
            Spacer(minLength: 8)
            Button {
                retrying = true
                Task {
                    await retry()
                    retrying = false
                }
            } label: {
                if retrying {
                    ProgressView().controlSize(.mini)
                } else {
                    Text(L10n.string("common.tryAgain"))
                }
            }
            .controlSize(.small)
            .disabled(retrying)
        }
        .chadexFont(font)
    }
}

struct SectionEyebrow: View {
    let title: String

    var body: some View {
        Text(title)
            .chadexFont(.caption, weight: .semibold)
            .foregroundStyle(.secondary)
            .accessibilityAddTraits(.isHeader)
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
                .foregroundStyle(.secondary)
                .lineLimit(1)

            if let detail {
                Text(detail)
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .frame(minWidth: layout.control(74))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(detail.map { "\(title), \($0)" } ?? title)
        .accessibilityValue(accessibilityStateText)
    }

    private var accessibilityStateText: String {
        switch state {
        case .complete: return L10n.string("a11y.step.complete")
        case .ready: return L10n.string("a11y.step.ready")
        case .active: return L10n.string("a11y.step.active")
        case .inactive: return L10n.string("a11y.step.inactive")
        case .error: return L10n.string("a11y.step.error")
        }
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
