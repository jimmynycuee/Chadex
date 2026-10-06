import Foundation
import XCTest
@testable import ChadexApp

/// `AppModel.installSkill(skillKey:archiveURL:)` against a fake helper that records the
/// artifact path it receives and whether that file exists at that moment.
@MainActor
final class SkillArchiveInstallTests: XCTestCase {
    private struct Fixture {
        let model: AppModel
        let project: URL
        let log: URL
        let zip: URL
    }

    private func makeFixture(installFails: Bool) throws -> Fixture {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-install-test-\(UUID().uuidString)", isDirectory: true)
        let project = root.appendingPathComponent("project", isDirectory: true)
        try FileManager.default.createDirectory(at: project, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }

        let zip = root.appendingPathComponent("download.zip")
        try Data("PK-fake".utf8).write(to: zip)

        let log = root.appendingPathComponent("helper.log")
        let executable = root.appendingPathComponent("fake-helper")
        let script = #"""
        #!/bin/sh
        LOG='\#(log.path)'
        PROJECT='\#(project.path)'
        SNAP='{"phase":"stopped","selected_project":null,"tunnel_ready":false,"chat_gpt_connected":false,"chat_gpt_verified_for_selected_project":false,"last_verified_at_ms":null,"current_operation":null,"error":null,"activity_sequence":0,"state_revision":1}'
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
          method=$(printf '%s' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
          if [ "$method" = "installSkill" ]; then
            art=$(printf '%s' "$line" | sed -n 's/.*"artifact_path":"\([^"]*\)".*/\1/p' | tr -d '\\')
            if [ -f "$PROJECT/$art" ]; then echo "install $art EXISTS" >> "$LOG"; else echo "install $art MISSING" >> "$LOG"; fi
            if [ "\#(installFails ? "1" : "0")" = "1" ]; then
              printf '{"protocol_version":1,"request_id":"%s","error":{"code":"skill_install_failed","message":"nope"}}\n' "$id"
            else
              printf '{"protocol_version":1,"request_id":"%s","result":{}}\n' "$id"
            fi
          else
            echo "$method" >> "$LOG"
            printf '{"protocol_version":1,"request_id":"%s","result":%s}\n' "$id" "$SNAP"
          fi
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
        return Fixture(model: model, project: project, log: log, zip: zip)
    }

    private func installLines(_ fixture: Fixture) -> [String] {
        ((try? String(contentsOf: fixture.log, encoding: .utf8)) ?? "")
            .split(separator: "\n").map(String.init).filter { $0.hasPrefix("install ") }
    }

    private func staged(_ fixture: Fixture) -> [String] {
        (try? FileManager.default.contentsOfDirectory(
            atPath: fixture.project.appendingPathComponent(".chadex/skill-imports").path
        )) ?? []
    }

    func testInstallUsesProjectRelativeCopyAndDeletesItAfterSuccess() async throws {
        let fixture = try makeFixture(installFails: false)
        let ok = await fixture.model.installSkill(skillKey: "demo", archiveURL: fixture.zip)
        await fixture.model.shutdown()

        XCTAssertTrue(ok)
        let lines = installLines(fixture)
        XCTAssertEqual(lines.count, 1)
        XCTAssertTrue(lines[0].hasPrefix("install .chadex/skill-imports/"), lines[0])
        XCTAssertTrue(lines[0].hasSuffix(".zip EXISTS"), "the copy must exist while the helper installs: \(lines[0])")
        XCTAssertEqual(staged(fixture), [])
        XCTAssertTrue(FileManager.default.fileExists(atPath: fixture.zip.path))
        XCTAssertFalse(fixture.model.skillInstallInFlight)
    }

    func testInstallDeletesCopyAfterHelperFailureAndReportsError() async throws {
        let fixture = try makeFixture(installFails: true)
        let ok = await fixture.model.installSkill(skillKey: "demo", archiveURL: fixture.zip)
        await fixture.model.shutdown()

        XCTAssertFalse(ok)
        XCTAssertEqual(installLines(fixture).count, 1)
        XCTAssertEqual(staged(fixture), [])
        XCTAssertEqual(fixture.model.skillsError, "nope")
        XCTAssertFalse(fixture.model.skillInstallInFlight)
    }

    func testInstallRejectsBadArchiveWithoutCallingHelper() async throws {
        let fixture = try makeFixture(installFails: false)
        let notZip = fixture.zip.deletingLastPathComponent().appendingPathComponent("a.txt")
        try Data("x".utf8).write(to: notZip)
        let ok = await fixture.model.installSkill(skillKey: "demo", archiveURL: notZip)
        await fixture.model.shutdown()

        XCTAssertFalse(ok)
        XCTAssertEqual(installLines(fixture), [])
        XCTAssertEqual(fixture.model.skillsError, SkillArchiveStagingError.notZip.localizedDescription)
        XCTAssertFalse(fixture.model.skillInstallInFlight)
    }
}
