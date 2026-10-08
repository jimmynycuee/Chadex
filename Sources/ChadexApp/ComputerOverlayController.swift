import Foundation

/// What the controller asks a presenter to draw.
struct ComputerOverlayPresentation: Equatable, Sendable {
    var action: OverlayActionKind
    var layout: OverlayLayout
    var badge: OverlayBadge?
}

/// The drawing side of the overlay. The production implementation owns an
/// `NSPanel`; tests use a recorder.
@MainActor
protocol ComputerOverlayPresenting: AnyObject {
    func present(_ presentation: ComputerOverlayPresentation, plan: OverlayAnimationPlan)
    func showOutcome(_ outcome: OverlayOutcomeKind)
    func hide(fadeDuration: TimeInterval)
    func hideImmediately()
}

/// Everything the controller reads from the outside world, injectable for tests.
struct ComputerOverlayEnvironment {
    var screens: @MainActor () -> [OverlayScreen]
    /// Height of the primary display for CG -> AppKit conversion; nil when AppKit
    /// and CoreGraphics disagree (screen configuration is changing).
    var primaryHeight: @MainActor () -> CGFloat?
    var reduceMotion: @MainActor () -> Bool
    /// Wall clock, epoch milliseconds (compared with the event's `emitted_at_ms`).
    var nowMs: @MainActor () -> UInt64
    /// Monotonic seconds.
    var uptime: @MainActor () -> TimeInterval
    /// Run `action` after `delay`; the returned closure cancels it.
    var schedule: @MainActor (TimeInterval, @escaping @MainActor () -> Void) -> (() -> Void)
}

/// Turns helper overlay events into presenter calls: staleness checks, screen
/// resolution, motion plan, and auto-hide timing. Purely best effort: any event
/// it cannot place exactly is dropped, never guessed.
@MainActor
final class ComputerOverlayController {
    private let presenter: ComputerOverlayPresenting
    private let environment: ComputerOverlayEnvironment

    private(set) var isEnabled: Bool
    private(set) var visibility: OverlayVisibility = .hidden
    private(set) var droppedEventCount = 0
    private var currentActionID: UInt64?
    private var lastEventUptime: TimeInterval?
    private var currentFadeDuration = OverlayAnimationPlan.fadeOut
    private var cancelPendingHide: (() -> Void)?

    init(
        presenter: ComputerOverlayPresenting,
        environment: ComputerOverlayEnvironment,
        isEnabled: Bool = true
    ) {
        self.presenter = presenter
        self.environment = environment
        self.isEnabled = isEnabled
    }

    func setEnabled(_ enabled: Bool) {
        isEnabled = enabled
        if !enabled { hideImmediately() }
    }

    func handle(_ event: HelperEvent) {
        switch event {
        case .computerOverlay(let data):
            handle(data)
        }
    }

    func handle(_ data: ComputerOverlayEventData) {
        guard isEnabled else { return }
        switch data.phase {
        case .willAct: handleWillAct(data)
        case .finished: handleFinished(data)
        case .clear: hideImmediately()
        }
    }

    /// Hide at once: Stop, helper exit, sleep, screen changes, switch off, `clear` frames.
    func hideImmediately() {
        cancelPendingHide?()
        cancelPendingHide = nil
        currentActionID = nil
        visibility = .hidden
        presenter.hideImmediately()
    }

    // MARK: - Phases

    private func handleWillAct(_ data: ComputerOverlayEventData) {
        guard data.v == ComputerOverlayEventData.supportedVersion,
              data.space == ComputerOverlayEventData.supportedSpace,
              let actionID = data.actionId,
              let target = data.target,
              let emittedAtMs = data.emittedAtMs
        else {
            drop()
            return
        }
        // An event that sat in a queue is a lie about where the agent is now.
        let nowMs = environment.nowMs()
        if nowMs > emittedAtMs, nowMs - emittedAtMs > ComputerOverlayTiming.staleEventLimitMs {
            drop()
            return
        }
        guard let anchor = ComputerOverlayGeometry.anchor(of: target),
              let primaryHeight = environment.primaryHeight(),
              let screen = ComputerOverlayGeometry.resolveScreen(
                  display: data.display,
                  anchor: anchor,
                  screens: environment.screens()
              ),
              let layout = ComputerOverlayGeometry.layout(
                  target: target,
                  screen: screen,
                  primaryHeight: primaryHeight
              )
        else {
            // No usable geometry (for example the AX frame could not be read, or the
            // screen layout changed): draw nothing, and do not leave an old marker behind.
            drop()
            return
        }

        let action = OverlayActionKind(wire: data.action)
        let uptime = environment.uptime()
        let sameScreen: Bool
        if case .shown(let screenID) = visibility {
            sameScreen = screenID == layout.screenID
        } else {
            sameScreen = false
        }
        let plan = OverlayAnimationPlan.plan(
            action: action,
            reduceMotion: environment.reduceMotion(),
            previous: visibility,
            sameScreen: sameScreen,
            sinceLast: lastEventUptime.map { uptime - $0 } ?? .infinity
        )

        cancelPendingHide?()
        presenter.present(
            ComputerOverlayPresentation(
                action: action,
                layout: layout,
                badge: OverlayBadge.badge(for: action, key: data.key)
            ),
            plan: plan
        )
        visibility = .shown(screenID: layout.screenID)
        currentActionID = actionID
        lastEventUptime = uptime
        currentFadeDuration = plan.fadeOutDuration
        scheduleFadeHide(after: ComputerOverlayTiming.ttl(milliseconds: data.ttlMs))
    }

    private func handleFinished(_ data: ComputerOverlayEventData) {
        // Only the action that is on screen can finish it.
        guard let actionID = data.actionId, actionID == currentActionID else { return }
        let outcome = OverlayOutcomeKind(wire: data.outcome)
        presenter.showOutcome(outcome)
        scheduleFadeHide(after: ComputerOverlayTiming.retention(after: outcome))
    }

    private func drop() {
        droppedEventCount += 1
        if visibility != .hidden { hideImmediately() }
    }

    private func scheduleFadeHide(after delay: TimeInterval) {
        cancelPendingHide?()
        cancelPendingHide = environment.schedule(delay) { [weak self] in
            self?.fadeOut()
        }
    }

    private func fadeOut() {
        cancelPendingHide = nil
        guard visibility != .hidden else { return }
        currentActionID = nil
        visibility = .hidden
        presenter.hide(fadeDuration: currentFadeDuration)
    }
}
