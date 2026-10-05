import Foundation
import XCTest
@testable import ChadexApp

/// App-side behaviour of the launch-time runtime warm-up, driven against a fake
/// helper that logs every request method it receives (and every process start).
@MainActor
final class RuntimePrewarmTests: XCTestCase {
    private struct Fixture {
        let model: AppModel
        let log: URL
    }

    private func makeFixture(
        prepareServiceOnLaunch: Bool?,
        withProject: Bool = true,
        slowPrewarm: Bool = false
    ) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-prewarm-test-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }

        let log = root.appendingPathComponent("helper.log")
        let executable = root.appendingPathComponent("fake-helper")
        let script = """
        #!/bin/sh
        LOG='\(log.path)'
        echo START >> "$LOG"
        SNAP='{"phase":"stopped","selected_project":null,"tunnel_ready":false,"chat_gpt_connected":false,"chat_gpt_verified_for_selected_project":false,"last_verified_at_ms":null,"current_operation":null,"error":null,"activity_sequence":0,"state_revision":1}'
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\\([^"]*\\)".*/\\1/p')
          method=$(printf '%s' "$line" | sed -n 's/.*"method":"\\([^"]*\\)".*/\\1/p')
          echo "$method" >> "$LOG"
          if [ "$method" = "prewarmRuntime" ] && [ "\(slowPrewarm ? "1" : "0")" = "1" ]; then
            ( sleep 4; printf '{"protocol_version":1,"request_id":"%s","result":%s}\\n' "$id" "$SNAP" ) &
          else
            printf '{"protocol_version":1,"request_id":"%s","result":%s}\\n' "$id" "$SNAP"
          fi
          if [ "$method" = "shutdown" ]; then exit 0; fi
        done
        """
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o755))],
            ofItemAtPath: executable.path
        )

        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": root.path])
        let project = ProjectRecord(name: "Project", path: root.path)
        var preferences = ChadexPreferences()
        preferences.projects = withProject ? [project] : []
        preferences.selectedProjectID = withProject ? project.id : nil
        preferences.prepareServiceOnLaunch = prepareServiceOnLaunch
        try store.save(preferences)

        let model = AppModel(
            helper: HelperClient(executableURL: executable),
            store: store,
            autostart: false
        )
        return Fixture(model: model, log: log)
    }

    private func logLines(_ log: URL) -> [String] {
        ((try? String(contentsOf: log, encoding: .utf8)) ?? "")
            .split(separator: "\n")
            .map(String.init)
    }

    private func waitUntil(
        timeout: TimeInterval = 3,
        _ condition: () -> Bool
    ) async -> Bool {
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        while ProcessInfo.processInfo.systemUptime < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return condition()
    }

    func testPrewarmRunsByDefaultWhenSettingWasNeverChosen() async throws {
        let fixture = try makeFixture(prepareServiceOnLaunch: nil)
        defer { Task { await fixture.model.shutdown() } }

        fixture.model.startRuntimePrewarmIfEnabled()
        XCTAssertNotNil(fixture.model.runtimePrewarmTask)
        await fixture.model.runtimePrewarmTask?.value

        XCTAssertEqual(logLines(fixture.log).filter { $0 == "prewarmRuntime" }.count, 1)
        XCTAssertNil(fixture.model.runtimePrewarmTask)
    }

    func testPrewarmIsStartedOnlyOnce() async throws {
        let fixture = try makeFixture(prepareServiceOnLaunch: true, slowPrewarm: true)
        defer { Task { await fixture.model.shutdown() } }

        fixture.model.startRuntimePrewarmIfEnabled()
        fixture.model.startRuntimePrewarmIfEnabled()
        let sent = await waitUntil { self.logLines(fixture.log).contains("prewarmRuntime") }
        XCTAssertTrue(sent)
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertEqual(logLines(fixture.log).filter { $0 == "prewarmRuntime" }.count, 1)
    }

    func testPrewarmIsSkippedWhenSettingIsOff() async throws {
        let fixture = try makeFixture(prepareServiceOnLaunch: false)
        defer { Task { await fixture.model.shutdown() } }

        fixture.model.startRuntimePrewarmIfEnabled()
        XCTAssertNil(fixture.model.runtimePrewarmTask)
        try await Task.sleep(for: .milliseconds(200))
        XCTAssertFalse(logLines(fixture.log).contains("prewarmRuntime"))
        XCTAssertFalse(
            FileManager.default.fileExists(atPath: fixture.log.path),
            "no helper process should even be started for a disabled warm-up"
        )
    }

    func testPrewarmIsSkippedWithoutASelectedProject() async throws {
        let fixture = try makeFixture(prepareServiceOnLaunch: true, withProject: false)
        fixture.model.startRuntimePrewarmIfEnabled()
        XCTAssertNil(fixture.model.runtimePrewarmTask)
    }

    func testTurningTheSettingOffPersistsAndGatesPrewarm() async throws {
        let fixture = try makeFixture(prepareServiceOnLaunch: nil)
        XCTAssertTrue(fixture.model.preferences.prepareServiceOnLaunchEnabled)
        fixture.model.setPrepareServiceOnLaunch(false)
        XCTAssertFalse(fixture.model.preferences.prepareServiceOnLaunchEnabled)
        fixture.model.startRuntimePrewarmIfEnabled()
        XCTAssertNil(fixture.model.runtimePrewarmTask)
    }

    func testShutdownCancelsInFlightPrewarmAndNeitherRefreshesNorRelaunchesHelper() async throws {
        let fixture = try makeFixture(prepareServiceOnLaunch: true, slowPrewarm: true)

        fixture.model.startRuntimePrewarmIfEnabled()
        let inFlight = await waitUntil { self.logLines(fixture.log).contains("prewarmRuntime") }
        XCTAssertTrue(inFlight, "the prewarm request should reach the helper")

        let started = ProcessInfo.processInfo.systemUptime
        await fixture.model.shutdown()
        XCTAssertLessThan(
            ProcessInfo.processInfo.systemUptime - started,
            3.5,
            "quit must not wait for the warm-up to finish"
        )
        XCTAssertTrue(fixture.model.isShuttingDown)
        XCTAssertNil(fixture.model.runtimePrewarmTask)

        // Give a stray follow-up refresh time to show up.
        try await Task.sleep(for: .milliseconds(400))
        let lines = logLines(fixture.log)
        XCTAssertEqual(lines.filter { $0 == "START" }.count, 1, "helper must not be relaunched during quit")
        XCTAssertFalse(lines.contains("getStatus"), "no refresh after shutdown began")

        // Nothing may start after shutdown either.
        fixture.model.startRuntimePrewarmIfEnabled()
        XCTAssertNil(fixture.model.runtimePrewarmTask)
        await fixture.model.refreshStatus(force: true)
        XCTAssertEqual(logLines(fixture.log).filter { $0 == "START" }.count, 1)
    }

    func testOtherRequestsAreNotQueuedBehindTheAppSidePrewarm() async throws {
        let fixture = try makeFixture(prepareServiceOnLaunch: true, slowPrewarm: true)
        defer { Task { await fixture.model.shutdown() } }

        fixture.model.startRuntimePrewarmIfEnabled()
        let inFlight = await waitUntil { self.logLines(fixture.log).contains("prewarmRuntime") }
        XCTAssertTrue(inFlight)

        // The helper decides whether a request joins or cancels the warm-up;
        // the app must hand the request over immediately rather than waiting
        // for the (up to 120s) prewarm to finish first.
        fixture.model.stopLocalService()
        let reached = await waitUntil(timeout: 2) {
            self.logLines(fixture.log).contains("stopLocalService")
        }
        XCTAssertTrue(reached, "stopLocalService must not wait behind the prewarm")
        XCTAssertNotNil(fixture.model.runtimePrewarmTask, "prewarm is still pending")
    }
}

