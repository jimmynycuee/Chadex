import Foundation
import XCTest
@testable import ChadexApp

/// Project switching and removal against a fake helper whose switch can
/// succeed, fail, be slow, or fail on the App side after the helper already
/// switched (what an App-side timeout looks like).
@MainActor
final class ProjectSwitchRemovalTests: XCTestCase {
    private enum SwitchMode: String {
        case succeed
        case fail
        /// The App gets an error (like its own timeout) although the helper
        /// switched: the helper's selection then names the target.
        case failAfterSwitching = "fail_after_switching"
        /// Answers after ~1s.
        case slow
    }

    private struct Fixture {
        let model: AppModel
        let root: URL
        let alpha: ProjectRecord
        let beta: ProjectRecord
        let gamma: ProjectRecord

        func setSwitchMode(_ mode: SwitchMode) throws {
            try mode.rawValue.write(to: root.appendingPathComponent("switch_mode"), atomically: true, encoding: .utf8)
        }

        func methods() -> [String] {
            ((try? String(contentsOf: root.appendingPathComponent("helper.log"), encoding: .utf8)) ?? "")
                .split(separator: "\n").map(String.init)
        }

        func persistedProjectIDs() throws -> [UUID] {
            let data = try Data(contentsOf: root.appendingPathComponent("preferences.json"))
            return try JSONDecoder().decode(ChadexPreferences.self, from: data).projects.map(\.id)
        }
    }

    private func makeFixture(mode: SwitchMode = .succeed) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-switch-remove-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        var records: [ProjectRecord] = []
        for name in ["Alpha", "Beta", "Gamma"] {
            let folder = root.appendingPathComponent(name, isDirectory: true)
            try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
            records.append(ProjectRecord(name: name, path: folder.path))
        }
        try records[0].path.write(to: root.appendingPathComponent("selected"), atomically: true, encoding: .utf8)

