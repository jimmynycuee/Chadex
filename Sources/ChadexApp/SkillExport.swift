import Foundation

/// Builds the ChatGPT Skill zips (`chadex-plan.zip`, `chadex-goal.zip`). A Skill
/// only reaches Chadex's tools when `agents/openai.yaml` declares the Chadex MCP
/// tunnel as a dependency, and that URL is per user, so the zips are generated
/// from the user's own Tunnel ID instead of being shipped prebuilt.
enum SkillExport {
    static let skillNames = ["chadex-plan", "chadex-goal"]

    enum ExportError: LocalizedError {
        case invalidTunnelID
        case missingSkillSource(String)
        case zipFailed(String)

        var errorDescription: String? {
            switch self {
            case .invalidTunnelID: return "Invalid Tunnel ID"
            case .missingSkillSource(let name): return "Missing bundled skill: \(name)"
            case .zipFailed(let detail): return "Zip failed: \(detail)"
            }
        }
    }

    static func tunnelURL(tunnelID: String) -> String {
        "https://api.openai.com/v1/tunnel/\(tunnelID)"
    }

    /// Exact `agents/openai.yaml` text; `nil` when the Tunnel ID is not valid.
    static func openAIYAML(tunnelID: String) -> String? {
        guard AppModel.isValidTunnelID(tunnelID) else { return nil }
        return """
        dependencies:
          tools:
            - type: "mcp"
              value: "chadex"
              description: "Chadex: the user's Mac (projects, Plan Mode, Goal Mode) through the Chadex MCP tunnel"
              transport: "streamable_http"
              url: "\(tunnelURL(tunnelID: tunnelID))"

        """
    }

    /// Folder holding `chadex-plan/` and `chadex-goal/` inside the app bundle.
    static var bundledSkillsDirectory: URL? {
        Bundle.main.resourceURL?.appendingPathComponent("skills", isDirectory: true)
    }

    /// Writes one zip per skill into `destination` and returns their URLs.
    /// Each zip holds `<name>/SKILL.md` and `<name>/agents/openai.yaml`.
    @discardableResult
    static func export(tunnelID: String, skillsDirectory: URL, to destination: URL) throws -> [URL] {
        guard let yaml = openAIYAML(tunnelID: tunnelID) else { throw ExportError.invalidTunnelID }
        let fileManager = FileManager.default
        let staging = fileManager.temporaryDirectory
            .appendingPathComponent("chadex-skill-export-\(UUID().uuidString)", isDirectory: true)
        try fileManager.createDirectory(at: staging, withIntermediateDirectories: true)
        defer { try? fileManager.removeItem(at: staging) }

        var outputs: [URL] = []
        for name in skillNames {
            let source = skillsDirectory.appendingPathComponent(name, isDirectory: true)
                .appendingPathComponent("SKILL.md")
            guard fileManager.fileExists(atPath: source.path) else {
                throw ExportError.missingSkillSource(name)
            }
            let folder = staging.appendingPathComponent(name, isDirectory: true)
            let agents = folder.appendingPathComponent("agents", isDirectory: true)
            try fileManager.createDirectory(at: agents, withIntermediateDirectories: true)
            try fileManager.copyItem(at: source, to: folder.appendingPathComponent("SKILL.md"))
            try yaml.write(to: agents.appendingPathComponent("openai.yaml"), atomically: true, encoding: .utf8)

            let zip = destination.appendingPathComponent("\(name).zip")
            try? fileManager.removeItem(at: zip)
            try ditto(folder: folder, zip: zip)
            outputs.append(zip)
        }
        return outputs
    }

    private static func ditto(folder: URL, zip: URL) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = ["-c", "-k", "--keepParent", "--norsrc", "--noextattr", "--noqtn", "--noacl", folder.path, zip.path]
        let errorPipe = Pipe()
        process.standardError = errorPipe
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let detail = String(data: errorPipe.fileHandleForReading.readDataToEndOfFile(), encoding: .utf8) ?? ""
            throw ExportError.zipFailed(detail.trimmingCharacters(in: .whitespacesAndNewlines))
        }
    }
}
