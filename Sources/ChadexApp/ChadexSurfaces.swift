import AppKit
import SwiftUI

/// Chadex's own visual layer: one brand hue taken from Code Ferret's terminal
/// glow, used only for connection graphics, and raised content cards that sit
/// on the window background the way System Settings groups do.
enum ChadexBrand {
    /// Graphic-only accent (never body text). Light #0A8FB8 is 3.6:1 on white,
    /// dark #4CD3FF is 9.7:1 on the dark card, both above the 3:1 non-text bar.
    static let signal = Color(nsColor: NSColor(name: "ChadexSignal") { appearance in
        appearance.isDark
            ? NSColor(srgbRed: 0.30, green: 0.83, blue: 1.0, alpha: 1)
            : NSColor(srgbRed: 0.04, green: 0.56, blue: 0.72, alpha: 1)
    })

    /// Opaque in both appearances so the state wash behind a card never
    /// lowers the contrast of the text on it.
    static let cardFill = Color(nsColor: NSColor(name: "ChadexCardFill") { appearance in
        appearance.isDark ? NSColor(white: 0.155, alpha: 1) : NSColor.white
    })

    /// Glyph drawn on a solid `signal` fill: white on the deep light-mode hue
    /// (3.7:1), near-black on the bright dark-mode hue (white would be 1.7:1).
    static let onSignal = Color(nsColor: NSColor(name: "ChadexOnSignal") { appearance in
        appearance.isDark ? NSColor(white: 0.08, alpha: 1) : NSColor.white
    })

    /// Glyph color for a solid fill of `tint`, chosen so the symbol, which is
    /// the non-color cue, keeps at least 3:1 against its fill. White on
    /// systemGreen/systemOrange is only ~2.2:1, so those take near-black.
    static func glyph(on tint: Color) -> Color {
        // systemGray brightens in Dark Mode too (white is 2.9:1 there).
        if tint == signal || tint == .gray { return onSignal }
        if tint == .green || tint == .orange || tint == .yellow { return Color(white: 0.08) }
        return .white
    }

    static let cardStroke = Color(nsColor: NSColor(name: "ChadexCardStroke") { appearance in
        appearance.isDark ? NSColor(white: 1, alpha: 0.09) : NSColor(white: 0, alpha: 0.08)
    })
}

private extension NSAppearance {
    var isDark: Bool {
        bestMatch(from: [.darkAqua, .vibrantDark, .accessibilityHighContrastDarkAqua, .aqua]) != .aqua
    }
}

private struct ChadexCardModifier: ViewModifier {
    @Environment(\.chadexLayout) private var layout
    var padding: CGFloat

    func body(content: Content) -> some View {
        let shape = RoundedRectangle(cornerRadius: layout.control(12), style: .continuous)
        content
            .frame(maxWidth: .infinity, alignment: .leading)
            .chadexPadding(padding)
            .background(ChadexBrand.cardFill, in: shape)
            .overlay(shape.strokeBorder(ChadexBrand.cardStroke, lineWidth: 1))
    }
}

extension View {
    /// A raised content card. Use for the primary objects on a page, not for
    /// every block: section titles and page headers stay on the background.
    func chadexCard(padding: CGFloat = 20) -> some View {
        modifier(ChadexCardModifier(padding: padding))
    }

    /// A card that needs the person to act (an error, a pending approval):
    /// the same card plus a leading rule in the state color, so it outranks
    /// the static cards around it.
    func chadexAttentionCard(tint: Color, padding: CGFloat = 20) -> some View {
        modifier(ChadexAttentionCardModifier(tint: tint, padding: padding))
    }
}

private struct ChadexAttentionCardModifier: ViewModifier {
    @Environment(\.chadexLayout) private var layout
    let tint: Color
    let padding: CGFloat

    func body(content: Content) -> some View {
        let radius = layout.control(12)
        content
            .chadexCard(padding: padding)
            .overlay(alignment: .leading) {
                UnevenRoundedRectangle(
                    topLeadingRadius: radius,
                    bottomLeadingRadius: radius,
                    style: .continuous
                )
                .fill(tint)
                .frame(width: 4)
                .accessibilityHidden(true)
            }
    }
}

/// Compact state capsule: a colored dot plus a label in primary text, so the
/// color is never the only signal and never carries caption-size text.
struct ChadexStatusPill: View {
    let title: String
    let tint: Color
    var pulsing = false

