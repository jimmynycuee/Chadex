import Foundation

struct ChadexPreferences: Codable, Sendable {
    var projects: [ProjectRecord] = []
    var selectedProjectID: UUID?
    var tunnelID: String = ""
    var restoreServiceOnLaunch = false
    var restoreConnectionOnLaunch = false
    var backgroundCloseHintShown = false
}

final class ProjectStore: @unchecked Sendable {
    private let fileURL: URL
    private let queue = DispatchQueue(label: "app.chadex.preferences")

    init(
        fileManager: FileManager = .default,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        let override = environment["CHADEX_PREFERENCES_DIR"]?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let base: URL
        if let override, !override.isEmpty {
            base = URL(fileURLWithPath: override, isDirectory: true)
        } else {
            base = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
                .appendingPathComponent("Chadex", isDirectory: true)
        }
        try? fileManager.createDirectory(at: base, withIntermediateDirectories: true)
        self.fileURL = base.appendingPathComponent("preferences.json")
    }

    func load() -> ChadexPreferences {
        queue.sync {
            guard let data = try? Data(contentsOf: fileURL),
                  let prefs = try? JSONDecoder().decode(ChadexPreferences.self, from: data)
            else { return ChadexPreferences() }
            return prefs
        }
    }

    func save(_ preferences: ChadexPreferences) throws {
        try queue.sync {
            let data = try JSONEncoder.pretty.encode(preferences)
            try data.write(to: fileURL, options: [.atomic])
        }
    }
}

extension JSONEncoder {
    static var pretty: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return encoder
    }
}