@MainActor
final class PollWakerTests: XCTestCase {
    func testWaitReturnsAfterTimeoutWithoutSignal() async {
        let waker = PollWaker()
        let started = ProcessInfo.processInfo.systemUptime
        await waker.wait(seconds: 0.15)
        let elapsed = ProcessInfo.processInfo.systemUptime - started
        XCTAssertGreaterThanOrEqual(elapsed, 0.1)
        XCTAssertLessThan(elapsed, 1.0)
    }

    func testSignalWakesALongWaitEarly() async {
        let waker = PollWaker()
        let started = ProcessInfo.processInfo.systemUptime
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(50))
            waker.signal()
        }
        await waker.wait(seconds: 30)
        XCTAssertLessThan(ProcessInfo.processInfo.systemUptime - started, 2.0)
    }

    func testSignalBeforeWaitMakesNextWaitReturnImmediately() async {
        let waker = PollWaker()
        waker.signal()
        let started = ProcessInfo.processInfo.systemUptime
        await waker.wait(seconds: 30)
        XCTAssertLessThan(ProcessInfo.processInfo.systemUptime - started, 1.0)
        // The signal is consumed: a second wait honours its timeout.
        let secondStart = ProcessInfo.processInfo.systemUptime
        await waker.wait(seconds: 0.15)
        XCTAssertGreaterThanOrEqual(ProcessInfo.processInfo.systemUptime - secondStart, 0.1)
    }

    func testCancellationEndsAWait() async {
        let waker = PollWaker()
        let waiting = Task { @MainActor in
            await waker.wait(seconds: 30)
        }
        try? await Task.sleep(for: .milliseconds(50))
        let started = ProcessInfo.processInfo.systemUptime
        waiting.cancel()
        await waiting.value
        XCTAssertLessThan(ProcessInfo.processInfo.systemUptime - started, 2.0)
    }

    func testStaleTimerFromAnEarlierWaitDoesNotWakeALaterOne() async {
        let waker = PollWaker()
        waker.signal()
        await waker.wait(seconds: 0.2) // returns at once; its timer is cancelled
        let started = ProcessInfo.processInfo.systemUptime
        await waker.wait(seconds: 0.5)
        XCTAssertGreaterThanOrEqual(ProcessInfo.processInfo.systemUptime - started, 0.4)
    }
}
