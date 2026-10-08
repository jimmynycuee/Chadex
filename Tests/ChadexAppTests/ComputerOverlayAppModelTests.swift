import Foundation
import XCTest
@testable import ChadexApp

/// AppModel wiring for the cursor overlay: the helper is told the user's choice,
/// the choice persists, events reach the controller and Stop hides at once.
@MainActor
final class ComputerOverlayAppModelTests: XCTestCase {
    private final class CountingPresenter: ComputerOverlayPresenting {
        private(set) var presented = 0
        private(set) var hiddenImmediately = 0
        func present(_ presentation: ComputerOverlayPresentation, plan: OverlayAnimationPlan) { presented += 1 }
        func showOutcome(_ outcome: OverlayOutcomeKind) {}
        func hide(fadeDuration: TimeInterval) {}
        func hideImmediately() { hiddenImmediately += 1 }
    }

    private struct Fixture {
        let model: AppModel
        let store: ProjectStore
        let log: URL
        let presenter: CountingPresenter
    }

    private func makeFixture(
        overlayPreference: Bool? = nil,
        eventLine: String? = nil
    ) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-overlay-model-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }

        let log = root.appendingPathComponent("helper.log")
        let executable = root.appendingPathComponent("fake-helper")
        let emit = eventLine.map { "printf '%s\\n' '\($0)'" } ?? ":"
        let script = """
        #!/bin/sh
        SAFETY='{"mode":"ask_before_control","stopped":false,"generation":1,"pending_approvals":[],"audit":[]}'
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\\([^"]*\\)".*/\\1/p')
          method=$(printf '%s' "$line" | sed -n 's/.*"method":"\\([^"]*\\)".*/\\1/p')
          printf '%s\\n' "$line" >> '\(log.path)'
          case "$method" in
            setComputerOverlayEvents)
              \(emit)
              printf '{"protocol_version":1,"request_id":"%s","result":{"enabled":true,"runner_channel":"detached"}}\\n' "$id" ;;
            *) printf '{"protocol_version":1,"request_id":"%s","result":%s}\\n' "$id" "$SAFETY" ;;
          esac
          if [ "$method" = "shutdown" ]; then exit 0; fi
        done
        """
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o755))],
            ofItemAtPath: executable.path
        )

        // Isolated preferences: never the real Application Support folder.
        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": root.path])
        var preferences = ChadexPreferences()
        preferences.computerCursorOverlay = overlayPreference
        try store.save(preferences)

        let presenter = CountingPresenter()
        let controller = ComputerOverlayController(
            presenter: presenter,
            environment: .live(),
            isEnabled: preferences.computerCursorOverlayEnabled
        )
        let model = AppModel(
            helper: HelperClient(executableURL: executable),
            store: store,
            overlayController: controller,
            autostart: false
        )
        return Fixture(model: model, store: store, log: log, presenter: presenter)
    }

    private func logged(_ fixture: Fixture, method: String) -> [String] {
        ((try? String(contentsOf: fixture.log, encoding: .utf8)) ?? "")
            .split(separator: "\n").map(String.init)
            .filter { $0.contains("\"method\":\"\(method)\"") }
    }

    private func waitUntil(_ timeout: TimeInterval = 5, _ condition: () -> Bool) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return condition()
    }

    func testStartingTheHelperSyncsTheDefaultOnChoice() async throws {
        let fixture = try makeFixture()
        await fixture.model.refreshComputerSafety() // first request starts the helper
        let synced = await waitUntil { !logged(fixture, method: "setComputerOverlayEvents").isEmpty }
        await fixture.model.shutdown()
        XCTAssertTrue(synced, "a new helper must be told whether to forward overlay events")
        XCTAssertTrue(logged(fixture, method: "setComputerOverlayEvents")[0].contains("\"enabled\":true"))
    }

    func testSavedOffChoiceIsSentToANewHelper() async throws {
        let fixture = try makeFixture(overlayPreference: false)
        XCTAssertFalse(fixture.model.computerCursorOverlayEnabled)
        await fixture.model.refreshComputerSafety()
        let synced = await waitUntil { !logged(fixture, method: "setComputerOverlayEvents").isEmpty }
        await fixture.model.shutdown()
        XCTAssertTrue(synced)
        XCTAssertTrue(logged(fixture, method: "setComputerOverlayEvents")[0].contains("\"enabled\":false"))
    }

    func testTogglePersistsHidesAndTellsTheHelper() async throws {
        let fixture = try makeFixture()
        await fixture.model.setComputerCursorOverlay(false)
        XCTAssertFalse(fixture.model.computerCursorOverlayEnabled)
        XCTAssertEqual(fixture.store.load().computerCursorOverlay, false, "the choice is saved")
        XCTAssertGreaterThan(fixture.presenter.hiddenImmediately, 0, "switching off hides at once")
        let sentOff = logged(fixture, method: "setComputerOverlayEvents").last
        XCTAssertTrue(sentOff?.contains("\"enabled\":false") == true, sentOff ?? "nothing sent")

        await fixture.model.setComputerCursorOverlay(true)
        XCTAssertEqual(fixture.store.load().computerCursorOverlay, true)
        XCTAssertTrue(logged(fixture, method: "setComputerOverlayEvents").last?.contains("\"enabled\":true") == true)
        await fixture.model.shutdown()
    }

    func testStopHidesTheOverlayBeforeAskingTheHelper() async throws {
        let fixture = try makeFixture()
        let before = fixture.presenter.hiddenImmediately
        await fixture.model.stopComputerControl()
        XCTAssertGreaterThan(fixture.presenter.hiddenImmediately, before)
        await fixture.model.shutdown()
    }

    func testOverlayEventFromTheHelperReachesTheController() async throws {
        // The helper answers setComputerOverlayEvents with an event frame, like a
        // runner event forwarded after the App enabled events. emitted_at_ms is
        // "now" so it is not stale; the display is omitted so the point picks a screen.
        let nowMs = UInt64(Date().timeIntervalSince1970 * 1000)
        let line = #"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"seq":1,"phase":"will_act","action_id":1,"action":"click","target":{"kind":"point","x":5.0,"y":5.0},"space":"macos_cg_global_pt","ttl_ms":500,"emitted_at_ms":\#(nowMs)}}"#
        let fixture = try makeFixture(eventLine: line)
        await fixture.model.refreshComputerSafety()
        let presented = await waitUntil { fixture.presenter.presented > 0 }
        await fixture.model.shutdown()
        // Headless CI can have no display; then the controller correctly drops the
        // event instead of guessing. Either way nothing may crash or fail a request.
        if NSScreen.screens.isEmpty {
            XCTAssertFalse(presented)
        } else {
            XCTAssertTrue(presented, "a valid event on a real screen is presented")
        }
    }
}
