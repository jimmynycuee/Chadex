import AppKit
import ApplicationServices
import CoreGraphics
import SwiftUI

/// The macOS privacy permissions Computer Use needs. The helper and the
/// runtime run as children of Chadex.app, so macOS attributes their access to
/// the app and the app can check the same grants itself.
enum ComputerPermission: String, CaseIterable, Identifiable {
    case accessibility
    case screenRecording

    var id: String { rawValue }

    var titleKey: String { "computer.permission.\(rawValue).title" }
    var purposeKey: String { "computer.permission.\(rawValue).purpose" }

    var symbol: String {
        switch self {
        case .accessibility: return "accessibility"
        case .screenRecording: return "rectangle.dashed.badge.record"
        }
    }

    /// The pane in System Settings › Privacy & Security for this permission.
    var settingsURL: URL {
        let anchor = switch self {
        case .accessibility: "Privacy_Accessibility"
        case .screenRecording: "Privacy_ScreenCapture"
        }
        return URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)")!
    }
}

struct ComputerPermissionState: Equatable {
    var accessibility: Bool
    var screenRecording: Bool

    func isGranted(_ permission: ComputerPermission) -> Bool {
        switch permission {
        case .accessibility: return accessibility
        case .screenRecording: return screenRecording
        }
    }

    var missing: [ComputerPermission] {
        ComputerPermission.allCases.filter { !isGranted($0) }
    }

    var allGranted: Bool { missing.isEmpty }

    static func current() -> ComputerPermissionState {
        ComputerPermissionState(
            accessibility: AXIsProcessTrusted(),
            screenRecording: CGPreflightScreenCaptureAccess()
        )
    }
}

/// Keeps the permission state current while the Computer Use page is open:
/// on appear, whenever Chadex comes back to the front (people grant access in
/// System Settings and switch back), and on a slow poll while something is
/// still missing.
@MainActor
final class ComputerPermissionMonitor: ObservableObject {
    @Published private(set) var state: ComputerPermissionState
    private let read: () -> ComputerPermissionState
    private var activationObserver: NSObjectProtocol?
    private var pollTask: Task<Void, Never>?

    init(read: @escaping () -> ComputerPermissionState = ComputerPermissionState.current) {
        self.read = read
        self.state = read()
    }

    func start() {
        refresh()
        if activationObserver == nil {
            activationObserver = NotificationCenter.default.addObserver(
                forName: NSApplication.didBecomeActiveNotification,
                object: nil,
                queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.refresh() }
            }
        }
        pollTask?.cancel()
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                guard let self, !Task.isCancelled else { return }
                if self.state.allGranted { continue }
                self.refresh()
            }
        }
    }

    func stop() {
        pollTask?.cancel()
        pollTask = nil
        if let activationObserver {
            NotificationCenter.default.removeObserver(activationObserver)
        }
        activationObserver = nil
    }

    func refresh() {
        let next = read()
        if next != state { state = next }
    }

    /// Adds Chadex to the permission's list in System Settings (macOS only
    /// lists apps that have asked) and opens that pane.
    func requestAccess(_ permission: ComputerPermission) {
        switch permission {
        case .accessibility:
            let options = ["AXTrustedCheckOptionPrompt": false] as CFDictionary
            _ = AXIsProcessTrustedWithOptions(options)
        case .screenRecording:
            _ = CGRequestScreenCaptureAccess()
        }
        NSWorkspace.shared.open(permission.settingsURL)
        refresh()
    }
}

struct ComputerPermissionsCard: View {
    @Environment(\.chadexLayout) private var layout
    @ObservedObject var monitor: ComputerPermissionMonitor

    var body: some View {
        let state = monitor.state
        Group {
            if state.allGranted {
                HStack(spacing: layout.spacing(10)) {
                    Image(systemName: "checkmark.circle.fill")
                        .foregroundStyle(Self.grantedText)
                        .accessibilityHidden(true)
                    Text(L10n.string("computer.permission.ready"))
                        .chadexFont(.callout)
                    Spacer(minLength: 0)
                }
                .accessibilityElement(children: .combine)
                .chadexCard(padding: 14)
            } else {
                VStack(alignment: .leading, spacing: layout.spacing(14)) {
                    VStack(alignment: .leading, spacing: layout.spacing(4)) {
                        Text(L10n.string("computer.permission.title"))
                            .chadexFont(.headline, weight: .semibold)
                            .accessibilityAddTraits(.isHeader)
                        Text(L10n.string("computer.permission.help"))
                            .chadexFont(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    ForEach(ComputerPermission.allCases) { permission in
                        row(permission, granted: state.isGranted(permission))
                    }
                    Text(L10n.string("computer.permission.footnote"))
                        .chadexFont(.caption)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                .chadexAttentionCard(tint: .orange, padding: 18)
            }
        }
        .onAppear { monitor.start() }
        .onDisappear { monitor.stop() }
    }

    /// Green text for "On". System green is too light to read as small text
    /// on a light card (about 2:1), so light mode uses a deeper green that
    /// keeps 4.5:1; dark mode keeps the system green.
    static let grantedText = Color(nsColor: NSColor(name: "ChadexGrantedText") { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? .systemGreen
            : NSColor(srgbRed: 0x1A / 255, green: 0x7F / 255, blue: 0x37 / 255, alpha: 1)
    })

    private func row(_ permission: ComputerPermission, granted: Bool) -> some View {
        HStack(alignment: .center, spacing: layout.spacing(12)) {
            Image(systemName: permission.symbol)
                .font(.system(size: layout.control(14), weight: .semibold))
                .foregroundStyle(granted ? ChadexBrand.glyph(on: .green) : ChadexBrand.glyph(on: .orange))
                .frame(width: layout.control(30), height: layout.control(30))
                .background(granted ? Color.green : Color.orange, in: RoundedRectangle(cornerRadius: layout.control(8), style: .continuous))
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(L10n.string(permission.titleKey))
                    .chadexFont(.callout, weight: .semibold)
                Text(L10n.string(permission.purposeKey))
                    .chadexFont(.caption)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: layout.spacing(12))
            if granted {
                Label(L10n.string("computer.permission.granted"), systemImage: "checkmark.circle.fill")
                    .chadexFont(.caption, weight: .semibold)
                    .foregroundStyle(.green)
                    .labelStyle(.titleAndIcon)
                    .fixedSize()
            } else {
                Button(L10n.string("computer.permission.open")) {
                    monitor.requestAccess(permission)
                }
                .buttonStyle(.borderedProminent)
                .chadexControlSize(.regular)
                .fixedSize()
                .help(L10n.string("computer.permission.openHelp"))
            }
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel(L10n.string(permission.titleKey))
        .accessibilityValue(L10n.string(granted ? "computer.permission.granted" : "computer.permission.missing"))
    }
}
