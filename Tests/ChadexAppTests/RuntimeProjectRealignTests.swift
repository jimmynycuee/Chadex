import Foundation
import XCTest
@testable import ChadexApp

/// When the local runtime serves another project than the selected one
/// (`project_runtime_mismatch`), the Skills refresh re-activates the selected
/// project once through `realignLocalProject` and retries, instead of only
/// showing the error. The realignment never touches the tunnel, blocks other
/// connection work while it runs, and waits for a ready runtime.
@MainActor
final class RuntimeProjectRealignTests: XCTestCase {
    private struct Fixture {
        let model: AppModel
        let root: URL
        let log: URL

        /// Makes the fake runtime report ready from now on.
        func makeRuntimeReady() throws {
            try Data().write(to: root.appendingPathComponent("ready"))
        }
    }

    /// - healsAfterRealign: the fake runtime serves the selection once
    ///   `realignLocalProject` ran; otherwise the mismatch persists.
    /// - realignFails: `realignLocalProject` answers with an error.
    /// - runtimeReady: whether the runtime is ready from the start.
    /// - slowRealign / slowPrewarm: that request answers after ~1.5s.
    private func makeFixture(
        healsAfterRealign: Bool = true,
        realignFails: Bool = false,
        runtimeReady: Bool = true,
        slowRealign: Bool = false,
        slowPrewarm: Bool = false
    ) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-realign-test-\(UUID().uuidString)", isDirectory: true)
        let project = root.appendingPathComponent("project", isDirectory: true)
        let other = root.appendingPathComponent("other", isDirectory: true)
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: other, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        if runtimeReady {
            try Data().write(to: root.appendingPathComponent("ready"))
        }

