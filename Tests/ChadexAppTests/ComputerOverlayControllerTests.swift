import CoreGraphics
import XCTest
@testable import ChadexApp

@MainActor
private final class RecordingPresenter: ComputerOverlayPresenting {
    enum Call: Equatable {
        case present(ComputerOverlayPresentation, OverlayAnimationPlan)
        case outcome(OverlayOutcomeKind)
        case hide(TimeInterval)
        case hideImmediately
    }

    private(set) var calls: [Call] = []

    func present(_ presentation: ComputerOverlayPresentation, plan: OverlayAnimationPlan) {
        calls.append(.present(presentation, plan))
    }

    func showOutcome(_ outcome: OverlayOutcomeKind) { calls.append(.outcome(outcome)) }
    func hide(fadeDuration: TimeInterval) { calls.append(.hide(fadeDuration)) }
    func hideImmediately() { calls.append(.hideImmediately) }

    var presentations: [(ComputerOverlayPresentation, OverlayAnimationPlan)] {
        calls.compactMap {
            if case .present(let presentation, let plan) = $0 { return (presentation, plan) }
            return nil
        }
    }

    func reset() { calls.removeAll() }
}

/// Manual clock and timers: nothing fires unless the test advances time.
@MainActor
private final class FakeWorld {
    var uptime: TimeInterval = 100
    var wallMs: UInt64 = 1_791_500_000_000
    var reduceMotion = false
    var screens = [OverlayScreen(displayID: 1, cgBounds: CGRect(x: 0, y: 0, width: 1512, height: 982))]
    var primaryHeight: CGFloat? = 982
    private var timers: [(id: Int, due: TimeInterval, action: @MainActor () -> Void)] = []
    private var nextTimerID = 0

    var pendingTimerCount: Int { timers.count }

    var environment: ComputerOverlayEnvironment {
        ComputerOverlayEnvironment(
            screens: { [unowned self] in self.screens },
            primaryHeight: { [unowned self] in self.primaryHeight },
            reduceMotion: { [unowned self] in self.reduceMotion },
            nowMs: { [unowned self] in self.wallMs },
            uptime: { [unowned self] in self.uptime },
            schedule: { [unowned self] delay, action in
                let id = self.nextTimerID
                self.nextTimerID += 1
                self.timers.append((id, self.uptime + delay, action))
                return { [weak self] in self?.timers.removeAll { $0.id == id } }
            }
        )
    }

    func advance(_ seconds: TimeInterval) {
        uptime += seconds
        wallMs += UInt64(seconds * 1000)
        let due = timers.filter { $0.due <= uptime }.sorted { $0.due < $1.due }
        timers.removeAll { entry in due.contains { $0.id == entry.id } }
        for entry in due { entry.action() }
    }
}

@MainActor
final class ComputerOverlayControllerTests: XCTestCase {
    private func makeController(
        enabled: Bool = true
    ) -> (ComputerOverlayController, RecordingPresenter, FakeWorld) {
        let presenter = RecordingPresenter()
        let world = FakeWorld()
        let controller = ComputerOverlayController(
            presenter: presenter,
            environment: world.environment,
            isEnabled: enabled
        )
        return (controller, presenter, world)
    }

    private func willAct(
        id: UInt64 = 1,
        action: String = "click",
        target: ComputerOverlayTarget = .point(CGPoint(x: 400, y: 300)),
        display: ComputerOverlayDisplay? = nil,
        ttlMs: Int? = 2000,
        emittedAtMs: UInt64,
        space: String? = "macos_cg_global_pt",
        key: ComputerOverlayKey? = nil
    ) -> ComputerOverlayEventData {
        ComputerOverlayEventData(
            phase: .willAct,
            seq: id * 2,
            actionId: id,
            action: action,
            target: target,
            space: space,
            display: display,
            ttlMs: ttlMs,
            key: key,
            emittedAtMs: emittedAtMs
        )
    }

    private func finished(id: UInt64 = 1, outcome: String = "succeeded") -> ComputerOverlayEventData {
        ComputerOverlayEventData(phase: .finished, seq: id * 2 + 1, actionId: id, space: nil, outcome: outcome)
    }