    var body: some View {
        HStack(spacing: 6) {
            ChadexPulseDot(tint: tint, pulsing: pulsing)
            Text(title)
                .chadexFont(.caption, weight: .semibold)
                .foregroundStyle(.primary)
                .lineLimit(1)
        }
        .padding(.horizontal, 9)
        .padding(.vertical, 4)
        .background(tint.opacity(0.12), in: Capsule())
        .overlay(Capsule().strokeBorder(tint.opacity(0.22), lineWidth: 1))
        .accessibilityElement(children: .combine)
    }
}

struct ChadexPulseDot: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let tint: Color
    var pulsing = false
    @State private var expanded = false

    var body: some View {
        ZStack {
            if pulsing && !reduceMotion {
                Circle()
                    .fill(tint.opacity(expanded ? 0 : 0.45))
                    .frame(width: 7, height: 7)
                    .scaleEffect(expanded ? 2.6 : 1)
            }
            Circle()
                .fill(tint)
                .frame(width: 7, height: 7)
        }
        .frame(width: 10, height: 10)
        .onAppear {
            guard pulsing, !reduceMotion else { return }
            withAnimation(.easeOut(duration: 1.6).repeatForever(autoreverses: false)) {
                expanded = true
            }
        }
        .accessibilityHidden(true)
    }
}

/// The signature element: the Mac → secure tunnel → ChatGPT path drawn as a
/// circuit. Completed links fill with the brand signal; the hop waiting on
/// the person or on ChatGPT carries the only motion in the app (a soft halo,
/// still under Reduce Motion).
struct ConnectionCircuitView: View {
    @Environment(\.chadexLayout) private var layout
    let projectName: String
    let phase: ConnectionPhase
    let tunnelReady: Bool
    let chatGPTConnected: Bool
    let chatGPTVerified: Bool
    var isConnecting = false
    /// The connection state's hue, shared with the pill and the page wash.
    var accent: Color = ChadexBrand.signal

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            CircuitNode(
                title: L10n.string("connection.projectNode"),
                detail: projectName,
                symbol: "laptopcomputer",
                state: .complete,
                accent: accent
            )
            link(filled: tunnelState == .complete, live: tunnelState == .active)
            CircuitNode(
                title: L10n.string("connection.tunnelNode"),
                detail: tunnelDetail,
                symbol: "lock.shield.fill",
                state: tunnelState,
                accent: accent
            )
            link(filled: chatGPTState == .complete, live: chatGPTState == .waiting)
            CircuitNode(
                title: L10n.string("connection.chatGPTNode"),
                detail: chatGPTDetail,
                symbol: "sparkles",
                state: chatGPTState,
                accent: accent
            )
        }
        .accessibilityElement(children: .contain)
    }

    private var tunnelState: CircuitNode.State {
        if phase == .error { return .error }
        if tunnelReady || chatGPTVerified { return .complete }
        if isConnecting || phase == .preparing { return .active }
        return .idle
    }

    private var chatGPTState: CircuitNode.State {
        // A failed tunnel never reached ChatGPT; mark only the hop that failed.
        if phase == .error { return tunnelReady ? .error : .idle }
        if chatGPTConnected || chatGPTVerified { return .complete }
        if tunnelReady { return .waiting }
        return .idle
    }

    private var tunnelDetail: String {
        switch tunnelState {
        case .complete: return L10n.string("circuit.tunnel.ready")
        case .active: return L10n.string("circuit.tunnel.starting")
        case .error: return L10n.string("a11y.step.error")
        case .idle, .waiting: return L10n.string("circuit.tunnel.idle")
        }
    }

    private var chatGPTDetail: String {
        if chatGPTVerified { return L10n.string("circuit.chatGPT.verified") }
        if chatGPTConnected { return L10n.string("connection.connectedShort") }
        if tunnelReady { return L10n.string("connection.awaitingFirstUseShort") }
        if phase == .error && tunnelReady { return L10n.string("a11y.step.error") }
        return L10n.string("circuit.chatGPT.idle")
    }

    private func link(filled: Bool, live: Bool) -> some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Color.secondary.opacity(0.22))
                if filled {
                    Capsule().fill(accent)
                } else if live {
                    Capsule()
                        .fill(
                            LinearGradient(
                                colors: [ChadexBrand.signal, ChadexBrand.signal.opacity(0)],
                                startPoint: .leading,
                                endPoint: .trailing
                            )
                        )
                        .frame(width: proxy.size.width * 0.7)
                }
            }
        }
        .frame(height: 2)
        .padding(.top, layout.control(CircuitNode.diameter) / 2 - 1)
        .padding(.horizontal, layout.spacing(6))
        .accessibilityHidden(true)
    }
}

private struct CircuitNode: View {
    static let diameter: CGFloat = 40

    enum State: Equatable {
        case complete
        case active
        case waiting
        case idle
        case error
    }

