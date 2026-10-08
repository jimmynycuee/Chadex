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

    static let cardFill = Color(nsColor: NSColor(name: "ChadexCardFill") { appearance in
        appearance.isDark ? NSColor(white: 1, alpha: 0.055) : NSColor.white
    })

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

    var body: some View {
        HStack(alignment: .top, spacing: 0) {
            CircuitNode(
                title: L10n.string("connection.projectNode"),
                detail: projectName,
                symbol: "laptopcomputer",
                state: .complete
            )
            link(filled: tunnelState == .complete, live: tunnelState == .active)
            CircuitNode(
                title: L10n.string("connection.tunnelNode"),
                detail: tunnelDetail,
                symbol: "lock.shield.fill",
                state: tunnelState
            )
            link(filled: chatGPTState == .complete, live: chatGPTState == .waiting)
            CircuitNode(
                title: L10n.string("connection.chatGPTNode"),
                detail: chatGPTDetail,
                symbol: "sparkles",
                state: chatGPTState
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
        if phase == .error { return .error }
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
        if phase == .error { return L10n.string("a11y.step.error") }
        return L10n.string("circuit.chatGPT.idle")
    }

    private func link(filled: Bool, live: Bool) -> some View {
        GeometryReader { proxy in
            ZStack(alignment: .leading) {
                Capsule()
                    .fill(Color.secondary.opacity(0.22))
                if filled {
                    Capsule().fill(ChadexBrand.signal)
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
        case .complete: return ChadexBrand.signal
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
        case .complete, .error: return .white
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
