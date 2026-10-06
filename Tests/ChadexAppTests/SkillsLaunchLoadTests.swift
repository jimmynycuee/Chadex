import Foundation
import XCTest
@testable import ChadexApp

/// The Skills list must finish loading after launch even when the page is
/// opened while the runtime is still warming up or being realigned: refreshes
/// requested mid-load are not dropped, leaving the page does not cancel the
/// shared load, and a finished warm-up reloads Skills that only saw a starting
/// runtime. Driven against a fake helper whose state lives in marker files.
@MainActor
final class SkillsLaunchLoadTests: XCTestCase {
    private struct Fixture {
        let model: AppModel
        let root: URL

        func makeRuntimeReady() throws {
            try Data().write(to: root.appendingPathComponent("ready"))
        }
    }

    /// - runtimeReady: the runtime is ready from the start.
    /// - slowCatalog: getSkillCatalog answers after ~0.8s with the runtime
    ///   state observed when the request arrived.
    /// - slowPrewarm: prewarmRuntime makes the runtime ready after ~1s.
    /// - catalogMismatch: getSkillCatalog fails with project_runtime_mismatch
    ///   until realignLocalProject ran.
    private func makeFixture(
        runtimeReady: Bool = true,
        slowCatalog: Bool = false,
        slowPrewarm: Bool = false,
        catalogMismatch: Bool = false
    ) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-skills-load-test-\(UUID().uuidString)", isDirectory: true)
        let project = root.appendingPathComponent("project", isDirectory: true)
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        if runtimeReady {
            try Data().write(to: root.appendingPathComponent("ready"))
        }