    @Environment(\.chadexLayout) private var layout
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    let title: String
    let detail: String
    let symbol: String
    let state: State
    var accent: Color = ChadexBrand.signal
    @State private var halo = false

    var body: some View {
        VStack(spacing: layout.spacing(8)) {
            ZStack {
                if (state == .waiting || state == .active) && !reduceMotion {
                    Circle()
                        .stroke(ChadexBrand.signal.opacity(halo ? 0 : 0.5), lineWidth: 2)
                        .scaleEffect(halo ? 1.45 : 1)
                }
                Circle()
                    .fill(fill)
                Circle()
                    .strokeBorder(stroke, lineWidth: state == .waiting || state == .active ? 2 : 1)
                if state == .active {
                    ProgressView().controlSize(.small)
                } else {
                    Image(systemName: displayedSymbol)
                        .font(.system(size: layout.control(15), weight: .semibold))
                        .foregroundStyle(glyph)
                }
            }
            .frame(width: layout.control(Self.diameter), height: layout.control(Self.diameter))
            .onAppear(perform: startHalo)
            .onChange(of: state) { _, _ in startHalo() }

            VStack(spacing: 2) {
                Text(title)
                    .chadexFont(.callout, weight: .semibold)
                    .foregroundStyle(state == .idle ? Color.secondary : Color.primary)
                Text(detail)
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.middle)
            }
            .multilineTextAlignment(.center)
        }
        .frame(minWidth: layout.control(96), maxWidth: layout.control(150))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title), \(detail)")
        .accessibilityValue(accessibilityState)
    }

    private func startHalo() {
        halo = false
        guard (state == .waiting || state == .active), !reduceMotion else { return }
        withAnimation(.easeOut(duration: 1.8).repeatForever(autoreverses: false)) {
            halo = true
        }
    }

    private var displayedSymbol: String {
        switch state {
        case .error: return "exclamationmark"
        default: return symbol
        }
    }

    private var fill: Color {
        switch state {
        case .complete: return accent
        case .error: return .red
        case .active, .waiting: return ChadexBrand.signal.opacity(0.1)
        case .idle: return Color.secondary.opacity(0.1)
        }
    }

    private var stroke: Color {
        switch state {
        case .complete, .error: return .clear
        case .active, .waiting: return ChadexBrand.signal
        case .idle: return Color.secondary.opacity(0.25)
        }
    }

    private var glyph: Color {
        switch state {
        case .complete: return ChadexBrand.glyph(on: accent)
        case .error: return .white
        case .active, .waiting: return ChadexBrand.signal
        case .idle: return .secondary
        }
    }

    private var accessibilityState: String {
        switch state {
        case .complete: return L10n.string("a11y.step.complete")
        case .waiting: return L10n.string("a11y.step.ready")
        case .active: return L10n.string("a11y.step.active")
        case .idle: return L10n.string("a11y.step.inactive")
        case .error: return L10n.string("a11y.step.error")
        }
    }
}

/// The overview's ambient light: a soft wash in the connection's state color
/// that sits behind the content and, on macOS 26 and later, extends under the
/// floating Liquid Glass sidebar so the system material picks up the state.
/// Cards stay opaque on top, so text contrast never depends on the wash.
struct ConnectionAmbience: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.accessibilityReduceTransparency) private var reduceTransparency
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.colorScheme) private var colorScheme
    let tint: Color

    var body: some View {
        // Reduce Transparency and Increase Contrast get the plain window
        // background: the state stays in the pill, circuit and copy.
        let muted = reduceTransparency || contrast == .increased
        let strength: Double = muted ? 0 : (colorScheme == .dark ? 0.22 : 0.16)
        ZStack {
            LinearGradient(
                stops: [
                    .init(color: tint.opacity(strength), location: 0),
                    .init(color: tint.opacity(strength * 0.35), location: 0.38),
                    .init(color: tint.opacity(0), location: 0.75),
                ],
                startPoint: .top,
                endPoint: .bottom
            )
            RadialGradient(
                colors: [tint.opacity(strength * 0.9), tint.opacity(0)],
                center: .topTrailing,
                startRadius: 0,
                endRadius: 520
            )
        }
        .animation(reduceMotion ? nil : .easeInOut(duration: 0.8), value: tint)
        .chadexExtendsUnderSidebar()
        .ignoresSafeArea()
        .accessibilityHidden(true)
    }
}

extension View {
    @ViewBuilder
    func chadexExtendsUnderSidebar() -> some View {
        if #available(macOS 26.0, *) {
            backgroundExtensionEffect()
        } else {
            self
        }
    }
}