        let log = root.appendingPathComponent("helper.log")
        let executable = root.appendingPathComponent("fake-helper")
        let script = #"""
        #!/bin/sh
        ROOT='\#(root.path)'
        LOG="$ROOT/helper.log"
        HEALS='\#(healsAfterRealign ? "1" : "0")'
        FAILS='\#(realignFails ? "1" : "0")'
        SLOW_REALIGN='\#(slowRealign ? "1" : "0")'
        SLOW_PREWARM='\#(slowPrewarm ? "1" : "0")'
        snap() {
          if [ -f "$ROOT/ready" ]; then R=true; else R=false; fi
          printf '{"phase":"waiting_for_chatgpt_verification","selected_project":null,"tunnel_ready":true,"chat_gpt_connected":false,"chat_gpt_verified_for_selected_project":false,"last_verified_at_ms":null,"current_operation":null,"error":null,"activity_sequence":0,"state_revision":1,"runtime_status":{"runtime_configured":true,"runtime_ready":%s,"needs_attention":false,"summary":"","next_action":null,"summary_kind":"","server":"","runner":"","exposure":"","project":""}}' "$R"
        }
        reply() { printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$1" "$2"; }
        CATALOG='{"project":"p","catalog_revision":"r","total_count":0,"returned_count":0,"skills":[],"invalid_count":0,"diagnostics":[],"discovery_truncated":false}'
        INVENTORY='{"project":"p","total_count":0,"skills":[]}'
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
          method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
          echo "$method" >> "$LOG"
          case "$method" in
            realignLocalProject)
              if [ "$FAILS" = "1" ]; then
                printf '{"protocol_version":1,"request_id":"%s","error":{"code":"runner_offline","message":"offline"}}\n' "$id"
              else
                if [ -f "$ROOT/ready" ]; then touch "$ROOT/realigned"; fi
                if [ "$SLOW_REALIGN" = "1" ]; then
                  ( sleep 1.5; reply "$id" "$(snap)" ) &
                else
                  reply "$id" "$(snap)"
                fi
              fi ;;
            prewarmRuntime)
              if [ "$SLOW_PREWARM" = "1" ]; then
                ( sleep 1.5; reply "$id" "$(snap)" ) &
              else
                reply "$id" "$(snap)"
              fi ;;
            getSkillCatalog)
              reply "$id" "$CATALOG" ;;
            getSkillInventory)
              if [ "$HEALS" = "1" ] && [ -f "$ROOT/realigned" ]; then
                reply "$id" "$INVENTORY"
              else
                printf '{"protocol_version":1,"request_id":"%s","error":{"code":"project_runtime_mismatch","message":"The local runtime is active for a different project"}}\n' "$id"
              fi ;;
            *)
              reply "$id" "$(snap)" ;;
          esac
          if [ "$method" = "shutdown" ]; then exit 0; fi
        done
        """#
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o755))],
            ofItemAtPath: executable.path
        )

        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": root.path])
        let first = ProjectRecord(name: "Project", path: project.path)
        let second = ProjectRecord(name: "Other", path: other.path)
        var preferences = ChadexPreferences()
        preferences.projects = [first, second]
        preferences.selectedProjectID = first.id
        try store.save(preferences)
        let model = AppModel(helper: HelperClient(executableURL: executable), store: store, autostart: false)
        return Fixture(model: model, root: root, log: log)
    }

    private func count(_ method: String, _ fixture: Fixture) -> Int {
        ((try? String(contentsOf: fixture.log, encoding: .utf8)) ?? "")
            .split(separator: "\n").filter { $0 == method }.count
    }

    private func waitUntil(timeout: TimeInterval = 3, _ condition: () -> Bool) async -> Bool {
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        while ProcessInfo.processInfo.systemUptime < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return condition()
    }

    func testMismatchRealignsTheSelectedProjectWithoutTouchingTheTunnel() async throws {
        let fixture = try makeFixture()
        await fixture.model.refreshStatus(force: true)
        await fixture.model.shutdown()

        XCTAssertEqual(count("realignLocalProject", fixture), 1)
        XCTAssertEqual(count("getSkillInventory", fixture), 2, "inventory is retried after the realignment")
        for method in ["switchLocalProject", "stopTunnel", "disconnectAI", "connectChatGPT"] {
            XCTAssertEqual(count(method, fixture), 0, "\(method) must not be part of a realignment")
        }
        XCTAssertNil(fixture.model.skillsError)
        XCTAssertNotNil(fixture.model.skillInventory)
        XCTAssertFalse(fixture.model.isRealigningRuntimeProject)
    }

    func testFailedRealignLeavesTheConnectionAloneAndIsNotRetried() async throws {
        let fixture = try makeFixture(realignFails: true)
        await fixture.model.refreshStatus(force: true)
        await fixture.model.refreshSkills()
        await fixture.model.shutdown()

        XCTAssertEqual(count("realignLocalProject", fixture), 1, "no realignment loop")
        for method in ["switchLocalProject", "stopTunnel", "disconnectAI"] {
            XCTAssertEqual(count(method, fixture), 0, "\(method) must not follow a failed realignment")
        }
        XCTAssertTrue(fixture.model.snapshot.tunnelReady)
        XCTAssertNil(fixture.model.presentedError, "a background realignment never raises an alert")
        XCTAssertEqual(fixture.model.skillsError, L10n.string("skills.error.projectRuntimeMismatch"))
    }

    func testPersistentMismatchIsRealignedOnlyOncePerSelection() async throws {
        let fixture = try makeFixture(healsAfterRealign: false)
        await fixture.model.refreshStatus(force: true)
        await fixture.model.refreshSkills()
        await fixture.model.shutdown()

        XCTAssertEqual(count("realignLocalProject", fixture), 1, "no realignment loop")
        XCTAssertEqual(fixture.model.skillsError, L10n.string("skills.error.projectRuntimeMismatch"))
    }

    func testRuntimeNotReadyDoesNotUseUpTheRealignment() async throws {
        let fixture = try makeFixture(runtimeReady: false)
        await fixture.model.refreshStatus(force: true)
        XCTAssertEqual(fixture.model.snapshot.runtimeReady, false)
        XCTAssertEqual(count("realignLocalProject", fixture), 0, "nothing to realign before the runtime is ready")
        XCTAssertNil(fixture.model.runtimeProjectRealignAttemptedID)

        try fixture.makeRuntimeReady()
        await fixture.model.refreshStatus(force: true)
        await fixture.model.shutdown()

        XCTAssertEqual(count("realignLocalProject", fixture), 1, "the attempt is still available once ready")
        XCTAssertNil(fixture.model.skillsError)
    }

    func testRealignBlocksConnectionWorkAndProjectSwitches() async throws {
        let fixture = try makeFixture(slowRealign: true)
        let model = fixture.model
        let refresh = Task { await model.refreshStatus(force: true) }
        let busy = await waitUntil { model.isRealigningRuntimeProject }
        XCTAssertTrue(busy, "the realignment should be observed in flight")

        XCTAssertTrue(model.connectionActionInFlight)
        XCTAssertTrue(model.hasUpdateBlockingWork)
        let other = try XCTUnwrap(model.preferences.projects.last?.id)
        let switched = await model.selectProject(other)
        XCTAssertFalse(switched, "project switches wait for the realignment")
        model.primaryAction()
        try await Task.sleep(for: .milliseconds(150))
        for method in ["disconnectAI", "connectChatGPT", "switchLocalProject"] {
            XCTAssertEqual(count(method, fixture), 0, "\(method) must wait for the realignment")
        }

        await refresh.value
        await model.shutdown()
        XCTAssertFalse(model.isRealigningRuntimeProject)
        XCTAssertFalse(model.connectionActionInFlight)
    }

    func testNoRealignmentWhileTheLaunchWarmUpOwnsTheRuntime() async throws {
        let fixture = try makeFixture(slowPrewarm: true)
        let model = fixture.model
        model.startRuntimePrewarmIfEnabled()
        XCTAssertNotNil(model.runtimePrewarmTask)
        await model.refreshStatus(force: true)
        XCTAssertEqual(model.snapshot.runtimeReady, true)
        XCTAssertNotNil(model.runtimePrewarmTask, "warm-up still in flight")
        XCTAssertEqual(count("realignLocalProject", fixture), 0)
        XCTAssertNil(model.runtimeProjectRealignAttemptedID, "the attempt is kept for after the warm-up")
        await model.runtimePrewarmTask?.value
        await model.shutdown()
    }
}
