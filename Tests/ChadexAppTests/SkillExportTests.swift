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
              description: "Chadex: the user's Mac (projects, Plan Mode, Goal Mode) through the Chadex MCP tunnel"
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
