import Foundation
import XCTest
@testable import ChadexApp

/// When the local runtime serves another project than the selected one
/// (`project_runtime_mismatch`), the Skills refresh re-activates the selected
/// project once and retries, instead of only showing the error.
@MainActor
final class RuntimeProjectRealignTests: XCTestCase {
    private struct Fixture {
        let model: AppModel
        let project: URL
        let log: URL
    }

    /// `healsAfterSwitch`: the fake runtime serves the selection once
    /// `switchLocalProject` ran; otherwise the mismatch persists.
    private func makeFixture(healsAfterSwitch: Bool) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-realign-test-\(UUID().uuidString)", isDirectory: true)
        let project = root.appendingPathComponent("project", isDirectory: true)
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }

        let log = root.appendingPathComponent("helper.log")
        let marker = root.appendingPathComponent("switched")
        let executable = root.appendingPathComponent("fake-helper")
        let script = #"""
        #!/bin/sh
        LOG='\#(log.path)'
        MARKER='\#(marker.path)'
        HEALS='\#(healsAfterSwitch ? "1" : "0")'
        SNAP='{"phase":"stopped","selected_project":null,"tunnel_ready":false,"chat_gpt_connected":false,"chat_gpt_verified_for_selected_project":false,"last_verified_at_ms":null,"current_operation":null,"error":null,"activity_sequence":0,"state_revision":1}'
        CATALOG='{"project":"p","catalog_revision":"r","total_count":0,"returned_count":0,"skills":[],"invalid_count":0,"diagnostics":[],"discovery_truncated":false}'
        INVENTORY='{"project":"p","total_count":0,"skills":[]}'
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
          method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
          echo "$method" >> "$LOG"
          case "$method" in
            switchLocalProject)
              touch "$MARKER"
              printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$id" "$SNAP" ;;
            getSkillCatalog)
              printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$id" "$CATALOG" ;;
            getSkillInventory)
              if [ "$HEALS" = "1" ] && [ -f "$MARKER" ]; then
                printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$id" "$INVENTORY"
              else
                printf '{"protocol_version":1,"request_id":"%s","error":{"code":"project_runtime_mismatch","message":"The local runtime is active for a different project"}}\n' "$id"
              fi ;;
            *)
              printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$id" "$SNAP" ;;
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
        try store.save(preferences)
        let model = AppModel(helper: HelperClient(executableURL: executable), store: store, autostart: false)
        return Fixture(model: model, project: project, log: log)
    }

    private func lines(_ fixture: Fixture) -> [String] {
        ((try? String(contentsOf: fixture.log, encoding: .utf8)) ?? "")
            .split(separator: "\n").map(String.init)
    }

    func testMismatchReactivatesTheSelectedProjectAndClearsTheError() async throws {
        let fixture = try makeFixture(healsAfterSwitch: true)
        await fixture.model.refreshSkills()
        await fixture.model.shutdown()

        let log = lines(fixture)
        XCTAssertEqual(log.filter { $0 == "switchLocalProject" }.count, 1)
        XCTAssertEqual(log.filter { $0 == "getSkillInventory" }.count, 2, "inventory is retried after the realignment")
        XCTAssertNil(fixture.model.skillsError)
        XCTAssertNotNil(fixture.model.skillInventory)
    }

    func testPersistentMismatchIsRealignedOnlyOncePerSelectionAndExplained() async throws {
        let fixture = try makeFixture(healsAfterSwitch: false)
        await fixture.model.refreshSkills()
        await fixture.model.refreshSkills()
        await fixture.model.shutdown()

        XCTAssertEqual(lines(fixture).filter { $0 == "switchLocalProject" }.count, 1, "no realignment loop")
        XCTAssertEqual(fixture.model.skillsError, L10n.string("skills.error.projectRuntimeMismatch"))
    }

    func testNoRealignmentWhileTheLaunchWarmUpOwnsTheRuntime() async throws {
        let fixture = try makeFixture(healsAfterSwitch: true)
        fixture.model.startRuntimePrewarmIfEnabled()
        XCTAssertNotNil(fixture.model.runtimePrewarmTask)
        let realigned = await fixture.model.realignRuntimeProjectIfNeeded()
        await fixture.model.runtimePrewarmTask?.value
        await fixture.model.shutdown()

        XCTAssertFalse(realigned)
        XCTAssertFalse(lines(fixture).contains("switchLocalProject"))
    }
}