        let executable = root.appendingPathComponent("fake-helper")
        let script = #"""
        #!/bin/sh
        ROOT='\#(root.path)'
        LOG="$ROOT/helper.log"
        SLOW_CATALOG='\#(slowCatalog ? "1" : "0")'
        SLOW_PREWARM='\#(slowPrewarm ? "1" : "0")'
        MISMATCH='\#(catalogMismatch ? "1" : "0")'
        snap() {
          if [ -f "$ROOT/ready" ]; then R=true; else R=false; fi
          printf '{"phase":"stopped","selected_project":null,"tunnel_ready":false,"chat_gpt_connected":false,"chat_gpt_verified_for_selected_project":false,"last_verified_at_ms":null,"current_operation":null,"error":null,"activity_sequence":0,"state_revision":1,"runtime_status":{"runtime_configured":true,"runtime_ready":%s,"needs_attention":false,"summary":"","next_action":null,"summary_kind":"","server":"","runner":"","exposure":"","project":""}}' "$R"
        }
        reply() { printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$1" "$2"; }
        fail() { printf '{"protocol_version":1,"request_id":"%s","error":{"code":"%s","message":"%s"}}\n' "$1" "$2" "$2"; }
        CATALOG='{"project":"p","catalog_revision":"r","total_count":0,"returned_count":0,"skills":[],"invalid_count":0,"diagnostics":[],"discovery_truncated":false}'
        INVENTORY='{"project":"p","total_count":0,"skills":[]}'
        catalog() {
          if [ ! -f "$ROOT/ready" ]; then fail "$1" project_runtime_unavailable
          elif [ "$MISMATCH" = "1" ] && [ ! -f "$ROOT/realigned" ]; then fail "$1" project_runtime_mismatch
          else reply "$1" "$CATALOG"; fi
        }
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
          method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
          echo "$method" >> "$LOG"
          case "$method" in
            getSkillCatalog)
              if [ "$SLOW_CATALOG" = "1" ]; then
                RESP=$(catalog "$id"); ( sleep 0.8; printf '%s\n' "$RESP" ) &
              else catalog "$id"; fi ;;
            getSkillInventory)
              if [ -f "$ROOT/ready" ]; then reply "$id" "$INVENTORY"; else fail "$id" project_runtime_unavailable; fi ;;
            prewarmRuntime)
              if [ "$SLOW_PREWARM" = "1" ]; then
                ( sleep 1; touch "$ROOT/ready"; reply "$id" "$(snap)" ) &
              else
                touch "$ROOT/ready"; reply "$id" "$(snap)"
              fi ;;
            realignLocalProject)
              touch "$ROOT/realigned"; reply "$id" "$(snap)" ;;
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
        let record = ProjectRecord(name: "Project", path: project.path)
        var preferences = ChadexPreferences()
        preferences.projects = [record]
        preferences.selectedProjectID = record.id
        preferences.prepareServiceOnLaunch = true
        try store.save(preferences)
        let model = AppModel(helper: HelperClient(executableURL: executable), store: store, autostart: false)
        return Fixture(model: model, root: root)
    }

    private func count(_ method: String, _ fixture: Fixture) -> Int {
        ((try? String(contentsOf: fixture.root.appendingPathComponent("helper.log"), encoding: .utf8)) ?? "")
            .split(separator: "\n").filter { $0 == method }.count
    }

    private func waitUntil(timeout: TimeInterval = 5, _ condition: () -> Bool) async -> Bool {
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        while ProcessInfo.processInfo.systemUptime < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return condition()
    }

    func testRefreshRequestedDuringALoadIsNotDropped() async throws {
        let fixture = try makeFixture(runtimeReady: false, slowCatalog: true)
        let model = fixture.model
        let first = Task { await model.refreshSkills() }
        let sent = await waitUntil { self.count("getSkillCatalog", fixture) == 1 }
        XCTAssertTrue(sent)
        XCTAssertTrue(model.skillsLoading)
        // The runtime becomes ready while the first (doomed) load is in flight;
        // the refresh asked for now must still be served.
        try fixture.makeRuntimeReady()
        await model.refreshSkills()
        await first.value

        XCTAssertNotNil(model.skillCatalog, "the refresh requested mid-load must read again")
        XCTAssertNil(model.skillsError)
        XCTAssertFalse(model.skillsLoading)
        XCTAssertEqual(count("getSkillCatalog", fixture), 2)
        await model.shutdown()
    }

    func testLeavingThePageDoesNotCancelOrClearTheLoad() async throws {
        let fixture = try makeFixture(slowCatalog: true)
        let model = fixture.model
        // The Skills page's `.task` is cancelled when the page goes away.
        let page = Task { await model.refreshSkills() }
        let started = await waitUntil { model.skillsLoading }
        XCTAssertTrue(started)
        page.cancel()
        await page.value

        let finished = await waitUntil { !model.skillsLoading }
        XCTAssertTrue(finished, "loading must end even though its caller was cancelled")
        XCTAssertNotNil(model.skillCatalog)
        XCTAssertNil(model.skillsError, "a cancelled caller must not surface as a Skills error")
        await model.shutdown()
    }

    func testFinishedWarmUpReloadsSkillsThatFailedWhileItRan() async throws {
        let fixture = try makeFixture(runtimeReady: false, slowPrewarm: true)
        let model = fixture.model
        model.startRuntimePrewarmIfEnabled()
        // Skills page opened during the warm-up: the runtime is still starting.
        await model.refreshSkills()
        XCTAssertNil(model.skillCatalog)
        XCTAssertNotNil(model.skillsError)

        await model.runtimePrewarmTask?.value
        let loaded = await waitUntil { model.skillCatalog != nil && !model.skillsLoading }
        XCTAssertTrue(loaded, "the warm-up finishing must reload Skills without user action")
        XCTAssertNil(model.skillsError)
        await model.shutdown()
    }

    func testWarmUpDoesNotReloadSkillsThatAlreadyLoaded() async throws {
        let fixture = try makeFixture(runtimeReady: true)
        let model = fixture.model
        await model.refreshSkills()
        XCTAssertNotNil(model.skillCatalog)
        model.startRuntimePrewarmIfEnabled()
        await model.runtimePrewarmTask?.value
        XCTAssertEqual(count("getSkillCatalog", fixture), 1)
        await model.shutdown()
    }

    func testCatalogMismatchRealignsAndLoads() async throws {
        let fixture = try makeFixture(catalogMismatch: true)
        let model = fixture.model
        await model.refreshStatus(force: true)

        XCTAssertEqual(count("realignLocalProject", fixture), 1)
        XCTAssertNotNil(model.skillCatalog, "Skills load once the runtime serves the selection")
        XCTAssertNil(model.skillsError)
        XCTAssertFalse(model.skillsLoading)
        XCTAssertFalse(model.isRealigningRuntimeProject)
        await model.shutdown()
    }
}