    private var clear: ComputerOverlayEventData {
        ComputerOverlayEventData(phase: .clear, space: nil, reason: "stopped")
    }

    func testWillActShowsThePanelAtTheMappedPosition() throws {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(emittedAtMs: world.wallMs))
        let (presentation, plan) = try XCTUnwrap(presenter.presentations.first)
        XCTAssertEqual(presentation.action, .click)
        XCTAssertEqual(presentation.layout.style, .pointer)
        XCTAssertEqual(presentation.layout.panelFrame.midX, 400)
        XCTAssertEqual(presentation.layout.panelFrame.midY, 982 - 300)
        XCTAssertTrue(plan.ripple)
        XCTAssertEqual(controller.visibility, .shown(screenID: 1))
    }

    func testFinishedHidesAfterEightHundredMillisecondsOnSuccess() {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(emittedAtMs: world.wallMs))
        world.advance(0.05)
        controller.handle(finished(outcome: "succeeded"))
        XCTAssertEqual(presenter.calls.last, .outcome(.succeeded))
        world.advance(0.79)
        XCTAssertEqual(controller.visibility, .shown(screenID: 1), "still visible before 800 ms")
        world.advance(0.02)
        XCTAssertEqual(controller.visibility, .hidden)
        XCTAssertEqual(presenter.calls.last, .hide(0.2))
    }

    func testFinishedKeepsFailuresForTwelveHundredMilliseconds() {
        for outcome in ["failed", "not_started", "unknown"] {
            let (controller, presenter, world) = makeController()
            controller.handle(willAct(emittedAtMs: world.wallMs))
            controller.handle(finished(outcome: outcome))
            world.advance(0.81)
            XCTAssertEqual(controller.visibility, .shown(screenID: 1), outcome)
            world.advance(0.4)
            XCTAssertEqual(controller.visibility, .hidden, outcome)
            XCTAssertEqual(presenter.calls.last, .hide(0.2), outcome)
        }
    }

    func testWithoutFinishedTheClampedTTLApplies() {
        let (controller, _, world) = makeController()
        controller.handle(willAct(ttlMs: 3000, emittedAtMs: world.wallMs))
        world.advance(2.9)
        XCTAssertEqual(controller.visibility, .shown(screenID: 1))
        world.advance(0.2)
        XCTAssertEqual(controller.visibility, .hidden)

        // Tiny and huge TTLs are clamped to [0.5, 5].
        controller.handle(willAct(id: 2, ttlMs: 10, emittedAtMs: world.wallMs))
        world.advance(0.45)
        XCTAssertEqual(controller.visibility, .shown(screenID: 1))
        world.advance(0.1)
        XCTAssertEqual(controller.visibility, .hidden)

        controller.handle(willAct(id: 3, ttlMs: 60_000, emittedAtMs: world.wallMs))
        world.advance(4.9)
        XCTAssertEqual(controller.visibility, .shown(screenID: 1))
        world.advance(0.2)
        XCTAssertEqual(controller.visibility, .hidden)
    }

    func testANewWillActCancelsThePendingHide() {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(id: 1, emittedAtMs: world.wallMs))
        controller.handle(finished(id: 1))
        world.advance(0.7)
        controller.handle(willAct(id: 2, target: .point(CGPoint(x: 500, y: 400)), emittedAtMs: world.wallMs))
        XCTAssertEqual(world.pendingTimerCount, 1, "only the new action's timer remains")
        world.advance(0.5)
        XCTAssertEqual(controller.visibility, .shown(screenID: 1), "the old hide must not fire")
        XCTAssertFalse(presenter.calls.contains(.hide(0.2)))
    }

    func testFinishedForAnotherActionIsIgnored() {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(id: 7, emittedAtMs: world.wallMs))
        presenter.reset()
        controller.handle(finished(id: 6))
        XCTAssertTrue(presenter.calls.isEmpty)
        XCTAssertEqual(world.pendingTimerCount, 1, "still only the ttl timer")
        controller.handle(finished(id: 7))
        XCTAssertEqual(presenter.calls, [.outcome(.succeeded)])
    }

    func testFinishedAfterHideIsIgnored() {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(emittedAtMs: world.wallMs))
        controller.handle(finished())
        world.advance(1)
        presenter.reset()
        controller.handle(finished())
        XCTAssertTrue(presenter.calls.isEmpty)
    }

    func testStaleEventsAreDroppedButFutureTimestampsAreNot() {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(emittedAtMs: world.wallMs - 1001))
        XCTAssertTrue(presenter.calls.isEmpty)
        XCTAssertEqual(controller.droppedEventCount, 1)
        controller.handle(willAct(emittedAtMs: world.wallMs - 1000))
        XCTAssertEqual(presenter.presentations.count, 1, "exactly 1000 ms is still accepted")
        controller.hideImmediately()
        controller.handle(willAct(id: 2, emittedAtMs: world.wallMs + 5_000))
        XCTAssertEqual(presenter.presentations.count, 2, "a clock ahead of ours is not stale")
    }

    func testClearHidesImmediatelyAndCancelsTimers() {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(emittedAtMs: world.wallMs))
        presenter.reset()
        controller.handle(clear)
        XCTAssertEqual(presenter.calls, [.hideImmediately])
        XCTAssertEqual(controller.visibility, .hidden)
        XCTAssertEqual(world.pendingTimerCount, 0)
    }

    func testDisabledControllerIgnoresEventsAndDisablingHides() {
        let (controller, presenter, world) = makeController(enabled: false)
        controller.handle(willAct(emittedAtMs: world.wallMs))
        XCTAssertTrue(presenter.calls.isEmpty)

        controller.setEnabled(true)
        controller.handle(willAct(emittedAtMs: world.wallMs))
        XCTAssertEqual(presenter.presentations.count, 1)
        controller.setEnabled(false)
        XCTAssertEqual(presenter.calls.last, .hideImmediately)
        XCTAssertEqual(controller.visibility, .hidden)
        presenter.reset()
        controller.handle(willAct(id: 2, emittedAtMs: world.wallMs))
        XCTAssertTrue(presenter.calls.isEmpty)
    }

    func testEventsThatCannotBePlacedExactlyAreDroppedAndHideTheOldMarker() {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(id: 1, emittedAtMs: world.wallMs))
        presenter.reset()

        // Unknown space, wrong version, missing geometry, no target.
        controller.handle(willAct(id: 2, emittedAtMs: world.wallMs, space: "windows_virtual_screen_px"))
        XCTAssertEqual(presenter.calls, [.hideImmediately])
        for event in [
            ComputerOverlayEventData(v: 2, phase: .willAct, actionId: 3, action: "click", target: .point(.zero), emittedAtMs: world.wallMs),
            willAct(id: 4, target: .none, emittedAtMs: world.wallMs),
            willAct(id: 5, target: .point(CGPoint(x: 9000, y: 9000)), emittedAtMs: world.wallMs),
            ComputerOverlayEventData(phase: .willAct, actionId: 6, action: "click", emittedAtMs: world.wallMs),
        ] {
            presenter.reset()
            controller.handle(event)
            XCTAssertTrue(presenter.presentations.isEmpty, "\(event)")
        }
        XCTAssertEqual(controller.droppedEventCount, 5)
    }

    func testDisplayMismatchDropsTheEventInsteadOfGuessing() {
        let (controller, presenter, world) = makeController()
        let wrongBounds = ComputerOverlayDisplay(id: 1, bounds: CGRect(x: 0, y: 0, width: 1440, height: 900))
        controller.handle(willAct(display: wrongBounds, emittedAtMs: world.wallMs))
        XCTAssertTrue(presenter.presentations.isEmpty)
        let unknownDisplay = ComputerOverlayDisplay(id: 42, bounds: CGRect(x: 0, y: 0, width: 1512, height: 982))
        controller.handle(willAct(display: unknownDisplay, emittedAtMs: world.wallMs))
        XCTAssertTrue(presenter.presentations.isEmpty)
        let right = ComputerOverlayDisplay(id: 1, bounds: CGRect(x: 0, y: 0, width: 1512, height: 982))
        controller.handle(willAct(display: right, emittedAtMs: world.wallMs))
        XCTAssertEqual(presenter.presentations.count, 1)
    }

    func testDisagreeingPrimaryHeightDropsTheEvent() {
        let (controller, presenter, world) = makeController()
        world.primaryHeight = nil
        controller.handle(willAct(emittedAtMs: world.wallMs))
        XCTAssertTrue(presenter.presentations.isEmpty)
    }

    func testReduceMotionIsReadAtEveryEventSoItSwitchesLive() throws {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(id: 1, emittedAtMs: world.wallMs))
        world.advance(0.2)
        controller.handle(willAct(id: 2, emittedAtMs: world.wallMs))
        XCTAssertEqual(presenter.presentations.last?.1.glideDuration, 0.16)
        XCTAssertEqual(presenter.presentations.last?.1.ripple, true)

        world.reduceMotion = true
        world.advance(0.2)
        controller.handle(willAct(id: 3, emittedAtMs: world.wallMs))
        let reduced = try XCTUnwrap(presenter.presentations.last?.1)
        XCTAssertNil(reduced.glideDuration)
        XCTAssertFalse(reduced.ripple)
        XCTAssertEqual(reduced.appearDuration, 0)
        XCTAssertEqual(reduced.fadeOutDuration, 0.15)
    }

    func testGlideNeedsAVisibleMarkerOnTheSameScreenWithinTheWindow() throws {
        let (controller, presenter, world) = makeController()
        world.screens.append(OverlayScreen(displayID: 2, cgBounds: CGRect(x: 1512, y: 0, width: 1920, height: 1080)))
        controller.handle(willAct(id: 1, emittedAtMs: world.wallMs))
        world.advance(0.2)
        // Same screen, shortly after: glide.
        controller.handle(willAct(id: 2, emittedAtMs: world.wallMs))
        XCTAssertNotNil(presenter.presentations.last?.1.glideDuration)
        // Other screen: never glide.
        world.advance(0.2)
        controller.handle(willAct(id: 3, target: .point(CGPoint(x: 2000, y: 300)), emittedAtMs: world.wallMs))
        XCTAssertNil(presenter.presentations.last?.1.glideDuration)
        XCTAssertEqual(controller.visibility, .shown(screenID: 2))
        // Marker hidden meanwhile: direct placement.
        world.advance(3)
        XCTAssertEqual(controller.visibility, .hidden)
        controller.handle(willAct(id: 4, target: .point(CGPoint(x: 2000, y: 300)), emittedAtMs: world.wallMs))
        XCTAssertNil(presenter.presentations.last?.1.glideDuration)
    }

    func testKeyAndInputActionsCarryActionTypeBadgesOnly() throws {
        let (controller, presenter, world) = makeController()
        let rect = ComputerOverlayTarget.rect(CGRect(x: 100, y: 100, width: 300, height: 50))
        controller.handle(willAct(
            id: 1,
            action: "key",
            target: rect,
            emittedAtMs: world.wallMs,
            key: ComputerOverlayKey(name: "enter", modifiers: ["command"])
        ))
        XCTAssertEqual(presenter.presentations.last?.0.badge, .symbol("keyboard"))
        controller.handle(willAct(id: 2, action: "input", target: rect, emittedAtMs: world.wallMs))
        XCTAssertEqual(presenter.presentations.last?.0.badge, .symbol("character.cursor.ibeam"))
        XCTAssertEqual(presenter.presentations.last?.0.layout.style, .frame)
    }

    func testUnknownActionsAreDrawnLikeAPointer() throws {
        let (controller, presenter, world) = makeController()
        controller.handle(willAct(
            action: "drag",
            target: .path(from: CGPoint(x: 10, y: 10), to: CGPoint(x: 600, y: 500)),
            emittedAtMs: world.wallMs
        ))
        let (presentation, plan) = try XCTUnwrap(presenter.presentations.first)
        XCTAssertEqual(presentation.action, .other)
        XCTAssertEqual(presentation.layout.style, .pointer)
        XCTAssertEqual(presentation.layout.panelFrame.midX, 600)
        XCTAssertFalse(plan.ripple)
    }
}