        let executable = root.appendingPathComponent("fake-helper")
        let script = #"""
        #!/bin/sh
        ROOT='\#(root.path)'
        LOG="$ROOT/helper.log"
        snap() {
          P=$(cat "$ROOT/selected" 2>/dev/null)
          SP="{\"path\":\"$P\",\"allowed_root\":\"$P\",\"is_git_repository\":false,\"readable\":true,\"writable\":true}"
          printf '{"phase":"stopped","selected_project":%s,"tunnel_ready":false,"chat_gpt_connected":false,"chat_gpt_verified_for_selected_project":false,"last_verified_at_ms":null,"current_operation":null,"error":null,"activity_sequence":0,"state_revision":1,"runtime_status":{"runtime_configured":true,"runtime_ready":true,"needs_attention":false,"summary":"","next_action":null,"summary_kind":"","server":"","runner":"","exposure":"","project":""}}' "$SP"
        }
        reply() { printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$1" "$2"; }
        fail() { printf '{"protocol_version":1,"request_id":"%s","error":{"code":"project_activation_timed_out","message":"The local runtime did not finish opening the project in time","recovery":"Retry."}}\n' "$1"; }
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
          method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
          path=$(printf '%s' "$line" | sed -n 's/.*"path":"\([^"]*\)".*/\1/p' | sed 's#\\/#/#g')
          echo "$method" >> "$LOG"
          case "$method" in
            switchLocalProject)
              MODE=$(cat "$ROOT/switch_mode" 2>/dev/null)
              case "$MODE" in
                fail) fail "$id" ;;
                fail_after_switching) printf '%s' "$path" > "$ROOT/selected"; fail "$id" ;;
                slow) ( sleep 1; printf '%s' "$path" > "$ROOT/selected"; reply "$id" "$(snap)" ) & ;;
                *) printf '%s' "$path" > "$ROOT/selected"; reply "$id" "$(snap)" ;;
              esac ;;
            getSkillCatalog)
              reply "$id" '{"project":"p","catalog_revision":"r","total_count":0,"returned_count":0,"skills":[],"invalid_count":0,"diagnostics":[],"discovery_truncated":false}' ;;
            getSkillInventory)
              reply "$id" '{"project":"p","total_count":0,"skills":[]}' ;;
            queryActivities)
              reply "$id" '[]' ;;
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
        var preferences = ChadexPreferences()
        preferences.projects = records
        preferences.selectedProjectID = records[0].id
        try store.save(preferences)
        let model = AppModel(helper: HelperClient(executableURL: executable), store: store, autostart: false)
        let fixture = Fixture(model: model, root: root, alpha: records[0], beta: records[1], gamma: records[2])
        try fixture.setSwitchMode(mode)
        return fixture
    }

    private func waitUntil(timeout: TimeInterval = 3, _ condition: () -> Bool) async -> Bool {
        let deadline = ProcessInfo.processInfo.systemUptime + timeout
        while ProcessInfo.processInfo.systemUptime < deadline {
            if condition() { return true }
            try? await Task.sleep(for: .milliseconds(20))
        }
        return condition()
    }

    func testAfterSwitchingANonCurrentProjectIsRemovedWithoutAnotherSwitch() async throws {
        let fixture = try makeFixture()
        let model = fixture.model
        let switched = await model.selectProject(fixture.beta.id)
        XCTAssertTrue(switched)
        XCTAssertFalse(model.isSwitchingProject)

        let switchesBefore = fixture.methods().filter { $0 == "switchLocalProject" }.count
        let removed = await model.removeProject(fixture.gamma.id)
        XCTAssertTrue(removed)
        XCTAssertNil(model.presentedError)
        XCTAssertEqual(model.projects.map(\.id), [fixture.alpha.id, fixture.beta.id])
        XCTAssertEqual(model.selectedProject?.id, fixture.beta.id)
        XCTAssertEqual(try fixture.persistedProjectIDs(), [fixture.alpha.id, fixture.beta.id])
        XCTAssertEqual(
            fixture.methods().filter { $0 == "switchLocalProject" }.count,
            switchesBefore,
            "removing a project that is not current never switches"
        )
        await model.shutdown()
    }

    func testANonCurrentProjectCanBeRemovedWhileASwitchRuns() async throws {
        let fixture = try makeFixture(mode: .slow)
        let model = fixture.model
        let switching = Task { await model.selectProject(fixture.beta.id) }
        let busy = await waitUntil { model.isSwitchingProject }
        XCTAssertTrue(busy)

        let removed = await model.removeProject(fixture.gamma.id)
        XCTAssertTrue(removed, "a project that is neither current nor the switch target can go at once")
        XCTAssertFalse(model.projects.contains { $0.id == fixture.gamma.id })

        let switched = await switching.value
        XCTAssertTrue(switched)
        XCTAssertEqual(model.selectedProject?.id, fixture.beta.id)
        await model.shutdown()
    }

    func testRemovingTheSwitchTargetOrTheCurrentProjectDuringASwitchExplainsWhy() async throws {
        let fixture = try makeFixture(mode: .slow)
        let model = fixture.model
        let switching = Task { await model.selectProject(fixture.beta.id) }
        let busy = await waitUntil { model.isSwitchingProject }
        XCTAssertTrue(busy)

        let removedTarget = await model.removeProject(fixture.beta.id)
        XCTAssertFalse(removedTarget)
        XCTAssertEqual(model.presentedError?.code, "project_switch_in_progress")
        model.presentedError = nil

        let removedCurrent = await model.removeProject(fixture.alpha.id)
        XCTAssertFalse(removedCurrent)
        XCTAssertEqual(model.presentedError?.code, "project_switch_in_progress")
        XCTAssertEqual(model.projects.count, 3, "nothing is removed while the switch runs")

        _ = await switching.value
        await model.shutdown()
    }

    func testRemovingTheCurrentProjectKeepsItAndSaysWhyWhenTheFallbackSwitchFails() async throws {
        let fixture = try makeFixture(mode: .fail)
        let model = fixture.model
        let removed = await model.removeProject(fixture.alpha.id)
        XCTAssertFalse(removed)
        XCTAssertEqual(model.projects.count, 3, "the current project stays when the switch away fails")
        XCTAssertEqual(model.selectedProject?.id, fixture.alpha.id)
        let error = try XCTUnwrap(model.presentedError)
        XCTAssertEqual(error.title, L10n.string("project.removeSwitchFailedTitle"))
        XCTAssertTrue(error.message.contains(fixture.alpha.name))
        XCTAssertTrue(error.message.contains(fixture.beta.name), "names the project it tried to switch to")
        XCTAssertTrue(error.message.contains("did not finish opening the project"), "keeps the switch's own reason")
        XCTAssertEqual(error.recovery, L10n.string("project.removeSwitchFailedRecovery", fixture.alpha.name))
        XCTAssertFalse(model.isSwitchingProject)
        await model.shutdown()
    }

    func testAnAppSideFailureAdoptsTheSwitchTheHelperCompleted() async throws {
        let fixture = try makeFixture(mode: .failAfterSwitching)
        let model = fixture.model
        let switched = await model.selectProject(fixture.beta.id)
        XCTAssertTrue(switched, "the helper's selection is authoritative")
        XCTAssertEqual(model.selectedProject?.id, fixture.beta.id)
        XCTAssertNil(model.presentedError)
        XCTAssertFalse(model.isSwitchingProject)
        await model.shutdown()
    }

    func testAFailedSwitchTheHelperRolledBackKeepsTheSelectionAndReportsIt() async throws {
        let fixture = try makeFixture(mode: .fail)
        let model = fixture.model
        let switched = await model.selectProject(fixture.beta.id)
        XCTAssertFalse(switched)
        XCTAssertEqual(model.selectedProject?.id, fixture.alpha.id)
        XCTAssertEqual(model.presentedError?.code, "project_activation_timed_out")
        XCTAssertFalse(model.isSwitchingProject)
        await model.shutdown()
    }
}
