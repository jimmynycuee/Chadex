import AppKit
import SwiftUI

/// Whether ChatGPT is operating the Mac right now, as far as Chadex can tell.
///
/// ChatGPT calls Computer Use one action at a time and never says "done", so
/// "finished" only means no action arrived for `ComputerActivityTracker.idleDelay`.
enum ComputerActivityState: Equatable, Sendable {
    case idle
    case active(lastActionAt: Date)
    /// No action for a while; shown briefly, then `.idle`.
    case quiet
}

/// Turns helper overlay events into `ComputerActivityState`. Unlike the cursor
/// overlay it needs no geometry: every action counts, including keys.
@MainActor
final class ComputerActivityTracker {
    static let idleDelay: TimeInterval = 15
    static let quietDisplay: TimeInterval = 5
    /// Events that sat in a queue longer than this are old news.
    static let staleEventLimitMs: UInt64 = 5000

    private(set) var state: ComputerActivityState = .idle {
        didSet { if state != oldValue { onChange?(state) } }
    }
    var onChange: ((ComputerActivityState) -> Void)?

    private let now: @MainActor () -> Date
    private let schedule: @MainActor (TimeInterval, @escaping @MainActor () -> Void) -> (() -> Void)
    private var cancelTimer: (() -> Void)?

    init(
        now: @escaping @MainActor () -> Date = { Date() },
        schedule: @escaping @MainActor (TimeInterval, @escaping @MainActor () -> Void) -> (() -> Void)
    ) {
        self.now = now
        self.schedule = schedule
    }

    func handle(_ event: HelperEvent) {
        switch event {
        case .computerOverlay(let data):
            handle(data)
        }
    }

    func handle(_ data: ComputerOverlayEventData) {
        switch data.phase {
        case .willAct, .finished:
            if let emitted = data.emittedAtMs {
                let nowMs = UInt64(max(0, now().timeIntervalSince1970 * 1000))
                if nowMs > emitted, nowMs - emitted > Self.staleEventLimitMs { return }
            }
            markActive()
        case .clear:
            // An overflow only lost some frames; the agent is still working.
            if data.reason == "overflow" { return }
            reset()
        }
    }

    /// Back to idle at once: Stop, helper exit, events cleared by the helper.
    func reset() {
        cancelTimer?()
        cancelTimer = nil
        state = .idle
    }

    private func markActive() {
        state = .active(lastActionAt: now())
        cancelTimer?()
        cancelTimer = schedule(Self.idleDelay) { [weak self] in
            self?.becomeQuiet()
        }
    }

    private func becomeQuiet() {
        guard case .active = state else { return }
        state = .quiet
        cancelTimer = schedule(Self.quietDisplay) { [weak self] in
            guard let self, self.state == .quiet else { return }
            self.cancelTimer = nil
            self.state = .idle
        }
    }
}

extension ComputerActivityTracker {
    static func live() -> ComputerActivityTracker {
        ComputerActivityTracker(schedule: { delay, action in
            let item = DispatchWorkItem {
                MainActor.assumeIsolated { action() }
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: item)
            return { item.cancel() }
        })
    }
}

// MARK: - Notice window

/// Small click-through notice in the top-right corner of the main screen.
///
/// It never takes clicks: ChatGPT may click anywhere, and a button here could
/// be hit by its own clicks. Stopping is in the menu bar. It reuses the agent
/// cursor's window title, so `list_windows` hides it from ChatGPT, and it is
/// kept out of screenshots like the cursor.
@MainActor
final class ComputerActivityNotice {
    private var panel: NSPanel?
    private var model = ComputerActivityNoticeModel()

    func update(_ state: ComputerActivityState) {
        switch state {
        case .idle:
            panel?.orderOut(nil)
        case .active, .quiet:
            model.state = state
            let panel = self.panel ?? makePanel()
            self.panel = panel
            position(panel)
            panel.orderFrontRegardless()
        }
    }

    private func makePanel() -> NSPanel {
        let panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 320, height: 64),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        panel.title = ComputerOverlayPanel.windowTitle
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.ignoresMouseEvents = true
        panel.sharingType = .none
        panel.isFloatingPanel = true
        panel.hidesOnDeactivate = false
        panel.isReleasedWhenClosed = false
        panel.isExcludedFromWindowsMenu = true
        panel.animationBehavior = .none
        panel.level = .statusBar
        panel.collectionBehavior = ComputerOverlayPanel.collectionBehaviorFlags
        let host = NSHostingView(rootView: ComputerActivityNoticeView(model: model))
        host.frame = panel.contentRect(forFrameRect: panel.frame)
        panel.contentView = host
        return panel
    }

    private func position(_ panel: NSPanel) {
        guard let screen = NSScreen.screens.first(where: { $0.frame.origin == .zero }) ?? NSScreen.main else { return }
        if let host = panel.contentView {
            let size = host.fittingSize
            panel.setContentSize(NSSize(width: max(size.width, 240), height: max(size.height, 48)))
        }
        let visible = screen.visibleFrame
        let margin: CGFloat = 12
        panel.setFrameOrigin(NSPoint(
            x: visible.maxX - panel.frame.width - margin,
            y: visible.maxY - panel.frame.height - margin
        ))
    }
}

@MainActor
final class ComputerActivityNoticeModel: ObservableObject {
    @Published var state: ComputerActivityState = .idle
}

struct ComputerActivityNoticeView: View {
    @ObservedObject var model: ComputerActivityNoticeModel

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: isActive ? "cursorarrow.motionlines" : "checkmark.circle.fill")
                .font(.system(size: 18, weight: .semibold))
                .foregroundStyle(isActive ? Color.orange : Color.green)
            VStack(alignment: .leading, spacing: 2) {
                Text(L10n.string(isActive ? "computer.activity.activeTitle" : "computer.activity.quietTitle"))
                    .font(.system(size: 13, weight: .semibold))
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text(detail(at: context.date))
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .frame(width: 300, alignment: .leading)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private var isActive: Bool {
        if case .active = model.state { return true }
        return false
    }

    private func detail(at date: Date) -> String {
        switch model.state {
        case .active(let last):
            let seconds = max(0, Int(date.timeIntervalSince(last)))
            return L10n.string("computer.activity.activeDetail", Int64(seconds))
        case .quiet:
            return L10n.string("computer.activity.quietDetail", Int64(ComputerActivityTracker.idleDelay))
        case .idle:
            return ""
        }
    }
}
