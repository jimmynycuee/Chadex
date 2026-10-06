import Foundation
import Darwin

/// Why a Skill ZIP could not be staged into the project for installation.
enum SkillArchiveStagingError: Error, Equatable {
    case notZip
    case notRegularFile
    case tooLarge
    case unreadable
    case projectUnavailable
    case unsafeStagingDirectory
    case copyFailed
}

extension SkillArchiveStagingError: LocalizedError {
    var errorDescription: String? {
        switch self {
        case .notZip: return L10n.string("skills.archive.error.notZip")
        case .notRegularFile: return L10n.string("skills.archive.error.notRegularFile")
        case .tooLarge:
            return L10n.string("skills.archive.error.tooLarge", SkillArchiveStaging.maxArchiveBytes / (1024 * 1024))
        case .unreadable: return L10n.string("skills.archive.error.unreadable")
        case .projectUnavailable: return L10n.string("skills.archive.error.projectUnavailable")
        case .unsafeStagingDirectory: return L10n.string("skills.archive.error.unsafeDirectory")
        case .copyFailed: return L10n.string("skills.archive.error.copyFailed")
        }
    }
}

/// A temporary copy of a user-chosen ZIP inside the selected project. The runtime only
/// accepts project-relative artifacts, so the app copies the ZIP in, installs from the
/// relative path, then removes the copy.
struct StagedSkillArchive {
    let url: URL
    /// Project-relative path with `/` separators, as `installSkill` expects.
    let relativePath: String

    /// Best-effort delete; safe to call more than once. The empty `skill-imports` directory stays.
    func remove() {
        _ = url.withUnsafeFileSystemRepresentation { unlink($0) }
    }
}

enum SkillArchiveStaging {
    /// Same cap the runtime enforces on a Skill archive (`MAX_SKILL_STORE_ARCHIVE_BYTES`, 8 MiB).
    static let maxArchiveBytes = 8 * 1024 * 1024
    static let stagingDirectory = ".chadex/skill-imports"
    static let gitignoreEntry = ".chadex/skill-imports/"
    /// Copies left behind by a crash or force-quit are purged once they are this old.
    static let staleAge: TimeInterval = 24 * 60 * 60

    /// Cheap checks that need no copy: a `.zip` leaf that is a regular file (not a symlink) within the size cap.
    @discardableResult
    static func preflight(_ source: URL, limit: Int = maxArchiveBytes) throws -> Int {
        guard source.pathExtension.lowercased() == "zip" else { throw SkillArchiveStagingError.notZip }
        var info = stat()
        let status = source.withUnsafeFileSystemRepresentation { lstat($0, &info) }
        guard status == 0 else { throw SkillArchiveStagingError.unreadable }
        guard (info.st_mode & S_IFMT) == S_IFREG else { throw SkillArchiveStagingError.notRegularFile }
        guard info.st_size <= off_t(limit) else { throw SkillArchiveStagingError.tooLarge }
        return Int(info.st_size)
    }

    /// Copies `source` to `<project>/.chadex/skill-imports/<uuid>.zip`. The size is checked
    /// before copying and again while copying, the destination is created exclusively with a
    /// unique name, and a failed copy leaves nothing behind.
    static func stage(
        source: URL,
        projectRoot: URL,
        limit: Int = maxArchiveBytes,
        now: Date = Date()
    ) throws -> StagedSkillArchive {
        try preflight(source, limit: limit)
        let directory = try prepareStagingDirectory(projectRoot: projectRoot)
        purgeStale(in: directory, now: now)
        // Best effort: a read-only .gitignore must not block an install whose copy is removed right after.
        // Only in a git checkout (`.git` is a directory, or a file in a worktree), so non-git folders get no new file.
        if FileManager.default.fileExists(atPath: projectRoot.appendingPathComponent(".git").path) {
            try? ensureGitignoreEntry(projectRoot: projectRoot)
        }

        let name = "\(UUID().uuidString.lowercased()).zip"
        let destination = directory.appendingPathComponent(name, isDirectory: false)
        try copyFile(from: source, to: destination, limit: limit)
        return StagedSkillArchive(url: destination, relativePath: "\(stagingDirectory)/\(name)")
    }

    /// Stages `source`, runs `body` with the project-relative path, and always removes the copy.
    static func withStagedArchive<T>(
        source: URL,
        projectRoot: URL,
        limit: Int = maxArchiveBytes,
        body: (String) async throws -> T
    ) async throws -> T {
        let staged = try stage(source: source, projectRoot: projectRoot, limit: limit)
        defer { staged.remove() }
        return try await body(staged.relativePath)
    }

    // MARK: - Staging directory

    private static func prepareStagingDirectory(projectRoot: URL) throws -> URL {
        var rootInfo = stat()
        guard projectRoot.withUnsafeFileSystemRepresentation({ stat($0, &rootInfo) }) == 0,
              (rootInfo.st_mode & S_IFMT) == S_IFDIR else {
            throw SkillArchiveStagingError.projectUnavailable
        }
        var current = projectRoot
        for component in stagingDirectory.split(separator: "/") {
            current = current.appendingPathComponent(String(component), isDirectory: true)
            try ensureRealDirectory(current)
        }
        return current
    }

