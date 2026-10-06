import Foundation
import XCTest
@testable import ChadexApp

/// `AppModel.removeManagedSkill` and the enable/disable toggle against a fake helper that
/// records every Skill request line and answers `getSkillCatalog` / `getSkillInventory`.
@MainActor
final class SkillRemovalTests: XCTestCase {
    private struct Fixture {
        let model: AppModel
        let log: URL
    }

    private static let managed = ManagedSkillInventoryEntry(
        skillId: "wc_skill_demo",
        skillKey: "demo",
        stateRevision: "wc_skillstate_current",
        activePackageRevision: "wc_skillpkg_one",
        preferredPackageRevision: "wc_skillpkg_one",
        definitionRevision: "def1",
        name: "demo",
        description: "d",
        totalVersions: 2
    )

    private static func item(managed: ManagedSkillInventoryEntry?) -> SkillCenterItem {
        SkillCenterItem(
            skillId: "wc_skill_demo",
            name: "demo",
            description: "d",
            definitionRevision: "def1",
            packageRevision: nil,
            sourceScope: managed == nil ? "project" : "operator_installed",
            trust: managed == nil ? "project_guidance" : "operator_installed_guidance",
            nameConflict: false,
            scriptsAllowed: nil,
            managed: managed
        )
    }

    /// `failCode` empty: every mutation succeeds. Otherwise mutations fail with that code.
    private func makeFixture(failCode: String) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-remove-test-\(UUID().uuidString)", isDirectory: true)
        let project = root.appendingPathComponent("project", isDirectory: true)
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }

        let log = root.appendingPathComponent("helper.log")
        let executable = root.appendingPathComponent("fake-helper")
        let script = #"""
        #!/bin/sh
        LOG='\#(log.path)'
        SNAP='{"phase":"stopped","selected_project":null,"tunnel_ready":false,"chat_gpt_connected":false,"chat_gpt_verified_for_selected_project":false,"last_verified_at_ms":null,"current_operation":null,"error":null,"activity_sequence":0,"state_revision":1}'
        CATALOG='{"project":"p","catalog_revision":"r","total_count":0,"returned_count":0,"skills":[],"invalid_count":0,"diagnostics":[],"discovery_truncated":false}'
        INVENTORY='{"project":"p","total_count":0,"skills":[]}'
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
          method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
          case "$method" in
            removeSkill|activateSkill|deactivateSkill)
              printf '%s\n' "$line" >> "$LOG"
              if [ -n '\#(failCode)' ]; then
                printf '{"protocol_version":1,"request_id":"%s","error":{"code":"\#(failCode)","message":"runtime said no"}}\n' "$id"
              else
                printf '{"protocol_version":1,"request_id":"%s","result":{"skill_key":"demo","removed_revisions":2}}\n' "$id"
              fi ;;
            getSkillCatalog) printf '%s\n' "$line" >> "$LOG"; printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$id" "$CATALOG" ;;
            getSkillInventory) printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$id" "$INVENTORY" ;;
            *) printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$id" "$SNAP" ;;
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
        return Fixture(model: model, log: log)
    }

    private func requests(_ fixture: Fixture, method: String) -> [String] {
        ((try? String(contentsOf: fixture.log, encoding: .utf8)) ?? "")
            .split(separator: "\n").map(String.init).filter { $0.contains("\"method\":\"\(method)\"") }
    }

    func testRemoveSendsKeyAndCurrentStateRevisionThenRefreshes() async throws {
        let fixture = try makeFixture(failCode: "")
        let ok = await fixture.model.removeManagedSkill(Self.item(managed: Self.managed))
        await fixture.model.shutdown()

        XCTAssertTrue(ok)
        let sent = requests(fixture, method: "removeSkill")
        XCTAssertEqual(sent.count, 1)
        XCTAssertTrue(sent[0].contains("\"skill_key\":\"demo\""), sent[0])
        XCTAssertTrue(sent[0].contains("\"state_revision\":\"wc_skillstate_current\""), sent[0])
        XCTAssertFalse(requests(fixture, method: "getSkillCatalog").isEmpty, "the list is refreshed after removal")
        XCTAssertNil(fixture.model.skillsError)
        XCTAssertTrue(fixture.model.skillMutationInFlightIDs.isEmpty)
    }

    func testProjectAndExternalSkillsAreNeverRemoved() async throws {
        let fixture = try makeFixture(failCode: "")
        let ok = await fixture.model.removeManagedSkill(Self.item(managed: nil))
        await fixture.model.shutdown()

        XCTAssertFalse(ok)
        XCTAssertEqual(requests(fixture, method: "removeSkill"), [])
    }

    func testRemoveFailureIsShownInChineseOrEnglishNotSwallowedByTheRefresh() async throws {
        let fixture = try makeFixture(failCode: "skill_remove_revision_failed")
        let ok = await fixture.model.removeManagedSkill(Self.item(managed: Self.managed))
        await fixture.model.shutdown()

        XCTAssertFalse(ok)
        XCTAssertEqual(fixture.model.skillsError, L10n.string("skills.error.removeFailed"))
        XCTAssertTrue(fixture.model.skillMutationInFlightIDs.isEmpty)
    }

    func testStaleStateRevisionShowsTheChangedElsewhereMessage() async throws {
        let fixture = try makeFixture(failCode: "skill_state_changed")
        let ok = await fixture.model.removeManagedSkill(Self.item(managed: Self.managed))
        await fixture.model.shutdown()

        XCTAssertFalse(ok)
        XCTAssertEqual(fixture.model.skillsError, L10n.string("skills.error.stateChanged"))
    }

    func testEnableFailureIsShownAfterTheRefresh() async throws {
        let fixture = try makeFixture(failCode: "skill_activate_failed")
        var disabled = Self.managed
        disabled.activePackageRevision = nil
        await fixture.model.setManagedSkillEnabled(Self.item(managed: disabled), enabled: true)
        await fixture.model.shutdown()

        XCTAssertEqual(requests(fixture, method: "activateSkill").count, 1)
        XCTAssertEqual(fixture.model.skillsError, L10n.string("skills.error.toggleFailed"))
    }

    func testRemovalErrorCodesAndStringsAreLocalized() {
        for code in [
            "skill_remove_failed", "skill_remove_revision_failed", "skill_remove_cleanup_failed",
            "skill_remove_incomplete", "skill_remove_invalid", "skill_active_revision_remove_forbidden",
            "skill_activate_failed", "skill_deactivate_failed", "skill_package_not_found",
        ] {
            let message = SkillManagementErrorMessage.message(forCode: code)
            XCTAssertNotNil(message, code)
            XCTAssertFalse(message?.hasPrefix("skills.") ?? true, "missing localization for \(code)")
        }
        for key in [
            "skills.remove", "skills.removeHelp", "skills.remove.confirmTitle",
            "skills.remove.confirmMessage", "skills.remove.confirmAction",
        ] {
            XCTAssertNotEqual(L10n.string(key, "demo"), key, "missing localization for \(key)")
        }
    }
}
