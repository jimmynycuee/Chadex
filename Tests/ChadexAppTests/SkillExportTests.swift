import XCTest
@testable import ChadexApp

final class SkillExportTests: XCTestCase {
    private let tunnelID = "tunnel_6aa2855405988191b439e7c0860a51c7"

    private var repoSkills: URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("skills", isDirectory: true)
    }

    func testYAMLExactText() {
        let expected = """
        dependencies:
          tools:
            - type: "mcp"
              value: "chadex"
              description: "Chadex v0.7.1: the user's Mac (projects, Plan Mode, Goal Mode); omitted project defaults to the one open in the Chadex app. Older versions may require project explicitly"
              transport: "streamable_http"
              url: "https://api.openai.com/v1/tunnel/tunnel_6aa2855405988191b439e7c0860a51c7"

        """
        XCTAssertEqual(SkillExport.openAIYAML(tunnelID: tunnelID), expected)
    }

    func testInvalidTunnelIDIsRejected() throws {
        XCTAssertNil(SkillExport.openAIYAML(tunnelID: "tunnel_chadex_dev"))
        XCTAssertNil(SkillExport.openAIYAML(tunnelID: ""))
        let out = FileManager.default.temporaryDirectory.appendingPathComponent("se-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: out) }
        XCTAssertThrowsError(try SkillExport.export(tunnelID: "bad", skillsDirectory: repoSkills, to: out))
    }

    func testZipContainsExactlyTheFourEntries() throws {
        let out = FileManager.default.temporaryDirectory.appendingPathComponent("se-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: out, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: out) }

        let zips = try SkillExport.export(tunnelID: tunnelID, skillsDirectory: repoSkills, to: out)
        XCTAssertEqual(zips.map(\.lastPathComponent), ["chadex-plan.zip", "chadex-goal.zip"])

        for name in SkillExport.skillNames {
            let zip = out.appendingPathComponent("\(name).zip").path
            let listing = try run("/usr/bin/zipinfo", ["-1", zip])
            let entries = listing.split(separator: "\n").map(String.init).filter { !$0.hasPrefix("__MACOSX") }
            XCTAssertEqual(
                Set(entries.filter { !$0.hasSuffix("/") }),
                ["\(name)/SKILL.md", "\(name)/agents/openai.yaml"]
            )
            XCTAssertEqual(
                Set(entries.filter { $0.hasSuffix("/") }),
                ["\(name)/", "\(name)/agents/"]
            )
            let yaml = try run("/usr/bin/unzip", ["-p", zip, "\(name)/agents/openai.yaml"])
            XCTAssertTrue(yaml.contains(tunnelID))
            XCTAssertTrue(yaml.contains("Chadex v0.7.1"))
            XCTAssertTrue(yaml.contains("Older versions may require project explicitly"))
            let skill = try run("/usr/bin/unzip", ["-p", zip, "\(name)/SKILL.md"])
            XCTAssertTrue(skill.contains("Chadex v0.7.1"))
            XCTAssertTrue(skill.contains("project open in the Chadex app"))
            XCTAssertFalse(skill.contains("codex-plan"))
        }
    }

    private func run(_ path: String, _ arguments: [String]) throws -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = arguments
        let pipe = Pipe()
        process.standardOutput = pipe
        try process.run()
        let data = pipe.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        return String(data: data, encoding: .utf8) ?? ""
    }
}
