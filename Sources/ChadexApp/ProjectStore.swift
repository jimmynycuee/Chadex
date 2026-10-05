import Foundation

struct ChadexPreferences: Codable, Sendable {
    var projects: [ProjectRecord] = []
    var selectedProjectID: UUID?
    var tunnelID: String = ""
    /// Prepare the local service in the background at launch so a later
    /// Connect only has to start the tunnel. nil means "never chosen", which
    /// reads as ON. It deliberately uses a new key instead of the former
    /// "restore local service" one (`restoreServiceOnLaunch`, default off):
    /// that key is stored as `false` for almost everyone because it was never
    /// touched, so reusing it would silently turn the faster launch off for
    /// them. The legacy key is ignored when decoding and dropped on next save.
    var prepareServiceOnLaunch: Bool?
    var restoreConnectionOnLaunch = false
    var backgroundCloseHintShown = false
    var computerControlDefaultMode: ComputerControlMode?

    var prepareServiceOnLaunchEnabled: Bool {
        prepareServiceOnLaunch ?? true
    }
}

enum GlobalInstructionsStoreError: LocalizedError {
    case invalidFile
    case tooLarge
    case invalidEncoding

    var errorDescription: String? {
        switch self {
        case .invalidFile: return "Global Instructions storage is not a regular Chadex-owned file."
        case .tooLarge: return "Global Instructions are too large (maximum 8 KiB)."
        case .invalidEncoding: return "Global Instructions must be valid UTF-8 text."
        }
    }
}

final class ProjectStore: @unchecked Sendable {
    static let globalInstructionsFilename = "global-instructions.md"
    static let maxGlobalInstructionsBytes = 8 * 1024

    private let fileURL: URL
    private let globalInstructionsURL: URL
    private let queue = DispatchQueue(label: "app.chadex.preferences")

    init(
        fileManager: FileManager = .default,
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) {
        let globalURL = Self.globalInstructionsURL(
            environment: environment,
            homeDirectory: fileManager.homeDirectoryForCurrentUser
        )
        let base = globalURL.deletingLastPathComponent()
        try? fileManager.createDirectory(at: base, withIntermediateDirectories: true)
        self.fileURL = base.appendingPathComponent("preferences.json")
        self.globalInstructionsURL = globalURL
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

    private func globalInstructionsFileExists() throws -> Bool {
        if (try? FileManager.default.destinationOfSymbolicLink(atPath: globalInstructionsURL.path)) != nil {
            throw GlobalInstructionsStoreError.invalidFile
        }
        return FileManager.default.fileExists(atPath: globalInstructionsURL.path)
    }

    func loadGlobalInstructions() throws -> String {
        try queue.sync {
            guard try globalInstructionsFileExists() else { return "" }
            let values = try globalInstructionsURL.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
            guard values.isRegularFile == true, values.isSymbolicLink != true else {
                throw GlobalInstructionsStoreError.invalidFile
            }
            if let size = values.fileSize, size > Self.maxGlobalInstructionsBytes {
                throw GlobalInstructionsStoreError.tooLarge
            }
            let data = try Data(contentsOf: globalInstructionsURL, options: [.mappedIfSafe])
            guard data.count <= Self.maxGlobalInstructionsBytes else {
                throw GlobalInstructionsStoreError.tooLarge
            }
            guard let value = String(data: data, encoding: .utf8) else {
                throw GlobalInstructionsStoreError.invalidEncoding
            }
            return value
        }
    }

    func saveGlobalInstructions(_ value: String) throws {
        try queue.sync {
            let data = Data(value.utf8)
            guard data.count <= Self.maxGlobalInstructionsBytes else {
                throw GlobalInstructionsStoreError.tooLarge
            }
            if try globalInstructionsFileExists() {
                let values = try globalInstructionsURL.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey])
                guard values.isRegularFile == true, values.isSymbolicLink != true else {
                    throw GlobalInstructionsStoreError.invalidFile
                }
            }
            try data.write(to: globalInstructionsURL, options: [.atomic])
        }
    }

    static func globalInstructionsURL(
        environment: [String: String],
        homeDirectory: URL
    ) -> URL {
        if let override = environment["CHADEX_PREFERENCES_DIR"]?
            .trimmingCharacters(in: .whitespacesAndNewlines), !override.isEmpty {
            return URL(fileURLWithPath: override, isDirectory: true)
                .appendingPathComponent(globalInstructionsFilename)
        }
        return homeDirectory
            .appendingPathComponent("Library/Application Support/Chadex", isDirectory: true)
            .appendingPathComponent(globalInstructionsFilename)
    }
}

extension JSONEncoder {
    static var pretty: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        return encoder
    }
}
