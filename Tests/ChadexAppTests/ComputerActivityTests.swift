import XCTest
@testable import ChadexApp

@MainActor
final class ComputerActivityTests: XCTestCase {
    private final class Clock {
        var now = Date(timeIntervalSince1970: 1_800_000_000)
        var timers: [(fire: Date, action: @MainActor () -> Void, cancelled: Bool)] = []

        func advance(_ seconds: TimeInterval) {
            now += seconds
            // Fire due timers in order; a timer may schedule another.
            while let index = timers.indices.filter({ !timers[$0].cancelled && timers[$0].fire <= now })
                .min(by: { timers[$0].fire < timers[$1].fire }) {
                timers[index].cancelled = true
                MainActor.assumeIsolated { timers[index].action() }
            }
        }
    }

    private func makeTracker() -> (ComputerActivityTracker, Clock, () -> [ComputerActivityState]) {
        let clock = Clock()
        var seen: [ComputerActivityState] = []
        let tracker = ComputerActivityTracker(now: { clock.now }, schedule: { delay, action in
            clock.timers.append((clock.now + delay, action, false))
            let index = clock.timers.count - 1
            return { clock.timers[index].cancelled = true }
        })
        tracker.onChange = { seen.append($0) }
        return (tracker, clock, { seen })
    }

    private func event(_ phase: ComputerOverlayPhase, at clock: Clock, ageMs: UInt64 = 0, reason: String? = nil)
        -> ComputerOverlayEventData {
        let nowMs = UInt64(clock.now.timeIntervalSince1970 * 1000)
        return ComputerOverlayEventData(
            phase: phase, actionId: 1, action: "key", target: phase == .willAct ? ComputerOverlayTarget.none : nil,
            reason: reason, emittedAtMs: nowMs - ageMs
        )
    }

    func testAnActionShowsActiveThenQuietThenIdle() {
        let (tracker, clock, _) = makeTracker()
        tracker.handle(event(.willAct, at: clock))
        XCTAssertEqual(tracker.state, .active(lastActionAt: clock.now))

        clock.advance(ComputerActivityTracker.idleDelay - 1)
        guard case .active = tracker.state else { return XCTFail("still active before the idle delay") }
        clock.advance(1)
        XCTAssertEqual(tracker.state, .quiet)
        clock.advance(ComputerActivityTracker.quietDisplay)
        XCTAssertEqual(tracker.state, .idle)
    }

    func testEachActionRestartsTheIdleDelay() {
        let (tracker, clock, _) = makeTracker()
        tracker.handle(event(.willAct, at: clock))
        clock.advance(10)
        tracker.handle(event(.finished, at: clock))
        let last = clock.now
        clock.advance(10)
        XCTAssertEqual(tracker.state, .active(lastActionAt: last), "10 s after the latest action it is still active")
        clock.advance(5)
        XCTAssertEqual(tracker.state, .quiet)
    }

    func testAnActionDuringQuietLightsItUpAgain() {
        let (tracker, clock, _) = makeTracker()
        tracker.handle(event(.willAct, at: clock))
        clock.advance(ComputerActivityTracker.idleDelay)
        XCTAssertEqual(tracker.state, .quiet)
        tracker.handle(event(.willAct, at: clock))
        XCTAssertEqual(tracker.state, .active(lastActionAt: clock.now))
        clock.advance(ComputerActivityTracker.quietDisplay)
        guard case .active = tracker.state else { return XCTFail("the old quiet timer must not end the new activity") }
    }

    func testClearEndsAtOnceButOverflowDoesNot() {
        let (tracker, clock, _) = makeTracker()
        tracker.handle(event(.willAct, at: clock))
        tracker.handle(event(.clear, at: clock, reason: "overflow"))
        guard case .active = tracker.state else { return XCTFail("overflow only lost frames") }
        tracker.handle(event(.clear, at: clock, reason: "stopped"))
        XCTAssertEqual(tracker.state, .idle)
        clock.advance(60)
        XCTAssertEqual(tracker.state, .idle)
    }

    func testStaleEventsAreIgnoredAndResetIsImmediate() {
        let (tracker, clock, seen) = makeTracker()
        tracker.handle(event(.willAct, at: clock, ageMs: ComputerActivityTracker.staleEventLimitMs + 1))
        XCTAssertEqual(tracker.state, .idle)
        XCTAssertTrue(seen().isEmpty)
        tracker.handle(event(.willAct, at: clock))
        tracker.reset()
        XCTAssertEqual(tracker.state, .idle)
        XCTAssertEqual(seen().count, 2)
    }
}