    /// Creates `url` if missing; refuses anything that is not a plain directory (e.g. a symlink
    /// that would redirect the staged copy outside the project).
    private static func ensureRealDirectory(_ url: URL) throws {
        var info = stat()
        if url.withUnsafeFileSystemRepresentation({ lstat($0, &info) }) == 0 {
            guard (info.st_mode & S_IFMT) == S_IFDIR else { throw SkillArchiveStagingError.unsafeStagingDirectory }
            return
        }
        guard errno == ENOENT else { throw SkillArchiveStagingError.copyFailed }
        let made = url.withUnsafeFileSystemRepresentation { mkdir($0, 0o755) }
        guard made == 0 || errno == EEXIST else { throw SkillArchiveStagingError.copyFailed }
        guard url.withUnsafeFileSystemRepresentation({ lstat($0, &info) }) == 0,
              (info.st_mode & S_IFMT) == S_IFDIR else {
            throw SkillArchiveStagingError.unsafeStagingDirectory
        }
    }

    private static func purgeStale(in directory: URL, now: Date) {
        let fileManager = FileManager.default
        guard let entries = try? fileManager.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: [.contentModificationDateKey, .isRegularFileKey],
            options: [.skipsHiddenFiles]
        ) else { return }
        for entry in entries where entry.pathExtension == "zip" {
            guard let values = try? entry.resourceValues(forKeys: [.contentModificationDateKey, .isRegularFileKey]),
                  values.isRegularFile == true,
                  let modified = values.contentModificationDate,
                  now.timeIntervalSince(modified) > staleAge else { continue }
            _ = entry.withUnsafeFileSystemRepresentation { unlink($0) }
        }
    }

    // MARK: - Copy

    private static func copyFile(from source: URL, to destination: URL, limit: Int) throws {
        let input = source.withUnsafeFileSystemRepresentation { path in
            path.map { open($0, O_RDONLY | O_NOFOLLOW) } ?? -1
        }
        guard input >= 0 else {
            throw errno == ELOOP ? SkillArchiveStagingError.notRegularFile : SkillArchiveStagingError.unreadable
        }
        defer { close(input) }
        var info = stat()
        guard fstat(input, &info) == 0 else { throw SkillArchiveStagingError.unreadable }
        guard (info.st_mode & S_IFMT) == S_IFREG else { throw SkillArchiveStagingError.notRegularFile }
        guard info.st_size <= off_t(limit) else { throw SkillArchiveStagingError.tooLarge }

        // O_EXCL: never overwrite an existing file; O_NOFOLLOW: never write through a planted symlink.
        let output = destination.withUnsafeFileSystemRepresentation { path in
            path.map { open($0, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, mode_t(0o600)) } ?? -1
        }
        guard output >= 0 else { throw SkillArchiveStagingError.copyFailed }
        var completed = false
        defer {
            close(output)
            if !completed { _ = destination.withUnsafeFileSystemRepresentation { unlink($0) } }
        }

        var buffer = [UInt8](repeating: 0, count: 64 * 1024)
        var total = 0
        while true {
            let count = buffer.withUnsafeMutableBytes { read(input, $0.baseAddress, $0.count) }
            if count < 0 {
                if errno == EINTR { continue }
                throw SkillArchiveStagingError.unreadable
            }
            if count == 0 { break }
            total += count
            // The file may grow after the preflight; the cap holds for what is actually copied.
            guard total <= limit else { throw SkillArchiveStagingError.tooLarge }
            var written = 0
            while written < count {
                let result = buffer.withUnsafeBytes { raw in
                    write(output, raw.baseAddress!.advanced(by: written), count - written)
                }
                if result < 0 {
                    if errno == EINTR { continue }
                    throw SkillArchiveStagingError.copyFailed
                }
                written += result
            }
        }
        guard fsync(output) == 0 else { throw SkillArchiveStagingError.copyFailed }
        completed = true
    }

    // MARK: - .gitignore

    /// Appends `.chadex/skill-imports/` to the project's `.gitignore` unless it is already
    /// covered. Existing content is preserved byte for byte; only the missing entry (and a
    /// separating newline, in the file's own line-ending style) is appended.
    static func ensureGitignoreEntry(projectRoot: URL) throws {
        let url = projectRoot.appendingPathComponent(".gitignore", isDirectory: false)
        var info = stat()
        let exists = url.withUnsafeFileSystemRepresentation { lstat($0, &info) } == 0
        var existing = Data()
        if exists {
            guard (info.st_mode & S_IFMT) == S_IFREG else { throw SkillArchiveStagingError.copyFailed }
            guard let data = try? Data(contentsOf: url) else { throw SkillArchiveStagingError.copyFailed }
            existing = data
        }
        let text = String(decoding: existing, as: UTF8.self)
        if gitignoreCovers(text) { return }

        let newline = text.contains("\r\n") ? "\r\n" : "\n"
        var addition = ""
        if !existing.isEmpty, existing.last != UInt8(ascii: "\n") { addition += newline }
        addition += gitignoreEntry + newline
        let payload = Data(addition.utf8)
        do {
            if exists {
                // Append in place so the file keeps its permissions and inode.
                let handle = try FileHandle(forWritingTo: url)
                defer { try? handle.close() }
                try handle.seekToEnd()
                try handle.write(contentsOf: payload)
            } else {
                try payload.write(to: url)
            }
        } catch {
            throw SkillArchiveStagingError.copyFailed
        }
    }

    static func gitignoreCovers(_ text: String) -> Bool {
        let covering: Set<String> = [
            ".chadex", ".chadex/", "/.chadex", "/.chadex/",
            ".chadex/skill-imports", ".chadex/skill-imports/",
            "/.chadex/skill-imports", "/.chadex/skill-imports/",
        ]
        return text.split(whereSeparator: \.isNewline).contains { line in
            covering.contains(line.trimmingCharacters(in: .whitespaces))
        }
    }
}
